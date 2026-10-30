//! Completion is now in the `ide-completion` crate.
//! This module re-exports everything for backward compatibility.
pub use ide_completion::*;

#[cfg(test)]
mod tests {
    use super::CompletionConfig;

    #[test]
    fn disabling_templates_keeps_plain_keywords() {
        let (analysis, file, position) = crate::fixture::position("//- /main.sail\nfun$0\n");
        let enabled = analysis.completions(file, position, &Default::default()).unwrap();
        assert!(enabled.iter().any(|item| item.label == "function"
            && item.insert_text.as_deref().is_some_and(|text| text.contains('$'))));
        let config = CompletionConfig { enable_snippets: false, ..Default::default() };
        let disabled = analysis.completions(file, position, &config).unwrap();
        let function = disabled.iter().find(|item| item.label == "function").unwrap();
        assert_eq!(function.insert_text.as_deref(), Some("function"));
        assert!(!disabled
            .iter()
            .any(|item| item.kind == ide_db::defs::CompletionItemKind::Snippet));
    }

    #[test]
    fn postfix_setting_controls_provider() {
        let (analysis, file, position) =
            crate::fixture::position("//- /main.sail\nfunction f(x : bool) -> bool = x.i$0\n");
        let enabled = analysis.completions(file, position, &Default::default()).unwrap();
        assert!(enabled.iter().any(|item| item.label == ".if"), "{enabled:?}");
        let item = enabled.iter().find(|item| item.label == ".if").unwrap();
        assert_eq!(item.insert_text.as_deref(), Some("if x then {\n\t$0\n}"));
        let config = CompletionConfig { enable_postfix_completions: false, ..Default::default() };
        let disabled = analysis.completions(file, position, &config).unwrap();
        assert!(!disabled.iter().any(|item| item.label == ".if"));
    }

    fn labels(fixture: &str) -> Vec<String> {
        let (analysis, file, position) = crate::fixture::position(fixture);
        let items = analysis.completions(file, position, &Default::default()).unwrap();
        items.into_iter().map(|item| item.label).collect()
    }

    /// A type position is wherever the tree puts a type, not only right
    /// after `:` or `->`: a later tuple element, a type argument, a
    /// constraint's numeric type. Each offers a type another file declares.
    #[test]
    fn a_type_position_offers_types_from_other_files() {
        for position in [
            "val f : (bits_fflags, bits_r$0) -> unit",
            "val f : bits_fflags -> option(bits_r$0)",
            "val f : forall 'n, 'n <= bits_r$0. int('n) -> unit",
        ] {
            let found = labels(&format!(
                "//- /types.sail\ntype bits_rm = bits(3)\ntype bits_fflags = bits(5)\n\
                 //- /main.sail\n{position}\n"
            ));
            assert!(found.iter().any(|l| l == "bits_rm"), "{position}: {found:?}");
            assert!(!found.iter().any(|l| l == "function"), "{position}: item keywords");
        }
    }

    const B: &str = "bitfield B : bits(8) = { HI : 7 .. 4, LO : 3 .. 0 }\n";

    /// A bitfield's field is named in an index or an update, and both are a
    /// plain identifier to the parser; the base's type says what is wanted.
    #[test]
    fn a_field_slot_offers_the_fields_of_its_base() {
        for body in
            ["function f(v : B) -> bits(4) = v[L$0]", "function f(o : B) -> B = [o with L$0 = 0x0]"]
        {
            let found = labels(&format!("//- /main.sail\n{B}{body}\n"));
            assert_eq!(found.first().map(String::as_str), Some("LO"), "{body}: {found:?}");
        }
    }

    /// Indexing something that is not a record is an expression.
    #[test]
    fn an_index_into_a_vector_is_an_expression() {
        let found = labels(
            "//- /main.sail\nfunction f(xs : vector(4, int), idx : int) -> int = xs[id$0]\n",
        );
        assert!(found.iter().any(|l| l == "idx"), "{found:?}");
    }

