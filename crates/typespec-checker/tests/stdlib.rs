//! Standard library loading (M10): programs checked with the real
//! `@typespec/compiler` library files (`lib/intrinsics.tsp`, `lib/std/*.tsp`).

use std::path::Path;

use typespec_checker::{bind, check, resolve_names, CheckResult, Type, TypeId};
use typespec_parser::{
    parse_program_with_stdlib, InMemorySourceHost, ParseOptions, ProgramParseResult,
};

const COMPILER_LIB: &str = "/compiler/lib";

struct Program {
    parsed: ProgramParseResult,
    checked: CheckResult,
    diagnostics: Vec<(String, &'static str, String)>,
}

fn stdlib_files() -> Vec<(String, String)> {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/compiler/lib");
    let mut files = Vec::new();
    let mut stack = vec![lib.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("compiler lib directory") {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "tsp") {
                let relative = path.strip_prefix(&lib).unwrap().to_string_lossy();
                files.push((
                    format!("{COMPILER_LIB}/{}", relative.replace('\\', "/")),
                    std::fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }
    // JS imports of the library files; JS files are recorded but not read.
    for js in [
        "/compiler/dist/src/lib/intrinsic/tsp-index.js",
        "/compiler/dist/src/lib/tsp-index.js",
    ] {
        files.push((js.to_string(), String::new()));
    }
    files
}

fn check_with_stdlib(files: &[(&str, &str)]) -> Program {
    let stdlib = stdlib_files();
    let mut all = stdlib
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect::<Vec<_>>();
    all.extend_from_slice(files);
    let mut host = InMemorySourceHost::new(&all);
    let parsed = parse_program_with_stdlib(
        "/main.tsp",
        &mut host,
        ParseOptions::default(),
        &[
            format!("{COMPILER_LIB}/intrinsics.tsp"),
            format!("{COMPILER_LIB}/std/main.tsp"),
        ],
    );
    let mut table = bind(&parsed.parse_result);
    let resolution = resolve_names(&parsed.parse_result, &mut table);
    let checked = check(&parsed.parse_result, &table, &resolution.resolutions);
    let diagnostics = resolution
        .diagnostics
        .iter()
        .chain(checked.diagnostics.iter())
        .map(|diagnostic| {
            let path = match parsed.sources.locate_span(diagnostic.target) {
                Some((file, _)) => parsed.sources.get(file).path.clone(),
                None => String::new(),
            };
            (path, diagnostic.code, diagnostic.message.clone())
        })
        .collect();
    Program {
        parsed,
        checked,
        diagnostics,
    }
}

fn codes(program: &Program) -> Vec<(&str, &'static str, &str)> {
    program
        .diagnostics
        .iter()
        .map(|(path, code, message)| (path.as_str(), *code, message.as_str()))
        .collect()
}

fn find_model(program: &Program, name: &str) -> TypeId {
    program
        .checked
        .type_arena
        .types()
        .iter()
        .position(|ty| matches!(ty, Type::Model(model) if model.name == name))
        .map(|index| TypeId::new(index as u32))
        .unwrap_or_else(|| panic!("model {name}"))
}

fn property_type(program: &Program, model: &str, property: &str) -> TypeId {
    let Type::Model(model) = program.checked.type_arena.get(find_model(program, model)) else {
        unreachable!()
    };
    let Type::ModelProperty(prop) = program.checked.type_arena.get(model.properties[property])
    else {
        unreachable!()
    };
    prop.property_type
}

fn namespace_name(program: &Program, id: Option<TypeId>) -> Option<String> {
    match program.checked.type_arena.get(id?) {
        Type::Namespace(ns) => Some(ns.name.clone()),
        _ => None,
    }
}

#[test]
fn stdlib_loads_without_diagnostics() {
    let program = check_with_stdlib(&[("/main.tsp", "model M { name: string }")]);
    assert_eq!(codes(&program), []);
    assert_eq!(program.parsed.parse_result.compiler_scripts.len(), 7);
}

#[test]
fn intrinsic_scalars_are_declared_in_typespec_namespace() {
    let program = check_with_stdlib(&[("/main.tsp", "model M { name: string; id: int32 }")]);
    assert_eq!(codes(&program), []);
    let string = property_type(&program, "M", "name");
    assert!(matches!(
        program.checked.type_arena.get(string),
        Type::String
    ));
    let declaration = program
        .checked
        .intrinsic_declarations
        .get(&string)
        .expect("string declaration");
    assert_eq!(
        namespace_name(&program, declaration.namespace).as_deref(),
        Some("TypeSpec")
    );
    let int32 = property_type(&program, "M", "id");
    let int32_decl = &program.checked.intrinsic_declarations[&int32];
    let int64 = int32_decl.base_scalar.expect("int32 extends int64");
    assert!(matches!(program.checked.type_arena.get(int64), Type::Int64));
}

#[test]
fn array_expression_instantiates_typespec_array() {
    let program = check_with_stdlib(&[("/main.tsp", "model M { tags: string[] }")]);
    assert_eq!(codes(&program), []);
    let tags = property_type(&program, "M", "tags");
    let Type::Model(array) = program.checked.type_arena.get(tags) else {
        panic!("array model");
    };
    assert_eq!(array.name, "Array");
    assert_eq!(
        namespace_name(&program, array.namespace).as_deref(),
        Some("TypeSpec")
    );
    let indexer = array.indexer.expect("indexer");
    assert!(matches!(
        program.checked.type_arena.get(indexer.key),
        Type::Integer
    ));
    assert!(matches!(
        program.checked.type_arena.get(indexer.value),
        Type::String
    ));
}

#[test]
fn record_has_string_indexer() {
    let program = check_with_stdlib(&[("/main.tsp", "model M { map: Record<int32> }")]);
    assert_eq!(codes(&program), []);
    let map = property_type(&program, "M", "map");
    let Type::Model(record) = program.checked.type_arena.get(map) else {
        panic!("record model");
    };
    assert_eq!(record.name, "Record");
    let indexer = record.indexer.expect("indexer");
    assert!(matches!(
        program.checked.type_arena.get(indexer.key),
        Type::String
    ));
    assert!(matches!(
        program.checked.type_arena.get(indexer.value),
        Type::Int32
    ));
}

#[test]
fn record_spread_into_array_model_reports_no_array_properties() {
    let program = check_with_stdlib(&[("/main.tsp", "model A is string[] { ...Record<string> }")]);
    assert_eq!(
        codes(&program),
        [(
            "/main.tsp",
            "no-array-properties",
            "Array models cannot have any properties."
        )]
    );
}

#[test]
fn property_incompatible_with_record_indexer() {
    let program = check_with_stdlib(&[("/main.tsp", "model A is Record<string> { x: int32 }")]);
    assert_eq!(
        codes(&program),
        [(
            "/main.tsp",
            "incompatible-indexer",
            "Property is incompatible with indexer:\n  Type 'int32' is not assignable to type 'string'"
        )]
    );
}

#[test]
fn global_declaration_is_ambiguous_with_typespec_member() {
    let program = check_with_stdlib(&[("/main.tsp", "model string {} model A { x: string }")]);
    assert_eq!(
        codes(&program),
        [(
            "/main.tsp",
            "ambiguous-symbol",
            "\"string\" is an ambiguous name between global.string, TypeSpec.string. Try using fully qualified name instead: global.string, TypeSpec.string"
        )]
    );
}

#[test]
fn reflection_models_constrain_templates() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model Box<T extends Reflection.Model> { t: T }\nmodel Inner {}\nmodel M { ok: Box<Inner>; bad: Box<string> }",
    )]);
    assert_eq!(
        codes(&program),
        [(
            "/main.tsp",
            "invalid-argument",
            "Argument of type 'string' is not assignable to parameter of type 'Model'"
        )]
    );
}

