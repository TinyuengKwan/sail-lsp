//! Remove one unused parameter as an atomic semantic workspace refactoring.
//! Signature declarations, every implementation clause and actual call sites
//! are edited together. Argument evaluation is retained even when discarded.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use base_db::{text_range, TextRange};
use ide_db::{defs::Definition, source_change::SourceChange, text_edit::TextEdit};
use syntax::{SyntaxKind as SK, SyntaxNode};

pub(crate) fn remove_unused_param(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let (db, input) = ctx.file.file_text()?;
    let sema = hir::Semantics::new(db);
    let bodies = sema.bodies_at_offset(input, ctx.offset())?;
    let owner = bodies.entry_at_offset(ctx.offset())?;
    let (index, name, target) =
        owner.body.params.iter().enumerate().find_map(|(index, &pat)| {
            let binding = binding(&owner.body, pat)?;
            let span = owner.source_map.pat_syntax(binding.0)?;
            (span.start <= ctx.offset() && ctx.offset() <= span.end)
                .then(|| (index, binding.1.to_owned(), text_range(span.start, span.end)))
        })?;
    let root = ctx.source_file()?;
    let declaration = root
        .children()
        .find(|node| usize::from(node.text_range().start()) == owner.def_span.start)?;
    let at = name_offset(&declaration)?;
    let selected = Definition::classify_at(ctx.file, ctx.file.position_at(at))?.canonical(db);
    let function_name = selected.name(db)?.to_string();
    let local = [(input.file_id(db), ctx.file)];
    let workspace = if ctx.workspace.is_empty() { &local[..] } else { ctx.workspace };
    let mut change = SourceChange::new();
    let mut implementations = 0;
    let mut direct_targets = vec![selected.clone()];
    let mut implicit = vec![false; owner.body.params.len()];
    // First collect declarations so all call sites see the same original arity.
    for &(file_id, file) in workspace {
        db.unwind_if_revision_cancelled();
        let (_, ft) = file.file_text()?;
        let root = file.syntax()?;
        for node in root.children() {
            if !matches!(
                node.kind(),
                SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF | SK::CALLABLE_SPEC
            ) {
                continue;
            }
            let Some(at) = name_offset(&node) else { continue };
            let Some(definition) = Definition::classify_at(file, file.position_at(at)) else {
                continue;
            };
            if !same_callable(db, &definition, &selected) {
                continue;
            }
            let canonical = definition.canonical(db);
            if !direct_targets.contains(&canonical) {
                direct_targets.push(canonical);
            }
            if node.kind() == SK::CALLABLE_SPEC {
                let arrow = signature_arrow(&node)?;
                let parameters = arrow.children().find(|node| node.kind().is_type_node())?;
                let slots: Vec<_> = if parameters.kind() == SK::TYPE_TUPLE {
                    parameters.children().filter(|node| node.kind().is_type_node()).collect()
                } else {
                    vec![parameters.clone()]
                };
                if slots.len() != owner.body.params.len() {
                    return None;
                }
                implicit = slots.iter().map(is_implicit).collect();
                let replacement = if slots.len() <= 2 {
                    let remaining = slots
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != index)
                        .map(|(_, node)| node.text().to_string())
                        .collect::<Vec<_>>();
                    Some(TextEdit {
                        range: parameters.text_range(),
                        new_text: remaining.first().cloned().unwrap_or_else(|| "unit".into()),
                    })
                } else {
                    remove_slot(&parameters, index)
                }?;
                change.insert_source_edit(file_id, vec![replacement]);
                continue;
            }
            let group = sema.bodies_at_offset(ft, usize::from(node.text_range().start()))?;
            let entry = group
                .entries()
                .iter()
                .find(|entry| entry.def_span.start == usize::from(node.text_range().start()))?;
            let pat = *entry.body.params.get(index)?;
            let (pat, parameter) = binding(&entry.body, pat)?;
            let scopes =
                hir::source_order::bindings::body_scopes(db, ft, &entry.body, entry.def_span.start);
            if entry.body.iter_exprs().any(|(expr, value)| {
                matches!(value, hir::Expr::Ident(name) if name.as_str() == parameter)
                    && scopes
                        .scope_for(expr)
                        .and_then(|scope| {
                            scopes.resolve_name_in_scope(scope, &hir::Name::new(parameter))
                        })
                        .is_some_and(|bound| bound.pat() == pat)
            }) {
                return None;
            }
            // Type expressions can also depend on a value parameter.
            if node.descendants().filter(|child| child.kind().is_type_node()).any(|ty| {
                ty.descendants_with_tokens().filter_map(|e| e.into_token()).any(|token| {
                    matches!(token.kind(), SK::IDENT | SK::TY_VAR)
                        && token.text().trim_start_matches('\'') == parameter
                })
            }) {
                return None;
            }
            let list = node.children().find(|node| node.kind() == SK::PARAM_LIST)?;
            let edit = remove_slot(&list, index)?;
            change.insert_source_edit(file_id, vec![edit]);
            implementations += 1;
        }
    }
    if implementations == 0 {
        return None;
    }
    // An overload can hide a higher-order or infix use of the changed
    // function. Follow semantic membership, including nested aliases, then
    // require a known named call at every use before changing any signature.
    let mut related: Vec<_> = direct_targets
        .iter()
        .map(|definition| (definition.clone(), function_name.clone()))
        .collect();
    loop {
        let before = related.len();
        for &(_, file) in workspace {
            let root = file.syntax()?;
            for node in root.children().filter(|node| {
                node.kind() == SK::NAMED_DEF
                    && node.children_with_tokens().any(|element| element.kind() == SK::KW_OVERLOAD)
            }) {
                let name_span = syntax::definition_name_span(&node)?;
                let group =
                    Definition::classify_at(file, file.position_at(name_span.start))?.canonical(db);
                if related.iter().any(|(definition, _)| *definition == group) {
                    continue;
                }
                let contains = node
                    .descendants_with_tokens()
                    .filter_map(|element| element.into_token())
                    .filter(|token| {
                        !token.kind().is_trivia()
                            && usize::from(token.text_range().start()) != name_span.start
                    })
                    .filter_map(|token| {
                        Definition::classify_at(
                            file,
                            file.position_at(usize::from(token.text_range().start())),
                        )
                    })
                    .any(|definition| {
                        related.iter().any(|(member, _)| same_callable(db, &definition, member))
                    });
                if contains {
                    related.push((group, syntax::definition_name(&node)?));
                }
            }
        }
        if related.len() == before {
            break;
        }
    }
    for &(file_id, file) in workspace {
        let (_, ft) = file.file_text()?;
        let root = file.syntax()?;
        // A higher-order use has no syntactically known call list to update.
        for target in &direct_targets {
            let search = ide_db::search::FindUsages::new(target.clone(), &function_name);
            for (span, declaration) in search.spans_in_file(file) {
                if declaration {
                    continue;
                }
                let token = root
                    .token_at_offset((span.start as u32).into())
                    .find(|token| token.text_range().start() == (span.start as u32).into())?;
                let node = token.parent()?;
                if node.ancestors().any(|node| {
                    matches!(
                        node.kind(),
                        SK::SCATTERED_DEF | SK::SCATTERED_CLAUSE_DEF | SK::END_DEF
                    ) && syntax::definition_name_span(&node) == Some(span)
                }) {
                    continue;
                }
                if node.ancestors().any(|node| {
                    node.kind() == SK::NAMED_DEF
                        && node.children_with_tokens().any(|e| e.kind() == SK::KW_OVERLOAD)
                }) {
                    continue;
                }
                if !node.parent().is_some_and(|call| {
                    call.kind() == SK::CALL_EXPR && call.first_child().as_ref() == Some(&node)
                }) {
                    return None;
                }
            }
        }
        for (alias, name) in
            related.iter().filter(|(definition, _)| !direct_targets.contains(definition))
        {
            let search = ide_db::search::FindUsages::new(alias.clone(), name);
            for (span, declaration) in search.spans_in_file(file) {
                if declaration {
                    continue;
                }
                let token = root.token_at_offset((span.start as u32).into()).right_biased()?;
                let node = token.parent()?;
                if node.ancestors().any(|node| {
                    node.kind() == SK::NAMED_DEF
                        && node
                            .children_with_tokens()
                            .any(|element| element.kind() == SK::KW_OVERLOAD)
                }) {
                    continue;
                }
                if !node.parent().is_some_and(|call| {
                    call.kind() == SK::CALL_EXPR && call.first_child().as_ref() == Some(&node)
                }) {
                    return None;
                }
                // An ambiguous application might choose the member after this
                // edit. Never silently leave a possibly affected call stale.
                sema.resolve_overload_callee(ft, span.start)?;
            }
        }
        let mut calls: Vec<_> =
            root.descendants().filter(|node| node.kind() == SK::CALL_EXPR).collect();
        calls.sort_by_key(|call| call.text_range().len());
        let mut call_edits: Vec<TextEdit> = Vec::new();
        for call in calls {
            let Some(callee) = call.first_child().filter(|node| node.kind() == SK::IDENT_EXPR)
            else {
                continue;
            };
            let at = usize::from(callee.text_range().start());
            let definition = Definition::classify_at(file, file.position_at(at));
            let chosen = match definition {
                Some(Definition::Overload(_)) => {
                    sema.resolve_overload_callee(ft, at).and_then(Definition::from_def)
                }
                other => other,
            };
            if !chosen.is_some_and(|def| same_callable(db, &def, &selected)) {
                continue;
            }
            let args: Vec<_> = call
                .children()
                .find(|node| node.kind() == SK::ARG_LIST)
                .map(|node| node.children().collect())
                .unwrap_or_default();
            let explicit = implicit.iter().filter(|&&value| !value).count();
            let argument = if args.len() == implicit.len() {
                Some(index)
            } else if args.len() == explicit {
                (!implicit[index])
                    .then(|| implicit[..index].iter().filter(|&&value| !value).count())
            } else {
                return None;
            };
            let Some(argument) = argument else { continue };
            let instantiation = sema.call_instantiation(ft, at);
            let mut occupied: std::collections::HashSet<String> = root
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| t.kind() == SK::IDENT)
                .map(|token| token.text().to_owned())
                .collect();
            let mut statements = Vec::new();
            let mut remaining = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                let mut fresh = format!("__sail_argument{i}");
                while !occupied.insert(fresh.clone()) {
                    fresh.push('_');
                }
                let parameter_index = if args.len() == implicit.len() {
                    i
                } else {
                    implicit.iter().enumerate().filter(|(_, value)| !**value).nth(i)?.0
                };
                let annotation = instantiation
                    .as_ref()
                    .and_then(|instance| {
                        let hir::TyKind::FnPtr(sig) = instance.ty.kind() else { return None };
                        sig.params
                            .get(parameter_index)
                            .map(|ty| format!(" : {}", crate::utils::argument_annotation(ty, db)))
                    })
                    .unwrap_or_default();
                let mut argument_text = arg.text().to_string();
                let mut nested: Vec<_> = call_edits
                    .iter()
                    .filter(|edit| arg.text_range().contains_range(edit.range))
                    .collect();
                nested.sort_by_key(|edit| edit.range.start());
                for edit in nested.into_iter().rev() {
                    argument_text.replace_range(
                        usize::from(edit.range.start() - arg.text_range().start())
                            ..usize::from(edit.range.end() - arg.text_range().start()),
                        &edit.new_text,
                    );
                }
                statements.push(format!("let {fresh}{annotation} = {argument_text};"));
                if i != argument {
                    remaining.push(fresh);
                }
            }
            statements.push(format!("{}({})", callee.text(), remaining.join(", ")));
            call_edits.retain(|edit| !call.text_range().contains_range(edit.range));
            call_edits.push(TextEdit {
                range: call.text_range(),
                new_text: format!("{{ {} }}", statements.join(" ")),
            });
        }
        if !call_edits.is_empty() {
            change.insert_source_edit(file_id, call_edits);
        }
    }
    // Nested calls have been composed above. Declarations and call rewrites
    // must still form one non-overlapping workspace change.
    for edits in change.source_file_edits.values_mut() {
        edits.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
        edits.dedup();
        if edits.windows(2).any(|pair| pair[0].range.end() > pair[1].range.start()) {
            return None;
        }
    }
    acc.add_source_change(
        AssistId("remove_unused_param", AssistKind::RefactorRewrite),
        format!("Remove unused parameter `{name}`"),
        target,
        change,
    );
    Some(())
}

