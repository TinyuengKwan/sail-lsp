//! Opt-in, fuel-bounded expression synthesis. Every proposed call is proved
//! jointly; display strings and independently matched parameters are insufficient.
use super::Completions;
use crate::{context::CompletionContext, item::CompletionItem};
use hir::Ty;
use ide_db::defs::CompletionItemKind;

#[derive(Clone)]
struct Candidate {
    text: String,
    ty: Ty,
}

struct Search<'a, 'db> {
    ctx: &'a CompletionContext<'db>,
    file: base_db::FileText,
    names: Vec<hir::Name>,
    leaves: Vec<Candidate>,
    fuel: usize,
}

impl Search<'_, '_> {
    fn step(&mut self) -> bool {
        self.ctx.db.unwind_if_revision_cancelled();
        if self.fuel == 0 {
            return false;
        }
        self.fuel -= 1;
        true
    }

    fn expressions(&mut self, goal: Option<&Ty>, depth: usize, root: bool) -> Vec<Candidate> {
        let mut out = Vec::new();
        for index in 0..if root { 0 } else { self.leaves.len() } {
            if !self.step() {
                return out;
            }
            let leaf = self.leaves[index].clone();
            if goal.is_none_or(|goal| {
                self.ctx
                    .sema
                    .completion_type_match(self.file, self.ctx.semantic_offset(), goal, &leaf.ty)
                    .is_some()
            }) {
                out.push(leaf);
                if out.len() >= 8 {
                    break;
                }
            }
        }
        let Some(goal) = goal.filter(|_| depth > 0) else { return out };
        let prefix = self.ctx.prefix.to_lowercase();
        for index in 0..self.names.len() {
            if !self.step() || out.len() >= 8 {
                break;
            }
            let name = self.names[index].clone();
            if root && !name.as_str().to_lowercase().starts_with(&prefix) {
                continue;
            }
            let shapes = self.ctx.sema.completion_parameter_goals(
                self.file,
                self.ctx.semantic_offset(),
                goal,
                name.as_str(),
                &mut self.fuel,
            );
            for goals in shapes {
                let mut combinations: Vec<Vec<Candidate>> = vec![Vec::new()];
                for param in goals {
                    let choices = self.expressions(param.as_ref(), depth - 1, false);
                    let mut next = Vec::new();
                    for previous in &combinations {
                        for choice in &choices {
                            if !self.step() {
                                break;
                            }
                            let mut args = previous.clone();
                            args.push(choice.clone());
                            next.push(args);
                            if next.len() >= 8 {
                                break;
                            }
                        }
                        if next.len() >= 8 || self.fuel == 0 {
                            break;
                        }
                    }
                    combinations = next;
                    if combinations.is_empty() {
                        break;
                    }
                }
                for args in combinations {
                    if !self.step() {
                        break;
                    }
                    let types: Vec<_> = args.iter().map(|arg| arg.ty.clone()).collect();
                    if self.ctx.sema.completion_application_matches(
                        self.file,
                        self.ctx.semantic_offset(),
                        goal,
                        name.as_str(),
                        &types,
                        &mut self.fuel,
                    ) {
                        let text = format!(
                            "{}({})",
                            name.as_str(),
                            args.iter().map(|arg| arg.text.as_str()).collect::<Vec<_>>().join(", ")
                        );
                        if !out.iter().any(|candidate| candidate.text == text) {
                            out.push(Candidate { text, ty: goal.clone() });
                        }
                    }
                    if out.len() >= 8 {
                        break;
                    }
                }
                if out.len() >= 8 || self.fuel == 0 {
                    break;
                }
            }
        }
        out
    }
}

pub(super) fn complete(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    if !ctx.config.term_search
        || !ctx.config.enable_snippets
        || ctx.config.term_search_fuel == 0
        || ctx.is_callee
    {
        return;
    }
    let (Some(file), Some(expected)) = (ctx.file_text, ctx.expected_type.as_ref()) else { return };
    let mut fuel = ctx.config.term_search_fuel.min(10000);
    let mut leaves = Vec::new();
    for local in ctx.sema.locals_in_scope(file, ctx.semantic_offset()) {
        if fuel == 0 {
            break;
        }
        fuel -= 1;
        if let Some(ty) = ctx.sema.type_of_pat(file, local.span.start) {
            leaves.push(Candidate { text: local.name.to_string(), ty });
        }
    }
    for (text, ty) in [
        ("true", Ty::app("bool", vec![hir::ty::TyArg::numeric("true")], "")),
        ("false", Ty::app("bool", vec![hir::ty::TyArg::numeric("false")], "")),
        ("0", Ty::app("atom", vec![hir::ty::TyArg::numeric("0")], "")),
        ("()", Ty::scalar(hir::Scalar::Unit)),
    ] {
        leaves.push(Candidate { text: text.into(), ty });
    }
    let mut search = Search {
        ctx,
        file,
        names: ctx.sema.names_in_scope(file, ctx.semantic_offset()),
        leaves,
        fuel,
    };
    let candidates = search.expressions(Some(expected), 3, true);
    for candidate in candidates {
        acc.add(
            CompletionItem::new(CompletionItemKind::Text, &candidate.text)
                .detail("type-directed expression")
                .filter_text(candidate.text.split('(').next().unwrap_or(&candidate.text))
                .insert_text(&candidate.text),
        );
    }
}