    /// The receiver's type, not its text: `w.inner` is no binding the text
    /// fallback can look up, and the type was asked for at the dot itself,
    /// which answered with an inference variable.
    #[test]
    fn a_chained_dot_offers_the_fields_of_the_receivers_type() {
        let found = labels(
            "//- /main.sail\nstruct S = { first : int, second : bool }\nstruct W = { inner : S }\n\
             function f(w : W) -> int = w.inner.fi$0\n",
        );
        assert_eq!(found, ["first"]);
    }
    /// An argument expects its parameter's type: a local of that type
    /// first, then the type's members, then a local of another type.
    #[test]
    fn an_argument_ranks_what_its_parameter_expects() {
        let found = labels(
            "//- /main.sail\nenum extension = { Ext_A, Ext_B }\nval enabled : extension -> bool\n\
             function f(ext_mine : extension, ext_other : int) -> bool = enabled(ext$0)\n",
        );
        assert_eq!(&found[..3], ["ext_mine", "Ext_A", "Ext_B"], "{found:?}");
        assert!(
            found.iter().position(|name| name == "extension_of_num").unwrap()
                < found.iter().position(|name| name == "ext_other").unwrap(),
            "{found:?}"
        );
    }

    /// A body's position is not its function's return type: nothing here
    /// expects `string`, so the function returning one is not preferred.
    #[test]
    fn a_body_does_not_expect_its_functions_return_type() {
        let found = labels(
            "//- /main.sail\nval name_a : int -> string\nval name_b : int -> int\n\
             function f(x : int) -> string = { let y = nam$0; \"\" }\n",
        );
        assert_eq!(found, ["name_a", "name_b"], "alphabetical, no preference");
    }

    /// `bits('n)` expected is not `bits('n)` returned: the two `'n` belong
    /// to different signatures, so a generic library function
    /// (`isla_monomorphize`) does not outrank the local actually passed.
    #[test]
    fn a_generic_expectation_matches_nothing_exactly() {
        let found = labels(
            "//- prelude\n//- /main.sail\ndefault Order dec\n$include <prelude.sail>\n\
             function f(i_val : bits(5)) -> int = unsigned(i$0)\n",
        );
        assert_eq!(found.first().map(String::as_str), Some("i_val"), "{found:?}");
    }
    #[test]
    fn incomplete_callee_prefers_functions_to_non_callable_globals() {
        let found = labels("//- /main.sail\nregister name_a : int\nval name_b : int -> int\nfunction f(x : int) -> int = nam$0(x)\n");
        assert_eq!(found.first().map(String::as_str), Some("name_b"), "{found:?}");
        assert!(found.iter().any(|name| name == "name_a"), "ranking must retain candidates");
    }

    #[test]
    fn prefix_case_breaks_ties_without_hiding_other_cases() {
        let found =
            labels("//- /main.sail\nenum choice = { Name_A, name_b }\nfunction f() = nam$0\n");
        assert_eq!(found.first().map(String::as_str), Some("name_b"), "{found:?}");
        assert!(found.iter().any(|name| name == "Name_A"));
    }

    #[test]
    fn a_local_is_preferred_to_a_global_with_the_same_prefix() {
        let found = labels(
            "//- /main.sail\nregister name_a : int\nfunction f(name_z : int) -> int = nam$0\n",
        );
        assert_eq!(found.first().map(String::as_str), Some("name_z"), "{found:?}");
    }
    #[test]
    fn expected_width_and_aliases_rank_locals_and_real_return_types() {
        for expression in [
            "function f(value_a : bits(64), value_z : word) -> unit = accept(value$0)",
            "val value_a : unit -> bits(64)\nval value_z : unit -> word\nfunction f() -> unit = accept(value$0)",
        ] {
            let found = labels(&format!("//- /main.sail\ndefault Order dec\ntype word = bits(32)\nval accept : bits(32) -> unit\n{expression}\n"));
            assert_eq!(found.first().map(String::as_str), Some("value_z"), "{found:?}");
            assert!(found.iter().any(|label| label == "value_a"));
        }
    }