#[test]
fn explicit_using_typespec_is_ambiguous_with_implicit_using() {
    let program = check_with_stdlib(&[("/main.tsp", "using TypeSpec; model A { x: string }")]);
    assert_eq!(
        codes(&program),
        [(
            "/main.tsp",
            "ambiguous-symbol",
            "\"string\" is an ambiguous name between TypeSpec.string, TypeSpec.string. Try using fully qualified name instead: TypeSpec.string, TypeSpec.string"
        )]
    );
}

#[test]
fn spread_property_incompatible_with_indexer() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model A is Record<string> { ...B } model B { x: int32 }",
    )]);
    assert_eq!(
        codes(&program),
        [(
            "/main.tsp",
            "incompatible-indexer",
            "Property is incompatible with indexer:\n  Type 'int32' is not assignable to type 'string'"
        )]
    );
}

#[test]
fn spread_records_merge_indexers() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model A { ...Record<string>; ...Record<int32>; x: int32 }",
    )]);
    assert_eq!(codes(&program), []);
    let Type::Model(model) = program.checked.type_arena.get(find_model(&program, "A")) else {
        unreachable!()
    };
    let indexer = model.indexer.expect("indexer");
    assert!(matches!(
        program.checked.type_arena.get(indexer.key),
        Type::String
    ));
    let Type::Union(union) = program.checked.type_arena.get(indexer.value) else {
        panic!("union indexer value");
    };
    assert_eq!(union.variants.len(), 2);
}

