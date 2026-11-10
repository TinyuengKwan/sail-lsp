//! Actionable type-checker evidence for the expression under the cursor.
use super::*;
use hir_def::ExprOrPatId;
use syntax::{ast::AstNode as _, SyntaxKind as SK, SyntaxNode};

#[derive(Clone, Debug)]
pub struct CandidateExplanation {
    pub target: String,
    pub signature: String,
    pub reason: String,
    pub status: &'static str,
    pub arguments: Vec<crate::FileRange>,
    pub argument_indices: Vec<usize>,
    pub declaration: Option<crate::FileRange>,
}

#[derive(Clone, Debug)]
pub struct MismatchExplanation {
    pub range: crate::FileRange,
    pub expected: String,
    pub actual: String,
    pub origins: Vec<(crate::FileRange, String)>,
}

#[derive(Clone, Debug)]
pub struct ConstraintExplanation {
    pub constraint: String,
    pub status: &'static str,
    pub reason: Option<&'static str>,
    pub reason_code: Option<&'static str>,
    pub assumptions: Vec<String>,
    pub origin: Option<crate::FileRange>,
}

#[derive(Clone, Debug, Default)]
pub struct ExpressionExplanation {
    pub inferred: Option<String>,
    pub mismatches: Vec<MismatchExplanation>,
    pub candidates: Vec<CandidateExplanation>,
    pub constraints: Vec<ConstraintExplanation>,
}

