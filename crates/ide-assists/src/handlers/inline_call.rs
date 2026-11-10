//! `inline_call` assist.
//! Inlines a function body after evaluating each argument once, in call order.
//! Fresh temporary bindings preserve argument scope and avoid name capture.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use hir::HirDisplay;
use ide_db::ide_types::IdeTextEdit;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;
use syntax::{SyntaxKind as SK, SyntaxNode};

pub(crate) fn inline_call(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = inline_call_edits(ctx.file, ctx.offset())?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_call", AssistKind::RefactorInline),
        "Inline function call",
        ctx.range,
        te,
    );
    Some(())
}

/// Inline one uniquely selected ordinary function. Stage every argument before
/// introducing the callee's parameters: this preserves left-to-right, exactly
/// once evaluation even for unused parameters and shadowing inside the body.
pub fn inline_call_edits(file: &dyn FileDb, offset: usize) -> Option<Vec<IdeTextEdit>> {
    let root = file.syntax().unwrap_or_else(|| syntax::parse_text(file.text()).0);
    let token = root.token_at_offset((offset as u32).into()).find(|t| t.kind() == SK::IDENT)?;
    let callee = token.parent().filter(|n| n.kind() == SK::IDENT_EXPR)?;
    let call = callee.parent().filter(|n| n.kind() == SK::CALL_EXPR)?;
    if call.first_child()? != callee {
        return None;
    }
    let args: Vec<_> = call
        .children()
        .find(|n| n.kind() == SK::ARG_LIST)
        .map(|n| n.children().collect())
        .unwrap_or_default();
    let name = token.text();
    let semantic = file.file_text().map(|(db, ft)| (hir::Semantics::new(db), db, ft));
    let selected = if let Some((_, db, _)) = &semantic {
        let def = ide_db::defs::Definition::classify_at(file, file.position_at(offset))?;
        if !matches!(
            def,
            ide_db::defs::Definition::Function(_) | ide_db::defs::Definition::ValSpec(_)
        ) {
            return None;
        }
        Some(def.canonical(*db))
    } else {
        None
    };
    let mut definitions = root.children().filter(|n| {
        if n.kind() != SK::CALLABLE_DEF || syntax::definition_name(n).as_deref() != Some(name) {
            return false;
        }
        match (&semantic, &selected) {
            (Some((_, db, _)), Some(selected)) => n
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .and_then(|t| {
                    ide_db::defs::Definition::classify_at(
                        file,
                        file.position_at(usize::from(t.text_range().start())),
                    )
                })
                .is_some_and(|def| def.canonical(*db) == *selected),
            _ => true,
        }
    });
    let def = definitions.next()?;
    if definitions.next().is_some() {
        return None;
    }
    let params = formal_params(&def)?;
    let body = hir::CallableBodies::cst_callable_body_node(&def)?;
    if let Some((sema, _, file)) = &semantic {
        let groups = sema.bodies_at_offset(*file, usize::from(def.text_range().start()))?;
        if groups
            .entries()
            .iter()
            .find(|entry| entry.def_span.start == usize::from(def.text_range().start()))?
            .body
            .guard
            .is_some()
        {
            return None;
        }
    }

    // A return exits a function, not its enclosing block. Moving it into the
    // caller changes its destination; it needs a separate control-flow rewrite.
    if body.descendants_with_tokens().any(|e| e.kind() == SK::KW_RETURN) {
        return None;
    }
    if let Some((sema, _, ft)) = &semantic {
        let caller_locals = sema.locals_in_scope(*ft, offset);
        for reference in body.descendants().filter(|n| n.kind() == SK::IDENT_EXPR) {
            let Some(t) = reference.first_token() else { continue };
            let at = usize::from(t.text_range().start());
            let Some(definition) =
                ide_db::defs::Definition::classify_at(file, file.position_at(at))
            else {
                continue;
            };
            if !definition.is_local() && caller_locals.iter().any(|l| l.name.as_str() == t.text()) {
                return None;
            }
            // Resolving the original binding is required before moving it.
            if matches!(definition, ide_db::defs::Definition::Unresolved(_)) {
                return None;
            }
        }
    }
    let mut occupied: std::collections::HashSet<String> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SK::IDENT | SK::TY_VAR))
        .map(|t| t.text().to_owned())
        .collect();
    let instance = semantic.as_ref().and_then(|(sema, _, ft)| sema.call_instantiation(*ft, offset));
    let signature = instance.as_ref().and_then(|instance| match instance.ty.kind() {
        hir::TyKind::FnPtr(sig) if sig.params.len() == params.len() => Some(sig),
        _ => None,
    });
    let implicit = instance.as_ref().map(|instance| instance.implicit_params.as_slice());
    let omitted = params.len() != args.len();
    let supplied: Vec<_> = (0..params.len())
        .filter(|index| {
            !omitted || !implicit.and_then(|flags| flags.get(*index)).copied().unwrap_or(false)
        })
        .collect();
    if supplied.len() != args.len() {
        return None;
    }
    // A quantified annotation cannot be moved into the caller unless the
    // particular application supplied a proven concrete signature.
    if signature.is_none() {
        for token in def
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| {
                token.kind() == SK::TY_VAR && !body.text_range().contains_range(token.text_range())
            })
        {
            let annotation =
                token.parent()?.ancestors().filter(|node| node.kind().is_type_node()).last()?;
            let Some((_, db, ft)) = &semantic else { return None };
            let Some(ide_db::defs::Definition::TypeVar(variable)) =
                ide_db::defs::Definition::classify_at(
                    file,
                    file.position_at(usize::from(token.text_range().start())),
                )
            else {
                return None;
            };
            let (owner, span) = variable.binding?;
            // Existential binders contained in the copied annotation keep
            // their scope. A free callee quantifier would escape into caller.
            if owner != ft.file_id(*db)
                || !annotation.text_range().contains((span.start as u32).into())
            {
                return None;
            }
        }
    }
    let mut body_text = body.text().to_string();
    let mut substitutions = Vec::new();
    if let Some((_, db, ft)) = &semantic {
        for token in body
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SK::TY_VAR)
        {
            let at = usize::from(token.text_range().start());
            let Some(ide_db::defs::Definition::TypeVar(variable)) =
                ide_db::defs::Definition::classify_at(file, file.position_at(at))
            else {
                continue;
            };
            let (owner, span) = variable.binding?;
            if owner == ft.file_id(*db) && body.text_range().contains((span.start as u32).into()) {
                continue;
            }
            let (_, replacement) =
                instance.as_ref()?.type_arguments.iter().find(|(name, _)| name == token.text())?;
            substitutions.push((token.text_range(), replacement.display_source_code(*db).ok()?));
        }
    }
    substitutions.sort_by_key(|(range, _)| range.start());
    for (range, replacement) in substitutions.iter().rev() {
        body_text.replace_range(
            usize::from(range.start() - body.text_range().start())
                ..usize::from(range.end() - body.text_range().start()),
            replacement,
        );
    }
    let mut temporary = Vec::new();
    let mut statements = Vec::new();
    for (index, parameter) in params.iter().enumerate() {
        let argument = if let Some(position) = supplied.iter().position(|&slot| slot == index) {
            args[position].text().to_string()
        } else {
            let ty = &signature?.params[index];
            let args = match ty.kind() {
                hir::TyKind::App { name, args, .. } if name == "implicit" => args,
                _ => return None,
            };
            let value = args.first()?.as_value_str()?;
            let digits = value.strip_prefix('-').unwrap_or(&value);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            value
        };
        let mut fresh = format!("__sail_inline_arg{index}");
        while !occupied.insert(fresh.clone()) {
            fresh.push('_');
        }
        let annotation = signature
            .and_then(|sig| {
                semantic
                    .as_ref()
                    .map(|(_, db, _)| crate::utils::argument_annotation(&sig.params[index], *db))
            })
            .or_else(|| parameter.1.clone());
        let annotation = annotation.map(|ty| format!(" : {ty}")).unwrap_or_default();
        statements.push(format!("let {fresh}{annotation} = {argument};"));
        temporary.push(fresh);
    }
    for ((param, _), value) in params.iter().zip(&temporary) {
        statements.push(format!("let {param} = {value};"));
    }
    let return_annotation = signature
        .and_then(|sig| semantic.as_ref().map(|(_, db, _)| sig.ret.display_to_string(*db)))
        .or_else(|| {
            def.children_with_tokens()
                .skip_while(|e| e.kind() != SK::R_ARROW)
                .filter_map(|e| e.into_node())
                .find(|n| n.kind().is_type_node())
                .map(|n| n.text().to_string())
        });
    if let Some(annotation) = return_annotation {
        let mut result = "__sail_inline_result".to_owned();
        while !occupied.insert(result.clone()) {
            result.push('_');
        }
        statements.push(format!("let {result} : {annotation} = {body_text};"));
        statements.push(result);
    } else {
        statements.push(body_text);
    }
    let range = call.text_range();
    Some(vec![IdeTextEdit {
        range: base_db::text_range(usize::from(range.start()), usize::from(range.end())),
        new_text: format!("{{ {} }}", statements.join(" ")),
    }])
}

