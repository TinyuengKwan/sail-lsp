//! Rejections confirmed by upstream Sail, checked through the configured project path.
fn errors(source: &str) -> Vec<String> {
    let (analysis, files) =
        crate::fixture::project_analysis(&format!("//- prelude\n//- /main.sail\n{source}"));
    analysis
        .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
        .unwrap()
        .into_iter()
        .filter(|d| d.severity == ide_diagnostics::Severity::Error)
        .map(|d| d.message)
        .collect()
}

#[test]
fn attributed_function_checks_its_declared_return() {
    let source = "default Order dec\n$include <prelude.sail>\nval f : int -> bool\nfunction $[foo] f(x) = x\n";
    assert!(
        errors(source).iter().any(|e| e.contains("bool") && e.contains("int")),
        "{:?}",
        errors(source)
    );
    assert!(errors(&source.replace("-> bool", "-> int")).is_empty());
}

#[test]
fn one_pattern_binds_the_whole_tuple_argument() {
    let source = "default Order dec\n$include <prelude.sail>\nval f : (bool, int) -> unit\nfunction f(cond) = if cond then () else ()\n";
    assert!(
        errors(source).iter().any(|e| e.contains("bool") && e.contains("int")),
        "{:?}",
        errors(source)
    );
    assert!(errors(&source.replace("f(cond)", "f(cond, n)")).is_empty());
}

#[test]
fn tuple_binding_does_not_supply_an_implicit_runtime_witness() {
    let source = "default Order dec\n$include <prelude.sail>\nval f : forall ('n : Int). (implicit('n), unit) -> int\nfunction f(n) = 'n\n";
    assert!(!errors(source).is_empty());
    assert!(errors(&source.replace("f(n)", "f(n, ())")).is_empty());
}

#[test]
fn vector_alias_checks_type_constructor_arity() {
    let source = "default Order dec\n$include <prelude.sail>\ntype foo('n : Int) = vector('n)\nfunction f(x : foo(4)) -> unit = ()\n";
    assert!(!errors(source).is_empty());
    assert!(errors(&source.replace("vector('n)", "vector('n, int)")).is_empty());
}

#[test]
fn index_syntax_respects_the_declared_access_function() {
    let source = "default Order dec\nval vector_access : forall 'n. (bits('n), int) -> bool\nval f : bits(8) -> bits(1)\nfunction f(v) = [v[0]]\n";
    assert!(
        errors(source)
            .iter()
            .any(|e| e.contains("bool") && (e.contains("bits") || e.contains("bit"))),
        "{:?}",
        errors(source)
    );
    assert!(errors(&source.replace("-> bool", "-> bit")).is_empty());
}

#[test]
fn numeric_type_arguments_are_not_type_constructor_applications() {
    let source = "default Order dec\n$include <prelude.sail>\ntype bytes : Int = 1\ntype x : Int = bytes * 8\ntype operator /('n : Int, 'm : Int) -> Int = div('n, 'm)\ntype width('n : Int) = bits('n)\nregister r : bits(x / 2) = 0x0\nregister s : width(x / 2) = 0x0\n";
    assert!(errors(source).is_empty(), "{:?}", errors(source));
    let invalid = "default Order dec\n$include <prelude.sail>\ntype wrapped('a : Type) = list('a)\ntype bad = wrapped(vector(4))\n";
    assert!(
        errors(invalid)
            .iter()
            .any(|message| message.contains("vector") && message.contains("arguments")),
        "{:?}",
        errors(invalid)
    );
}

#[test]
fn existential_constructor_preserves_argument_order_constraints() {
    let source = "default Order dec\n\
        type datasize('n : Int) -> Bool = 'n in {32, 64}\n\
        union ast = { Ctor : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))} }\n\
        val f : forall 'd 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n)) -> ast\n\
        function f(d, n) = Ctor(n, d)\n";
    assert!(!errors(source).is_empty(), "swapped constrained indices must be rejected");
    assert!(errors(&source.replace("Ctor(n, d)", "Ctor(d, n)")).is_empty());
}

#[test]
fn existential_constructor_checks_witnesses_opened_by_tuple_binding() {
    let source = "default Order dec\n\
        type datasize('n : Int) -> Bool = 'n in {32, 64}\n\
        union ast = { Ctor : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))} }\n\
        val f : bool -> ast\n\
        function f(b) = {\n\
          let (d, n) : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))} =\n\
            if b then (64, 63) else (32, 31);\n\
          Ctor(n, d)\n\
        }\n";
    assert!(!errors(source).is_empty(), "opened witnesses retain their ordering");
    assert!(errors(&source.replace("Ctor(n, d)", "Ctor(d, n)")).is_empty());
}