#[test]
fn template_parameter_spread_is_applied_on_instantiation() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model Foo<T> { ...T; b: int32 } model X { a: string } model M { f: Foo<X> }",
    )]);
    assert_eq!(codes(&program), []);
    let foo_x = property_type(&program, "M", "f");
    let Type::Model(model) = program.checked.type_arena.get(foo_x) else {
        panic!("model instance");
    };
    assert_eq!(
        model
            .properties
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn decorator_argument_count_is_validated_per_checked_type() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "@doc model A<T> { x: T } model B { a: A<string>; b: A<int32> } @@doc(B); \
         extern dec foo(target: unknown, ...a: string[]); @foo model C {} \
         extern dec baz(target: unknown, a: string, ...b: string[]); @baz model D {} \
         extern dec bar(target: unknown, a?: string); @bar(\"a\", \"b\") op o(): void;",
    )]);
    let expected_doc = (
        "/main.tsp",
        "invalid-argument-count",
        "Expected 1-2 arguments, but got 0.",
    );
    assert_eq!(
        codes(&program),
        [
            expected_doc,
            expected_doc,
            expected_doc,
            expected_doc,
            (
                "/main.tsp",
                "invalid-argument-count",
                "Expected at least 1 arguments, but got 0."
            ),
            (
                "/main.tsp",
                "invalid-argument-count",
                "Expected 0-1 arguments, but got 2."
            ),
        ]
    );
}

#[test]
fn nested_array_instantiates_array_of_array() {
    let program = check_with_stdlib(&[("/main.tsp", "model A { x: string[][] }")]);
    assert_eq!(codes(&program), []);
    let outer = property_type(&program, "A", "x");
    let Type::Model(outer) = program.checked.type_arena.get(outer) else {
        panic!("outer array");
    };
    let inner = outer.indexer.expect("outer indexer").value;
    let Type::Model(inner) = program.checked.type_arena.get(inner) else {
        panic!(
            "inner array, got {:?}",
            program.checked.type_arena.get(inner)
        );
    };
    assert_eq!(inner.name, "Array");
}

#[test]
fn ambiguous_reference_is_error_type() {
    let program = check_with_stdlib(&[("/main.tsp", "model string {} model A { x: string }")]);
    assert!(matches!(
        program
            .checked
            .type_arena
            .get(property_type(&program, "A", "x")),
        Type::Error
    ));
}

#[test]
fn named_template_argument_name_is_not_a_reference() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model Foo<T> { value: T } model X { y: Foo<T = boolean> }",
    )]);
    assert_eq!(codes(&program), []);
}

#[test]
fn record_constraint_requires_index_on_named_models() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model Foo { n: int32 }
        model Bar is Record<int32> { n: int32 }
        model W<T extends Record<int32>> { t: T }
        model X { a: W<Foo>; b: W<Bar>; c: W<{ n: int32 }>; d: W<{ n: string }>; e: W<int32[]> }",
    )]);
    let found = codes(&program)
        .into_iter()
        .map(|(_, code, message)| (code, message))
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found[0].1.contains("Foo"), "{found:?}");
    assert!(found[1].1.contains("'{ n: string }'"), "{found:?}");
    assert!(found[2].1.contains("'int32[]'"), "{found:?}");
}

#[test]
fn tuples_relate_to_arrays_and_tuples() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model W<T extends string[]> { t: T }
        model Y<T extends [string, int32]> { t: T }
        model X { pair: [string, int32]; a: W<[\"a\", \"b\"]>; b: W<[1]>; c: Y<[string]>; d: Y<[\"x\", 1]> }",
    )]);
    let Type::Tuple(tuple) = program
        .checked
        .type_arena
        .get(property_type(&program, "X", "pair"))
    else {
        panic!("expected tuple");
    };
    assert_eq!(tuple.values.len(), 2);
    let found = codes(&program)
        .into_iter()
        .map(|(_, code, message)| (code, message))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            (
                "invalid-argument",
                "Argument of type '[1]' is not assignable to parameter of type 'string[]'"
            ),
            (
                "invalid-argument",
                "Argument of type '[string]' is not assignable to parameter of type '[string, int32]'"
            ),
        ]
    );
}

