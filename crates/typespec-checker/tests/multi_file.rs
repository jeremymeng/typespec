//! Multi-file programs (M9): cross-file binding, per-file usings and
//! diagnostics located in the file that produced them.

use typespec_checker::{bind, check, resolve_names, CheckResult, NamespaceType, Type};
use typespec_parser::{parse_program, InMemorySourceHost, ProgramParseResult};

struct Program {
    parsed: ProgramParseResult,
    checked: CheckResult,
    diagnostics: Vec<(String, &'static str, String)>,
}

fn check_files(files: &[(&str, &str)]) -> Program {
    let mut host = InMemorySourceHost::new(files);
    let parsed = parse_program("/main.tsp", &mut host);
    let mut table = bind(&parsed.parse_result);
    let resolution = resolve_names(&parsed.parse_result, &mut table);
    let checked = check(&parsed.parse_result, &table, &resolution.resolutions);
    // CheckResult::diagnostics already includes binder and parse diagnostics.
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

fn namespaces<'a>(program: &'a Program, name: &str) -> Vec<&'a NamespaceType> {
    program
        .checked
        .type_arena
        .types()
        .iter()
        .filter_map(|ty| match ty {
            Type::Namespace(ns) if ns.name == name => Some(ns),
            _ => None,
        })
        .collect()
}

fn codes(program: &Program) -> Vec<(&str, &'static str)> {
    program
        .diagnostics
        .iter()
        .map(|(path, code, _)| (path.as_str(), *code))
        .collect()
}

#[test]
fn merges_namespaces_across_files() {
    let program = check_files(&[
        (
            "/main.tsp",
            "import \"./a.tsp\"; namespace N { model A { b: B } }",
        ),
        ("/a.tsp", "namespace N; model B { a: A }"),
    ]);
    assert_eq!(codes(&program), []);
    let n = namespaces(&program, "N");
    assert_eq!(n.len(), 1);
    let mut models = n[0].models.keys().collect::<Vec<_>>();
    models.sort();
    assert_eq!(models, ["A", "B"]);
    assert_eq!(program.parsed.parse_result.scripts.len(), 2);
}

#[test]
fn resolves_global_declarations_from_other_files() {
    let program = check_files(&[
        ("/main.tsp", "import \"./a.tsp\"; model M { b: B }"),
        ("/a.tsp", "import \"./main.tsp\"; model B { m: M }"),
    ]);
    assert_eq!(codes(&program), []);
}

#[test]
fn usings_are_scoped_to_their_file() {
    let program = check_files(&[
        (
            "/main.tsp",
            "import \"./a.tsp\"; using Lib; model M { x: X }",
        ),
        ("/a.tsp", "namespace Lib { model X {} }\nmodel N { x: X }"),
    ]);
    assert_eq!(codes(&program), [("/a.tsp", "invalid-ref")]);
    assert_eq!(program.diagnostics[0].2, "Unknown identifier X");
}

#[test]
fn reports_unused_using_in_imported_file() {
    let program = check_files(&[
        ("/main.tsp", "import \"./a.tsp\"; namespace Lib {}"),
        ("/a.tsp", "using Lib;"),
    ]);
    assert_eq!(
        codes(&program),
        [("/a.tsp", "@typespec/compiler/unused-using")]
    );
}

#[test]
fn using_before_blockless_namespace_resolves_from_global() {
    // Upstream 325c16580.
    let program = check_files(&[
        (
            "/main.tsp",
            "import \"./common.tsp\";\nusing Common.Models;\nnamespace MyOrg.Common.Service;\nmodel M { foo: Foo }",
        ),
        ("/common.tsp", "namespace Common.Models { model Foo {} }"),
    ]);
    assert_eq!(codes(&program), []);
}

#[test]
fn using_after_blockless_namespace_resolves_from_file_namespace() {
    let program = check_files(&[
        (
            "/main.tsp",
            "import \"./sub.tsp\";\nnamespace MyOrg;\nusing Sub;\nmodel M { foo: Foo }",
        ),
        ("/sub.tsp", "namespace MyOrg.Sub { model Foo {} }"),
    ]);
    assert_eq!(codes(&program), []);
}

#[test]
fn global_and_using_bindings_are_ambiguous() {
    let program = check_files(&[
        (
            "/main.tsp",
            "import \"./lib.tsp\"; using Lib; model X {} model M { x: X }",
        ),
        ("/lib.tsp", "namespace Lib { model X {} }"),
    ]);
    // Like TS, an ambiguous match does not mark the using as used.
    assert_eq!(
        codes(&program),
        [
            ("/main.tsp", "ambiguous-symbol"),
            ("/main.tsp", "@typespec/compiler/unused-using")
        ]
    );
}

#[test]
fn reports_duplicate_symbols_in_each_file() {
    let program = check_files(&[
        ("/main.tsp", "import \"./a.tsp\"; model A {}"),
        ("/a.tsp", "model A {}"),
    ]);
    assert_eq!(
        codes(&program),
        [
            ("/main.tsp", "duplicate-symbol"),
            ("/a.tsp", "duplicate-symbol")
        ]
    );
}

#[test]
fn dotted_blockless_namespace_scopes_rest_of_its_file() {
    let program = check_files(&[
        (
            "/main.tsp",
            "import \"./a.tsp\"; model M { x: A.B.X; y: X }",
        ),
        ("/a.tsp", "namespace A.B;\nmodel X { y: Y }\nmodel Y {}"),
    ]);
    assert_eq!(codes(&program), [("/main.tsp", "invalid-ref")]);
    assert_eq!(namespaces(&program, "B")[0].models.len(), 2);
}
