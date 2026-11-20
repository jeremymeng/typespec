//! Decorator validation (M11).

use std::path::Path;

use typespec_checker::{
    bind, check_with_options, resolve_names, CheckOptions, CheckResult, DecoratorApplicationKind,
    DecoratorValueEntity, ResolvedArgumentValue, Type, TypeId,
};
use typespec_parser::{
    parse_program_with_stdlib, InMemorySourceHost, ParseOptions, ProgramParseResult,
};

const COMPILER_LIB: &str = "/compiler/lib";

struct Program {
    #[allow(dead_code)]
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
    check_with(files, CheckOptions::default())
}

fn check_with(files: &[(&str, &str)], options: CheckOptions) -> Program {
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
    let checked = check_with_options(
        &parsed.parse_result,
        &table,
        &resolution.resolutions,
        options,
    );
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

fn main_codes(program: &Program) -> Vec<(&'static str, &str)> {
    codes(program)
        .into_iter()
        .filter(|(path, _, _)| *path == "/main.tsp")
        .map(|(_, code, message)| (code, message))
        .collect()
}

fn check_main(source: &str) -> Program {
    check_with_stdlib(&[("/main.tsp", source)])
}

#[test]
fn stdlib_has_no_decorator_diagnostics() {
    let program = check_main("model M {}");
    assert_eq!(codes(&program), []);
}

#[test]
fn reports_decorator_wrong_target() {
    let program = check_main("extern dec foo(target: Reflection.Model); @foo op o(): void;");
    assert_eq!(
        main_codes(&program),
        [(
            "decorator-wrong-target",
            "Cannot apply @foo decorator to o since it is not assignable to Model"
        )]
    );
}

#[test]
fn reports_invalid_argument_for_value_parameter() {
    let program = check_main("@doc(1) model M {}");
    assert_eq!(
        main_codes(&program),
        [(
            "invalid-argument",
            "Argument of type '1' is not assignable to parameter of type 'valueof string'"
        )]
    );
}

#[test]
fn accepts_matching_type_and_value_arguments() {
    let program = check_main(
        "enum E { a } extern dec foo(target: unknown, t: Reflection.Model, v: valueof string | int32, e: valueof E, ...rest: valueof int32[]);\n\
         model X {} @foo(X, \"s\", E.a, 1, 2) @foo(X, X, E.a) model M {}",
    );
    assert_eq!(
        main_codes(&program),
        [(
            "invalid-argument",
            "Argument of type 'X' is not assignable to parameter of type 'valueof string | int32'"
        )]
    );
}

#[test]
fn validates_decorator_arguments_per_template_instance() {
    let program =
        check_main("model Foo<T> { @doc(1) x: T } model B { a: Foo<string>; b: Foo<int32> }");
    let message = (
        "invalid-argument",
        "Argument of type '1' is not assignable to parameter of type 'valueof string'",
    );
    assert_eq!(main_codes(&program), [message, message, message]);
}

#[test]
fn unconstrained_template_parameter_is_not_assignable_to_value() {
    let program = check_main(
        "@summary(T) model Foo<T> {} @summary(U) model Bar<U extends valueof string> {}",
    );
    assert_eq!(
        main_codes(&program),
        [(
            "invalid-argument",
            "Argument of type 'T' is not assignable to parameter of type 'valueof string'"
        )]
    );
}

#[test]
fn reports_expect_value_for_model_expression() {
    let program =
        check_main("extern dec foo(target: unknown, v: valueof unknown); @foo({}) model M {}");
    assert_eq!(
        main_codes(&program),
        [(
            "expect-value",
            "Is a model expression type, but is being used as a value here. Use #{} to create an object value."
        )]
    );
}

#[test]
fn reports_invalid_decorator_for_non_decorator_symbol() {
    let program = check_main("model Foo {} @Foo model M {}");
    assert!(main_codes(&program)
        .iter()
        .any(|(code, _)| *code == "invalid-decorator"));
}

#[test]
fn reports_invalid_modifiers() {
    let program = check_main(
        "internal namespace N {} extern model M {} dec foo(target: unknown); extern auto dec bar(target: unknown); internal fn f(): void;",
    );
    assert_eq!(
        main_codes(&program),
        [
            (
                "invalid-modifier",
                "Modifier 'internal' cannot be used on declarations of type 'namespace'."
            ),
            (
                "invalid-modifier",
                "Modifier 'extern' cannot be used on declarations of type 'model'."
            ),
            (
                "invalid-modifier",
                "Declaration of type 'dec' is missing one of the required modifiers: 'extern' or 'auto'."
            ),
            (
                "invalid-modifier",
                "Modifiers 'extern' and 'auto' cannot be used together."
            ),
            (
                "invalid-modifier",
                "Declaration of type 'function' is missing required modifier 'extern'."
            ),
            (
                "auto-decorator-disabled",
                "Auto decorator declarations require the 'auto-decorators' feature to be enabled. Add 'auto-decorators' to the 'features' list in your tspconfig.yaml."
            ),
        ]
    );
}

#[test]
fn auto_decorators_feature_allows_auto_declarations() {
    let program = check_with(
        &[(
            "/main.tsp",
            "auto dec tag(target: unknown, name: valueof string);",
        )],
        CheckOptions {
            auto_decorators: true,
            ..CheckOptions::default()
        },
    );
    assert_eq!(main_codes(&program), []);
    let tag = program
        .checked
        .extern_declarations
        .iter()
        .find(|declaration| declaration.fqn == "@tag")
        .expect("auto decorator declaration");
    assert_eq!(tag.parameter_names, ["name"]);
}

#[test]
fn reports_invalid_augment_targets() {
    let program = check_main(
        "model Foo<T> { t: T } model Bar {} alias A = Foo<string>; alias U = string | int32; alias ME = { x: string };\n\
         @@doc(Foo<string>, \"a\"); @@doc(A, \"b\"); @@doc(U, \"c\"); @@doc(ME, \"d\"); @@doc(Bar, \"ok\");",
    );
    assert_eq!(
        main_codes(&program),
        [
            (
                "augment-decorator-target",
                "Cannot reference template instances."
            ),
            (
                "augment-decorator-target",
                "Cannot reference template instances."
            ),
            (
                "augment-decorator-target",
                "Cannot augment union expressions."
            ),
            (
                "augment-decorator-target",
                "Cannot augment model expressions."
            ),
        ]
    );
}

#[test]
fn collects_applications_in_typescript_execution_order() {
    let program = check_with(
        &[(
            "/main.tsp",
            "/** comment */ @summary(\"s\") @doc(\"d\") model M {} @@doc(M, \"aug\");\n\
             /**\n * Op\n * @returns ret\n * @errors err\n */ op o(): void;",
        )],
        CheckOptions {
            collect_decorator_applications: true,
            ..CheckOptions::default()
        },
    );
    assert_eq!(main_codes(&program), []);
    let model = find_model(&program, "M");
    let applications = program
        .checked
        .decorator_applications
        .iter()
        .filter(|application| application.target == model)
        .map(|application| (application.decorator.as_str(), application.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        applications,
        [
            (
                "TypeSpec.@docFromComment",
                DecoratorApplicationKind::DocComment
            ),
            ("TypeSpec.@doc", DecoratorApplicationKind::Decorator),
            ("TypeSpec.@summary", DecoratorApplicationKind::Decorator),
            ("TypeSpec.@doc", DecoratorApplicationKind::Augment),
        ]
    );
    let doc_args = |key: &str| {
        program
            .checked
            .decorator_applications
            .iter()
            .filter(|application| {
                application.kind == DecoratorApplicationKind::DocComment
                    && matches!(
                        program.checked.type_arena.get(application.target),
                        Type::Model(model) if model.name == "M"
                    )
                    || matches!(
                        program.checked.type_arena.get(application.target),
                        Type::Operation(operation) if operation.name == "o"
                    )
            })
            .find_map(|application| {
                let [first, second] = application.args.as_slice() else {
                    return None;
                };
                match (&first.value, &second.value) {
                    (
                        ResolvedArgumentValue::Value(DecoratorValueEntity::String {
                            value: k, ..
                        }),
                        ResolvedArgumentValue::Value(DecoratorValueEntity::String {
                            value, ..
                        }),
                    ) if k == key => Some(value.clone()),
                    _ => None,
                }
            })
    };
    assert_eq!(doc_args("returns").as_deref(), Some("ret"));
    assert_eq!(doc_args("errors").as_deref(), Some("err"));
    assert_eq!(doc_args("self").as_deref(), Some("comment"));
    let summary = program
        .checked
        .decorator_applications
        .iter()
        .find(|application| application.decorator == "TypeSpec.@summary")
        .unwrap();
    assert!(matches!(
        &summary.args[0].value,
        ResolvedArgumentValue::Value(DecoratorValueEntity::String { value, .. }) if value == "s"
    ));
}

#[test]
fn template_declarations_are_not_collected() {
    let program = check_with(
        &[(
            "/main.tsp",
            "@doc(\"x\") model Foo<T> { @doc(\"p\") p: T } model B { a: Foo<string> }",
        )],
        CheckOptions {
            collect_decorator_applications: true,
            ..CheckOptions::default()
        },
    );
    let foo_count = program
        .checked
        .decorator_applications
        .iter()
        .filter(|application| {
            matches!(
                program.checked.type_arena.get(application.target),
                Type::Model(model) if model.name == "Foo"
            )
        })
        .count();
    assert_eq!(foo_count, 1);
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