impl Semantics<'_> {
    pub fn explain_expression(&self, file: FileText, offset: usize) -> ExpressionExplanation {
        self.explain_expression_inner(file, offset).unwrap_or_default()
    }

    fn explain_expression_inner(
        &self,
        file: FileText,
        offset: usize,
    ) -> Option<ExpressionExplanation> {
        let root = syntax::parse_query::semantic_parse_file(self.db, file).syntax_node()?;
        let token = root.token_at_offset((offset as u32).into()).right_biased()?;
        let node = token.parent()?;
        let call = node.ancestors().find(|node| node.kind() == SK::CALL_EXPR);
        let callee = call.as_ref().and_then(SyntaxNode::first_child);
        let groups = hir_def::def_query::callable_bodies_at_offset(self.db, file, offset)?;
        let owner = groups.0.entry_at_offset(offset)?;
        let result = self.imp.infer_for_callable(self.db, file, &groups.0, owner);
        let location = |span: parser::Span| crate::FileRange {
            file_id: file.file_id(self.db),
            range: base_db::text_range(span.start, span.end),
        };
        let inferred = owner
            .source_map
            .expr_at_offset(offset)
            .and_then(|id| result.expr_ty(id))
            .filter(|ty| !ty.is_error())
            .map(|ty| result.display_type(ty));
        let mut explanation = ExpressionExplanation { inferred, ..Default::default() };
        for (id, mismatch) in &result.type_mismatches {
            let Some(span) = (match id {
                ExprOrPatId::ExprId(id) => owner.source_map.expr_syntax(*id),
                ExprOrPatId::PatId(id) => owner.source_map.pat_syntax(*id),
            }) else {
                continue;
            };
            let range = base_db::text_range(span.start, span.end);
            if !(span.start <= offset && offset <= span.end)
                && !call.as_ref().is_some_and(|call| call.text_range().contains_range(range))
            {
                continue;
            }
            explanation.mismatches.push(MismatchExplanation {
                range: location(span),
                expected: result.display_type(&mismatch.expected),
                actual: result.display_type(&mismatch.actual),
                origins: self.expected_type_origins(file, span.start),
            });
        }
        for (status, obligations) in
            [("unknown", &result.unsolved_constraints), ("refuted", &result.refuted_constraints)]
        {
            for obligation in obligations {
                let span = obligation.origin.and_then(|id| owner.source_map.expr_syntax(id));
                if span.is_some_and(|span| {
                    !(span.start <= offset && offset <= span.end)
                        && !call.as_ref().is_some_and(|call| {
                            call.text_range()
                                .contains_range(base_db::text_range(span.start, span.end))
                        })
                }) {
                    continue;
                }
                explanation.constraints.push(ConstraintExplanation {
                    constraint: obligation.goal.constraint().to_text(),
                    status,
                    reason: obligation.unknown_reason.map(|reason| reason.explanation()),
                    reason_code: obligation.unknown_reason.map(|reason| reason.as_str()),
                    assumptions: obligation
                        .assumptions
                        .iter()
                        .map(|assumption| assumption.to_text())
                        .collect(),
                    origin: span.map(location),
                });
            }
        }
        if let Some(callee) = callee {
            let at = usize::from(callee.text_range().start());
            if let Some(id) = owner.source_map.expr_at_offset(at) {
                for failed in result.call_failures.get(&id).into_iter().flatten() {
                    let declaration =
                        self.resolve_callable_at(file, at, &failed.target).and_then(|definition| {
                            let location = definition.location();
                            let (tree, item) = location.item(self.db)?;
                            Some(crate::FileRange {
                                file_id: location.file_id(self.db),
                                range: base_db::text_range(
                                    item.name_span(tree).start,
                                    item.name_span(tree).end,
                                ),
                            })
                        });
                    let call_args =
                        owner.body.iter_exprs().find_map(|(_, expression)| match expression {
                            hir_def::Expr::Call { callee, args } if *callee == id => {
                                Some(args.as_slice())
                            }
                            _ => None,
                        });
                    let argument_indices = failed
                        .arguments
                        .iter()
                        .filter_map(|argument| {
                            call_args?.iter().position(|id| id == argument).map(|index| index + 1)
                        })
                        .collect();
                    explanation.candidates.push(CandidateExplanation {
                        target: failed.target.clone(),
                        signature: failed.signature.clone(),
                        reason: failed.reason.clone(),
                        status: failed.status,
                        argument_indices,
                        declaration,
                        arguments: failed
                            .arguments
                            .iter()
                            .filter_map(|id| owner.source_map.expr_syntax(*id))
                            .map(location)
                            .collect(),
                    });
                }
            }
        }
        explanation.mismatches.sort_by_key(|mismatch| mismatch.range.range.start());
        Some(explanation)
    }

    fn expected_type_origins(
        &self,
        file: FileText,
        offset: usize,
    ) -> Vec<(crate::FileRange, String)> {
        let Some(root) = syntax::parse_query::semantic_parse_file(self.db, file).syntax_node()
        else {
            return Vec::new();
        };
        let Some(node) = root
            .token_at_offset((offset as u32).into())
            .right_biased()
            .and_then(|token| token.parent())
        else {
            return Vec::new();
        };
        for ancestor in node.ancestors() {
            if ancestor.kind() == SK::ARG_LIST {
                let args: Vec<_> = ancestor.children().collect();
                let Some(index) =
                    args.iter().position(|arg| arg.text_range().contains((offset as u32).into()))
                else {
                    continue;
                };
                let Some(call) = ancestor.parent() else { continue };
                let Some(callee) = call.first_child() else { continue };
                let at = usize::from(callee.text_range().start());
                let name = callee.text().to_string();
                let Some(definition) = self.resolve_callable_at(file, at, &name) else { continue };
                if let Some(origin) =
                    self.signature_type_origin(definition.location(), Some((index, args.len())))
                {
                    return vec![(
                        origin,
                        format!("Expected type of argument {} is declared here", index + 1),
                    )];
                }
            }
            if ancestor.kind() == SK::LET_EXPR || ancestor.kind() == SK::VAR_EXPR {
                if let Some(annotation) = ancestor
                    .children()
                    .find(|node| syntax::ast::Pat::can_cast(node.kind()))
                    .and_then(|pattern| {
                        pattern.descendants().find(|node| node.kind().is_type_node())
                    })
                    .filter(|node| usize::from(node.text_range().end()) <= offset)
                {
                    return vec![(
                        crate::FileRange {
                            file_id: file.file_id(self.db),
                            range: annotation.text_range(),
                        },
                        "Explicit binding type is declared here".into(),
                    )];
                }
            }
            if ancestor.kind() == SK::CALLABLE_DEF || ancestor.kind() == SK::SCATTERED_CLAUSE_DEF {
                if let Some(annotation) = ancestor
                    .children_with_tokens()
                    .skip_while(|e| e.kind() != SK::R_ARROW)
                    .filter_map(|e| e.into_node())
                    .find(|node| node.kind().is_type_node())
                {
                    return vec![(
                        crate::FileRange {
                            file_id: file.file_id(self.db),
                            range: annotation.text_range(),
                        },
                        "Function return type is declared here".into(),
                    )];
                }
                let Some(name) = syntax::definition_name(&ancestor) else { continue };
                let Some(at) = syntax::definition_name_span(&ancestor) else { continue };
                if let Some(definition) = self.resolve_callable_at(file, at.start, &name) {
                    if let Some(origin) = self.signature_type_origin(definition.location(), None) {
                        return vec![(origin, "Function return type is declared here".into())];
                    }
                }
            }
        }
        Vec::new()
    }

    fn signature_type_origin(
        &self,
        location: crate::DefLocation,
        argument: Option<(usize, usize)>,
    ) -> Option<crate::FileRange> {
        let (tree, item) = location.item(self.db)?;
        let at = hir_def::source_order::at_offset(
            self.db,
            location.file_text,
            item.name_span(tree).start,
        )?;
        let declaration = hir_def::source_order::node(self.db, at)?;
        if declaration.kind() == SK::CALLABLE_DEF || declaration.kind() == SK::SCATTERED_CLAUSE_DEF
        {
            if let Some((index, _)) = argument {
                let list = declaration.children().find(|node| node.kind() == SK::PARAM_LIST)?;
                let separators: Vec<_> = list
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|token| token.kind() == SK::COMMA)
                    .map(|token| token.text_range())
                    .collect();
                let start = index
                    .checked_sub(1)
                    .and_then(|i| separators.get(i))
                    .map_or(list.text_range().start(), |range| range.end());
                let end =
                    separators.get(index).map_or(list.text_range().end(), |range| range.start());
                let annotation = list.descendants().find(|node| {
                    node.kind().is_type_node()
                        && start <= node.text_range().start()
                        && node.text_range().end() <= end
                })?;
                return Some(crate::FileRange {
                    file_id: location.file_id(self.db),
                    range: annotation.text_range(),
                });
            }
        }
        let mut ty = declaration.children().find(|node| node.kind().is_type_node())?;
        while ty.kind() == SK::TYPE_FORALL {
            ty = ty.children().filter(|node| node.kind().is_type_node()).last()?;
        }
        if ty.kind() != SK::TYPE_ARROW {
            return None;
        }
        let types: Vec<_> = ty.children().filter(|node| node.kind().is_type_node()).collect();
        let annotation = if let Some((index, supplied)) = argument {
            let input = types.first()?;
            let mut params: Vec<_> = if input.kind() == SK::TYPE_TUPLE {
                input.children().filter(|node| node.kind().is_type_node()).collect()
            } else {
                vec![input.clone()]
            };
            if supplied < params.len() {
                params.retain(|param| {
                    !(param.kind() == SK::TYPE_APP
                        && param.first_child().is_some_and(|head| head.text() == "implicit"))
                });
            }
            params.get(index)?.clone()
        } else {
            types.last()?.clone()
        };
        Some(crate::FileRange {
            file_id: location.file_id(self.db),
            range: annotation.text_range(),
        })
    }
}
