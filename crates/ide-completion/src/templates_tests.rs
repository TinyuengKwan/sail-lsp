use crate::{item::CompletionItem, CompletionConfig, CompletionTemplate, SnippetScope};
use ide_db::{test_utils::TestFile, FileDb};
fn complete(marked: &str, config: &CompletionConfig) -> Vec<CompletionItem> {
    let offset = marked.find("$0").unwrap();
    let source = marked.replacen("$0", "", 1);
    let file = TestFile::new(&source);
    let (db, ft) = file.file_text().unwrap();
    let uri = url::Url::parse("file:///test.sail").unwrap();
    crate::completions_with_config(
        db,
        Some(ft),
        &[(&uri, &file)],
        &uri,
        &file,
        &source,
        offset,
        crate::completion_prefix(&source, offset),
        &[],
        &[],
        config,
    )
}
fn templates(scope: SnippetScope) -> CompletionConfig {
    CompletionConfig {
        custom_templates: vec![CompletionTemplate {
            prefix: Some("mywrap".into()),
            postfix: Some("customwrap".into()),
            body: "wrapper(${receiver}, ${1:value})$0".into(),
            description: "custom wrapper".into(),
            scope,
        }]
        .into(),
        ..Default::default()
    }
}
#[test]
fn custom_prefixes_respect_item_expression_type_and_literal_contexts() {
    for (source, scope) in [
        ("mywr$0", SnippetScope::Item),
        ("function f() = mywr$0", SnippetScope::Expr),
        ("type T = mywr$0", SnippetScope::Type),
    ] {
        let items = complete(source, &templates(scope));
        assert!(items.iter().any(|item| item.label == "mywrap"), "{source}: {items:?}");
        let wrong =
            if scope == SnippetScope::Item { SnippetScope::Expr } else { SnippetScope::Item };
        assert!(!complete(source, &templates(wrong)).iter().any(|item| item.label == "mywrap"));
    }
    for source in ["// mywr$0", "function f() = \"mywr$0\""] {
        assert!(!complete(source, &templates(SnippetScope::Expr))
            .iter()
            .any(|item| item.label == "mywrap"));
    }
}
#[test]
fn custom_postfix_replaces_the_whole_receiver_and_honors_snippet_settings() {
    let source = "function f() = (1 + 2).customwr$0ap";
    let mut config = templates(SnippetScope::Expr);
    let item =
        complete(source, &config).into_iter().find(|item| item.label == ".customwrap").unwrap();
    assert_eq!(item.insert_text.as_deref(), Some("wrapper((1 + 2), ${1:value})$0"));
    let range = item.source_range.unwrap();
    let plain = source.replacen("$0", "", 1);
    assert_eq!(&plain[usize::from(range.start())..usize::from(range.end())], "(1 + 2).customwrap");
    config.enable_snippets = false;
    assert!(!complete(source, &config).iter().any(|item| item.label == ".customwrap"));
}
#[test]
fn expression_search_is_opt_in_fuel_bounded_and_composes_calls() {
    let source = "val as_bool : int -> bool\nfunction as_bool(x) = true\nval identity : int -> int\nfunction identity(x) = x\nfunction f(value : int) -> unit = if as_$0 then () else ()\n";
    let mut config = CompletionConfig::default();
    assert!(!complete(source, &config)
        .iter()
        .any(|item| item.detail.as_deref() == Some("type-directed expression")));
    config.term_search = true;
    let items = complete(source, &config);
    assert!(items.iter().any(|item| item.label == "as_bool(value)"), "{items:?}");
    assert!(items.iter().any(|item| item.label == "as_bool(identity(value))"), "{items:?}");
    config.term_search_fuel = 0;
    assert!(!complete(source, &config)
        .iter()
        .any(|item| item.detail.as_deref() == Some("type-directed expression")));
}
#[test]
fn application_proofs_share_quantifiers_and_reject_failed_or_unknown_constraints() {
    let source = "val same : forall 'a. ('a, 'a) -> bool\nval narrow : forall 'n, 'n > 8. bits('n) -> bool\nval unknown_bound : forall 'n, mystery('n) > 0. bits('n) -> bool\nfunction f(x : int, y : bool, small : bits(4)) -> unit = if true then () else ()\n";
    let file = TestFile::new(source);
    let (db, ft) = file.file_text().unwrap();
    let sema = hir::Semantics::new(db);
    let offset = source.find("if true").unwrap() + 3;
    let bool_ty = hir::Ty::scalar(hir::Scalar::Bool);
    let int_ty = hir::Ty::scalar(hir::Scalar::Int);
    let mut fuel = 100;
    assert!(sema.completion_application_matches(
        ft,
        offset,
        &bool_ty,
        "same",
        &[int_ty.clone(), int_ty.clone()],
        &mut fuel
    ));
    assert!(!sema.completion_application_matches(
        ft,
        offset,
        &bool_ty,
        "same",
        &[int_ty, bool_ty.clone()],
        &mut fuel
    ));
    let locals = sema.locals_in_scope(ft, offset);
    let small = locals.iter().find(|local| local.name.as_str() == "small").unwrap();
    let bits = sema.type_of_pat(ft, small.span.start).unwrap();
    assert!(!sema.completion_parameter_goals(ft, offset, &bool_ty, "narrow", &mut fuel).is_empty());
    assert!(!sema
        .completion_parameter_goals(ft, offset, &bool_ty, "unknown_bound", &mut fuel)
        .is_empty());
    assert!(!sema.completion_application_matches(
        ft,
        offset,
        &bool_ty,
        "narrow",
        std::slice::from_ref(&bits),
        &mut fuel
    ));
    assert!(!sema.completion_application_matches(
        ft,
        offset,
        &bool_ty,
        "unknown_bound",
        &[bits],
        &mut fuel
    ));
    assert!(fuel < 100);
}

#[test]
fn expression_search_offers_only_jointly_proved_applications() {
    let declarations = "val same : forall 'a. ('a, 'a) -> bool\nval narrow : forall 'n, 'n > 8. bits('n) -> bool\nval unknown_bound : forall 'n, mystery('n) > 0. bits('n) -> bool\n";
    let config = CompletionConfig { term_search: true, ..Default::default() };
    let source = format!("{declarations}function caller(x : int, y : bool, small : bits(4)) -> unit = if sa$0 then () else ()\n");
    let items = complete(&source, &config);
    assert!(items.iter().any(|item| item.label == "same(x, x)"), "{items:?}");
    assert!(!items.iter().any(|item| item.label == "same(x, y)"), "{items:?}");
    for prefix in ["nar", "unknown_b"] {
        let source = format!("{declarations}function caller(x : int, y : bool, small : bits(4)) -> unit = if {prefix}$0 then () else ()\n");
        assert!(
            !complete(&source, &config)
                .iter()
                .any(|item| item.detail.as_deref() == Some("type-directed expression")),
            "{source}"
        );
    }
}