    #[test]
    fn return_ranking_checks_generic_constraints_and_all_overloads() {
        for declarations in [
            "val make_a : forall 'n, 'n == 64. unit -> bits('n)\nval make_z : forall 'n, 'n == 32. unit -> bits('n)",
            "val make_a : unit -> bits(64)\nval narrow : unit -> bits(32)\nval wide : unit -> bits(64)\noverload make_z = {wide, narrow}",
        ] {
            let found = labels(&format!("//- /main.sail\ndefault Order dec\nval accept : bits(32) -> unit\n{declarations}\nfunction f() -> unit = accept(make$0)\n"));
            assert_eq!(found.first().map(String::as_str), Some("make_z"), "{found:?}");
        }
    }

    #[test]
    fn incomplete_arguments_keep_their_expected_types_in_a_project() {
        for expression in
            ["accept($0)", "accept($0", "pair(0, $0)", "pair(0, $0", "if $0 then true else false"]
        {
            let fixture = format!("//- /main.sail\nval accept : bool -> bool\nval pair : (int, bool) -> bool\nfunction f(value_a : int, value_z : bool) -> bool = {expression}\n");
            let (analysis, file, position) = crate::fixture::project_position(&fixture);
            let items = analysis.completions(file, position, &Default::default()).unwrap();
            let local: Vec<_> =
                items.iter().filter(|item| item.label.starts_with("value_")).collect();
            assert_eq!(
                local.first().map(|item| item.label.as_str()),
                Some("value_z"),
                "{expression}: {local:?}"
            );
            assert!(local[0].relevance.type_match.is_some(), "{expression}: {local:?}");
            assert!(local
                .iter()
                .find(|item| item.label == "value_a")
                .unwrap()
                .relevance
                .type_match
                .is_none());
            assert!(local[0].source_range.unwrap().is_empty());
            assert!(!local[0].insert_text.as_deref().unwrap().contains("sailCompletionMarker"));
        }
    }

    #[test]
    fn incomplete_record_values_and_dot_access_use_the_owner_type() {
        for expression in ["struct { ready = $0 }", "struct { ready = $0"] {
            let fixture = format!("//- /main.sail\nstruct state = {{ ready : bool, count : int }}\nfunction f(value_a : int, value_z : bool) -> state = {expression}\n");
            let (analysis, file, position) = crate::fixture::project_position(&fixture);
            let items = analysis.completions(file, position, &Default::default()).unwrap();
            let local: Vec<_> =
                items.iter().filter(|item| item.label.starts_with("value_")).collect();
            assert_eq!(
                local.first().map(|item| item.label.as_str()),
                Some("value_z"),
                "{expression}: {local:?}"
            );
            assert!(local[0].relevance.type_match.is_some(), "{expression}: {local:?}");
        }
        let (analysis, file, position) = crate::fixture::project_position("//- /main.sail\nstruct state = { ready : bool, count : int }\nfunction f(s : state) -> bool = s.$0\n");
        let items = analysis.completions(file, position, &Default::default()).unwrap();
        assert!(items.iter().any(|item| item.label == "ready"), "{items:?}");
        assert!(items.iter().any(|item| item.label == "count"), "{items:?}");
    }

    #[test]
    fn project_ranking_uses_aliases_and_overload_returns() {
        let fixture = "//- /main.sail\ndefault Order dec\ntype word = bits(32)\nval accept : word -> unit\nval make_a : unit -> bits(64)\nval narrow : unit -> bits(32)\nval wide : unit -> bits(64)\noverload make_z = {wide, narrow}\nfunction f() -> unit = accept(make$0)\n";
        let (analysis, file, position) = crate::fixture::project_position(fixture);
        let items = analysis.completions(file, position, &Default::default()).unwrap();
        assert_eq!(items.first().map(|item| item.label.as_str()), Some("make_z"), "{items:?}");
        assert!(items[0].relevance.type_match.is_some());
        assert!(items
            .iter()
            .find(|item| item.label == "make_a")
            .unwrap()
            .relevance
            .type_match
            .is_none());
    }
}
