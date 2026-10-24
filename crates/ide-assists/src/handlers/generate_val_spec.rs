//! `generate_val_spec` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn generate_val_spec(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let offset = ctx.offset();

    // Parse CST and find the callable at cursor
    // The tree the assist run already parsed. Parsing the text a second
    // time here built one unrelated to it.
    let cst_root = ctx.source_file()?.clone();

    // Find the CALLABLE_DEF node containing the cursor
    let mut target_node = None;
    for child in cst_root.children() {
        if child.kind() != SK::CALLABLE_DEF {
            continue;
        }
        let range = child.text_range();
        if usize::from(range.start()) <= offset && offset <= usize::from(range.end()) {
            target_node = Some(child);
            break;
        }
    }
    let node = target_node?;

    // Extract function name from the node
    let mut func_name = None;
    let mut is_function = false;
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if tok.kind() == SK::KW_FUNCTION {
            is_function = true;
        }
        if tok.kind() == SK::IDENT && func_name.is_none() {
            func_name = Some(tok.text().to_string());
            break;
        }
    }
    if !is_function {
        return None;
    }
    let name = func_name?;

    // Check if a val spec already exists for this name
    let val_exists = cst_root.children().any(|child| {
        if child.kind() != SK::CALLABLE_SPEC {
            return false;
        }
        child
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|tok| tok.kind() == SK::IDENT && tok.text() == name)
    });
    if val_exists {
        return None; // Already has a val spec
    }

    // Extract parameter names from PARAM_LIST, with the span of each so
    // inference can be asked what it is.
    let mut params: Vec<(String, parser::Span)> = Vec::new();
    for child in node.children() {
        if child.kind() == SK::PARAM_LIST {
            for tok in child.descendants_with_tokens().filter_map(|el| el.into_token()) {
                if tok.kind() == SK::IDENT {
                    let range = tok.text_range();
                    params.push((
                        tok.text().to_string(),
                        parser::Span::new(usize::from(range.start()), usize::from(range.end())),
                    ));
                }
            }
            break;
        }
    }

    // What inference makes of each parameter and of the body.
    //
    // This wrote `_` for every one of them, so the generated `val` never
    // type-checked and the user had to fill it in by hand. Sixty-four
    // handlers and not one asked `ctx.sema()` anything — the accessor had
    // no caller at all — while rust-analyzer's assists use `ctx.sema` in 89
    // of its 134.
    let sema = ctx.sema();
    let bodies = ctx.file.bodies();
    let type_of = |span: parser::Span| -> String {
        let Some((sema, _, ft)) = sema.as_ref() else { return "_".to_owned() };
        match sema.binding_type_text(*ft, span, bodies) {
            Some(ty) if ty != "unknown" && ty != "error" => ty,
            _ => "_".to_owned(),
        }
    };

    // Build the val spec text
    let param_types = if params.is_empty() {
        "unit".to_string()
    } else {
        let types: Vec<_> = params.iter().map(|(_, span)| type_of(*span)).collect();
        if types.len() == 1 {
            types[0].clone()
        } else {
            format!("({})", types.join(", "))
        }
    };
    let ret_ty = node
        .children()
        .find(|c| !matches!(c.kind(), SK::PARAM_LIST) && usize::from(c.text_range().start()) > 0)
        .and_then(|body| {
            let (sema, _, ft) = sema.as_ref()?;
            let range = body.text_range();
            sema.expr_type_text(
                *ft,
                parser::Span::new(usize::from(range.start()), usize::from(range.end())),
                bodies,
            )
        })
        .filter(|ty| ty != "unknown" && ty != "error")
        .unwrap_or_else(|| "_".to_owned());
    // Whatever inference could not name comes out as `_`, and that is
    // what has to be typed — so for a client that can expand snippets the
    // `_`s are the tabstops, in order.
    let val_text = match ctx.config.snippet_cap {
        Some(_) => {
            let body = format!("{param_types} -> {ret_ty}");
            let mut out = String::with_capacity(body.len());
            for (stop, part) in body.split('_').enumerate() {
                if stop > 0 {
                    out.push_str(&format!("${{{stop}:_}}"));
                }
                out.push_str(part);
            }
            format!("val {name} : {out}\n")
        }
        None => format!("val {name} : {param_types} -> {ret_ty}\n"),
    };

    // Insert before the function definition
    let insert_offset = usize::from(node.text_range().start());
    acc.add(
        AssistId("generate_val_spec", AssistKind::Generate),
        format!("Generate `val {name} : ...`"),
        ctx.range,
        |builder| match ctx.config.snippet_cap {
            Some(_) => builder.insert_snippet(insert_offset, val_text),
            None => builder.insert(insert_offset, val_text),
        },
    );
    Some(())
}

//
// Trigger: cursor on a match expression where the scrutinee is a known
// enum/union type and not all variants are covered.
// Action: insert `VariantName => todo("..."),` for each missing arm.
//
// For sail-riscv: the `extension` enum has 94+ scattered variants.
// This assist collects all variants from workspace ItemTrees.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    /// A function whose body pins the type: the `val` comes out usable.
    #[test]
    fn fills_in_what_inference_knows() {
        check_assist(
            generate_val_spec,
            "function $0answer() = 42\n",
            expect![[r#"
            val answer : unit -> int
            function answer() = 42
        "#]],
        );
    }

    #[test]
    fn applies() {
        check_assist(
            generate_val_spec,
            "function $0add(x, y) = x + y\n",
            expect![[r#"
            val add : (_, _) -> _
            function add(x, y) = x + y
        "#]],
        );
    }

    /// Whatever inference could not name is a `_`, and each is a tabstop.
    #[test]
    fn the_holes_are_tabstops() {
        crate::tests::check_assist_with_snippets(
            generate_val_spec,
            "function f$0oo(x) = x\n",
            expect_test::expect![[r#"
                val foo : ${1:_} -> ${2:_}
                function foo(x) = x
            "#]],
        );
    }
}