#[test]
fn string_templates_serialize_literal_spans() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "alias P = \"pre\";
        model T<A extends string> { s: \"${A}-${P}\" }
        model X { a: T<\"a\">; b: \"${P}-${int32}\"; c: \"${P}-fixed\" }
        @doc(\"${int32}\") model D {}",
    )]);
    let string_value = |model: &str, property: &str| match program
        .checked
        .type_arena
        .get(property_type(&program, model, property))
    {
        Type::StringTemplate(template) => template.string_value.clone(),
        other => panic!("expected string template, got {}", other.kind_name()),
    };
    assert_eq!(string_value("X", "b"), None);
    assert_eq!(string_value("X", "c"), Some("pre-fixed".to_string()));
    let Type::Model(x) = program.checked.type_arena.get(find_model(&program, "X")) else {
        unreachable!()
    };
    let Type::ModelProperty(a) = program.checked.type_arena.get(x.properties["a"]) else {
        unreachable!()
    };
    let Type::Model(instance) = program.checked.type_arena.get(a.property_type) else {
        panic!("expected template instance");
    };
    let Type::ModelProperty(s) = program.checked.type_arena.get(instance.properties["s"]) else {
        unreachable!()
    };
    assert!(matches!(
        program.checked.type_arena.get(s.property_type),
        Type::StringTemplate(template) if template.string_value.as_deref() == Some("a-pre")
    ));
    let found = codes(&program)
        .into_iter()
        .map(|(_, code, _)| code)
        .collect::<Vec<_>>();
    assert_eq!(found, vec!["non-literal-string-template"]);
}

#[test]
fn property_defaults_are_collected_as_values() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model X { a: string = \"x\"; b: int32[] = #[1, 2]; c: { n: int32 } = #{ n: 1 }; d?: int32 }",
    )]);
    assert!(codes(&program).is_empty(), "{:?}", codes(&program));
    let Type::Model(x) = program.checked.type_arena.get(find_model(&program, "X")) else {
        unreachable!()
    };
    let default_of = |name: &str| {
        program
            .checked
            .default_values
            .iter()
            .find(|(id, _)| *id == x.properties[name])
            .map(|(_, value)| format!("{value:?}"))
    };
    assert!(default_of("a").unwrap().contains("\"x\""));
    assert!(default_of("b").unwrap().starts_with("Array"));
    assert!(default_of("c").unwrap().starts_with("Object"));
    assert_eq!(default_of("d"), None);
}
#[test]
fn object_and_array_values_are_checked_against_their_type() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "model P { x: int32; y?: string }
        const c = #{ x: 1 };
        const bad: P = #{ x: \"s\" };
        model X {
          a?: int32[] = #[1, \"b\"];
          b?: P = #{ x: \"s\" };
          c?: P = #{ x: 1, z: 2 };
          d?: P = #{ y: \"a\" };
          h?: int32 = #[1];
          r?: Record<int32> = #{ a: \"x\" };
          u?: string | int32 = #[1];
          s?: P = #{ ...c, y: \"ok\" };
          ok?: P = c;
        }",
    )]);
    let found = codes(&program)
        .into_iter()
        .map(|(_, code, message)| (code, message))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![
            ("unassignable", "Type '\"s\"' is not assignable to type 'int32'"),
            ("unassignable", "Type '\"b\"' is not assignable to type 'int32'"),
            ("unassignable", "Type '\"s\"' is not assignable to type 'int32'"),
            (
                "unexpected-property",
                "Object value may only specify known properties, and 'z' does not exist in type 'P'."
            ),
            (
                "missing-property",
                "Property 'x' is missing on type '{ y: \"a\" }' but required in 'P'"
            ),
            ("unassignable", "Type '[1]' is not assignable to type 'int32'"),
            ("unassignable", "Type '\"x\"' is not assignable to type 'int32'"),
            (
                "unassignable",
                "Type '[1]' is not assignable to type 'string | int32'"
            ),
        ]
    );
}

#[test]
fn decorator_value_arguments_are_checked_against_value_constraints() {
    let program = check_with_stdlib(&[(
        "/main.tsp",
        "@service(#{ title: 1 }) namespace A {}
        @service(#{ title: \"ok\" }) namespace B {}",
    )]);
    let found = codes(&program)
        .into_iter()
        .map(|(_, code, message)| (code, message))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![(
            "invalid-argument",
            "Argument of type '{ title: 1 }' is not assignable to parameter of type 'ServiceOptions'"
        )]
    );
}