/// A source-defined callable can have multiple semantic include views. Its
/// physical declaration is edited once, and calls from every equivalent view
/// must be updated. The outer Analysis boundary still checks shared edits.
fn same_callable(db: &dyn salsa::Database, left: &Definition, right: &Definition) -> bool {
    let left = left.canonical(db);
    let right = right.canonical(db);
    if left == right {
        return true;
    }
    if std::mem::discriminant(&left) != std::mem::discriminant(&right) {
        return false;
    }
    let source = |definition: &Definition| {
        let location = definition.def_location()?;
        let (tree, item) = location.item(db)?;
        Some((base_db::preprocess::physical_file(db, location.file_id(db)), item.name_span(tree)))
    };
    source(&left).is_some_and(|left| Some(left) == source(&right))
}

fn binding(body: &hir::Body, mut pat: hir::PatId) -> Option<(hir::PatId, &str)> {
    loop {
        match body.pat(pat)? {
            hir::Pat::Typed { inner, .. } => pat = *inner,
            hir::Pat::Bind(name) => return Some((pat, name.as_str())),
            _ => return None,
        }
    }
}

fn name_offset(node: &SyntaxNode) -> Option<usize> {
    syntax::definition_name_span(node).map(|span| span.start)
}

fn signature_arrow(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut ty = node.children().find(|node| node.kind().is_type_node())?;
    while ty.kind() == SK::TYPE_FORALL {
        ty = ty.children().filter(|node| node.kind().is_type_node()).last()?;
    }
    (ty.kind() == SK::TYPE_ARROW).then_some(ty)
}