/// The parameter names of a definition, or `None` if any is a pattern
/// rather than a name.
fn formal_params(def: &SyntaxNode) -> Option<Vec<(String, Option<String>)>> {
    let list = def.children().find(|n| n.kind() == SK::PARAM_LIST);
    let Some(list) = list else {
        // `f()` lowers to UNIT: no parameters.
        return def.children_with_tokens().any(|e| e.kind() == SK::UNIT).then(Vec::new);
    };
    let mut names: Vec<(String, Option<String>)> = Vec::new();
    for element in list.children_with_tokens() {
        match element.kind() {
            SK::IDENT => names.push((element.into_token()?.text().to_owned(), None)),
            SK::L_PAREN | SK::R_PAREN | SK::COMMA | SK::COLON | SK::WHITESPACE => {}
            kind if kind.is_type_node() => names.last_mut()?.1 = Some(element.to_string()),
            _ => return None,
        }
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    fn replacement(source: &str, name: &str) -> String {
        let file = ide_db::test_utils::TestFile::new(source);
        let edits = inline_call_edits(&file, source.rfind(name).unwrap()).unwrap();
        assert_eq!(edits.len(), 1);
        edits[0].new_text.clone()
    }

    #[test]
    fn effectful_and_unused_arguments_are_evaluated_once_in_order() {
        let source = "val left : unit -> int\nval right : unit -> int\nfunction f(x : int, y : int) -> int = y + y\nfunction g() -> int = f(left(), right())\n";
        let result = replacement(source, "f(");
        assert_eq!(result.matches("left()").count(), 1);
        assert_eq!(result.matches("right()").count(), 1);
        assert!(result.find("left()").unwrap() < result.find("right()").unwrap());
        assert!(result.find("right()").unwrap() < result.find("let x =").unwrap());
        assert!(syntax::parse_text(&format!("function g() = {result}\n")).1.is_empty());
    }

    #[test]
    fn all_arguments_precede_parameter_bindings_and_fresh_names_do_not_capture() {
        let source = "function f(x : int, y : int) -> int = x + y\nfunction g(x : int, __sail_inline_arg0 : int) -> int = f(x, __sail_inline_arg0)\n";
        let result = replacement(source, "f(");
        assert!(result.contains("let __sail_inline_arg0_"));
        assert!(result.find("= __sail_inline_arg0;").unwrap() < result.find("let x =").unwrap());
    }

    #[test]
    fn a_local_callee_is_not_a_same_spelled_top_level_function() {
        crate::tests::check_assist_not_applicable(
            inline_call,
            "function f(x : int) -> int = x\nfunction g(f : int -> int) -> int = $0f(1)\n",
        );
    }

    #[test]
    fn an_early_return_cannot_be_moved_into_the_caller() {
        crate::tests::check_assist_not_applicable(inline_call,
            "function f(x : int) -> int = { if x > 0 then return x; 0 }\nfunction g(x : int) -> int = $0f(x) + 1\n");
    }

    /// Arguments and the result keep their grouping.
    #[test]
    fn grouping_survives() {
        check_assist(
            inline_call,
            "function dbl(x : int) -> int = x + x\nfunction g(a : int) -> int = 2 * $0dbl(a + 1)\n",
            expect![[r#"
                function dbl(x : int) -> int = x + x
                function g(a : int) -> int = 2 * { let __sail_inline_arg0 : int = a + 1; let x = __sail_inline_arg0; let __sail_inline_result : int = x + x; __sail_inline_result }
            "#]],
        );
    }

    /// The body starts after the definition's `=`, not at a `>=`, and a
    /// field spelled like a parameter is not one.
    #[test]
    fn the_body_and_its_mentions_come_from_the_tree() {
        check_assist(
            inline_call,
            "struct S = { x : int }\n\
             function get(x : S) -> {'n, 'n >= 0. int('n)} = x.x\n\
             function g(s : S) -> int = $0get(s)\n",
            expect![[r#"
                struct S = { x : int }
                function get(x : S) -> {'n, 'n >= 0. int('n)} = x.x
                function g(s : S) -> int = { let __sail_inline_arg0 : S = s; let x = __sail_inline_arg0; let __sail_inline_result : {'n, 'n >= 0. int('n)} = x.x; __sail_inline_result }
            "#]],
        );
    }

    /// Staging arguments before parameter bindings prevents capture.
    #[test]
    fn a_capturing_body_uses_staged_arguments() {
        check_assist(
            inline_call,
            "function f(x : int) -> int = { let y : int = 1; x + y }\n\
             function g(y : int) -> int = $0f(y)\n",
            expect![[r#"
                function f(x : int) -> int = { let y : int = 1; x + y }
                function g(y : int) -> int = { let __sail_inline_arg0 : int = y; let x = __sail_inline_arg0; let __sail_inline_result : int = { let y : int = 1; x + y }; __sail_inline_result }
            "#]],
        );
    }

    #[test]
    fn applies() {
        check_assist(
            inline_call,
            "val g : int -> int\nfunction g(x) = x\nval f : int -> int\nfunction f(x) = $0g(x)\n",
            expect![[r#"
                val g : int -> int
                function g(x) = x
                val f : int -> int
                function f(x) = { let __sail_inline_arg0 : int = x; let x = __sail_inline_arg0; let __sail_inline_result : int = x; __sail_inline_result }
            "#]],
        );
    }
}