fn is_implicit(node: &SyntaxNode) -> bool {
    node.kind() == SK::TYPE_APP && node.first_child().is_some_and(|name| name.text() == "implicit")
}

fn remove_slot(list: &SyntaxNode, index: usize) -> Option<TextEdit> {
    let range = list.text_range();
    let opening = list.first_token()?.text_range().end();
    let closing = list.last_token()?.text_range().start();
    let commas: Vec<_> = list
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|token| token.kind() == SK::COMMA)
        .map(|token| token.text_range())
        .collect();
    let begin =
        index.checked_sub(1).map_or(Some(opening), |i| commas.get(i).map(|range| range.end()))?;
    let end = commas.get(index).map_or(closing, |range| range.start());
    let text = list.text().to_string();
    let segment = &text[usize::from(begin - range.start())..usize::from(end - range.start())];
    if segment.trim().is_empty() {
        return None;
    }
    let start =
        begin + base_db::TextSize::from((segment.len() - segment.trim_start().len()) as u32);
    let finish = end - base_db::TextSize::from((segment.len() - segment.trim_end().len()) as u32);
    let (from, to) = if let Some(next) = commas.get(index) {
        (start, next.end())
    } else if let Some(previous) = index.checked_sub(1).and_then(|i| commas.get(i)) {
        (previous.start(), finish)
    } else {
        (start, finish)
    };
    Some(TextEdit { range: TextRange::new(from, to), new_text: String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    fn signature_and_calls_change_together() {
        check_assist(
            remove_unused_param,
            "val f : (int, int) -> int\nfunction f(x, $0y) = x\nfunction g() = f(1, 2)\n",
            expect![[r#"
                val f : int -> int
                function f(x) = x
                function g() = { let __sail_argument0 : int = 1; let __sail_argument1 : int = 2; f(__sail_argument0) }
            "#]],
        );
    }

    #[test]
    fn applies_without_a_val_and_keeps_unused_argument_evaluation() {
        check_assist(
            remove_unused_param,
            "function f(x, $0y) = x\nfunction g() = f(1, 2)\n",
            expect![[r#"
                function f(x) = x
                function g() = { let __sail_argument0 = 1; let __sail_argument1 = 2; f(__sail_argument0) }
            "#]],
        );
    }

    #[test]
    fn used_parameters_are_not_removed() {
        check_assist_not_applicable(
            remove_unused_param,
            "function f(x, $0y) = x + y\nfunction g() = f(1, 2)\n",
        );
    }
}
