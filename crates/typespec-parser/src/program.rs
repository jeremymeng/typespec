//! Multi-file source loading (port of `packages/compiler/src/core/source-loader.ts`).
//!
//! Starting from an entry file, files are loaded depth-first in import order,
//! mirroring the TS `SourceLoader`. All files are parsed into one shared
//! [`AstArena`] so `NodeId`s are unique program-wide, and each file's node
//! positions are shifted into the program position space managed by
//! [`ProgramSources`]. Import cycles are allowed: a file is only loaded once.

use std::collections::{HashMap, HashSet};

use typespec_source::path::{get_any_extension_from_path, get_directory_path, resolve_path};
use typespec_source::{
    Diagnostic, DiagnosticSeverity, ProgramSources, SourceFile, SourceFileId, SourceFileKind, Span,
};

use crate::ast::{AstArena, NodeData, NodeId};
use crate::{ParseOptions, ParseResult, Parser};

/// Successful module resolution: the final file to load. Directory and package
/// entrypoints (`main.tsp`, `tspMain`, `exports`) are already resolved by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModule {
    pub path: String,
    /// Resolved through a package manifest (TS `type: "module"`). Files loaded
    /// from it, and their relative imports, are in a library location context.
    pub library: bool,
}

/// Module resolution failure, mirroring TS `ResolveModuleError` codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleResolutionError {
    /// `MODULE_NOT_FOUND` → `import-not-found`.
    NotFound { manifest_path: Option<String> },
    /// `INVALID_MODULE`/`INVALID_MAIN`/`INVALID_MODULE_EXPORT_TARGET` → `library-invalid`.
    InvalidLibrary {
        message: String,
        manifest_path: Option<String>,
    },
}

/// File-system access needed by [`parse_program`]. Implemented by the JS host
/// through napi callbacks and by in-memory hosts in tests.
pub trait SourceHost {
    /// Read a UTF-8 file. `None` when the file does not exist.
    fn read_file(&mut self, path: &str) -> Option<String>;

    /// Resolve an import `specifier` from `base_dir` (the importing file's directory).
    fn resolve_module(
        &mut self,
        specifier: &str,
        base_dir: &str,
    ) -> Result<ResolvedModule, ModuleResolutionError>;
}

/// Import edges between loaded files. Cycles are permitted, as in TS.
#[derive(Debug, Clone, Default)]
pub struct ImportGraph {
    edges: Vec<(SourceFileId, SourceFileId)>,
    files: HashSet<SourceFileId>,
}

impl ImportGraph {
    pub fn add_file(&mut self, file: SourceFileId) {
        self.files.insert(file);
    }

    /// Record that `from` imports `to`. Duplicate edges are ignored.
    pub fn add(&mut self, from: SourceFileId, to: SourceFileId) {
        self.files.insert(from);
        self.files.insert(to);
        if !self.edges.contains(&(from, to)) {
            self.edges.push((from, to));
        }
    }

    pub fn edges(&self) -> &[(SourceFileId, SourceFileId)] {
        &self.edges
    }

    pub fn contains(&self, file: SourceFileId) -> bool {
        self.files.contains(&file)
    }

    pub fn imports_of(&self, file: SourceFileId) -> impl Iterator<Item = SourceFileId> + '_ {
        self.edges
            .iter()
            .filter(move |(from, _)| *from == file)
            .map(|(_, to)| *to)
    }

    /// Whether the import graph contains a cycle (informational only).
    pub fn has_cycle(&self) -> bool {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Visiting,
            Done,
        }
        fn visit(
            graph: &ImportGraph,
            file: SourceFileId,
            marks: &mut HashMap<SourceFileId, Mark>,
        ) -> bool {
            match marks.get(&file) {
                Some(Mark::Visiting) => return true,
                Some(Mark::Done) => return false,
                None => {}
            }
            marks.insert(file, Mark::Visiting);
            for next in graph.imports_of(file).collect::<Vec<_>>() {
                if visit(graph, next, marks) {
                    return true;
                }
            }
            marks.insert(file, Mark::Done);
            false
        }
        let mut marks = HashMap::new();
        let mut files = self.files.iter().copied().collect::<Vec<_>>();
        files.sort();
        files.into_iter().any(|file| visit(self, file, &mut marks))
    }
}

#[derive(Debug)]
pub struct ProgramParseResult {
    /// Combined parse result: one arena, every file's script in `scripts`
    /// (load order), and parse + import diagnostics in program positions.
    pub parse_result: ParseResult,
    pub sources: ProgramSources,
    pub import_graph: ImportGraph,
}

/// Load and parse a program rooted at `entry_path`.
pub fn parse_program(entry_path: &str, host: &mut dyn SourceHost) -> ProgramParseResult {
    parse_program_with_options(entry_path, host, ParseOptions::default())
}

pub fn parse_program_with_options(
    entry_path: &str,
    host: &mut dyn SourceHost,
    options: ParseOptions,
) -> ProgramParseResult {
    parse_program_with_stdlib(entry_path, host, options, &[])
}

/// Load and parse a program, first loading the standard library files in
/// `stdlib` (in order) in the compiler location context, like TS
/// `loadIntrinsicTypes` + `loadStandardLibrary` (`lib/intrinsics.tsp`, then
/// `lib/std/main.tsp`). An empty `stdlib` loads only the project.
pub fn parse_program_with_stdlib(
    entry_path: &str,
    host: &mut dyn SourceHost,
    options: ParseOptions,
    stdlib: &[String],
) -> ProgramParseResult {
    let mut loader = SourceLoader {
        host,
        options,
        arena: AstArena::new(),
        scripts: Vec::new(),
        library_scripts: Vec::new(),
        compiler_scripts: Vec::new(),
        loading_compiler: false,
        script_files: HashMap::new(),
        diagnostics: Vec::new(),
        comments: Vec::new(),
        sources: ProgramSources::new(),
        import_graph: ImportGraph::default(),
        seen: HashSet::new(),
    };
    loader.loading_compiler = true;
    for path in stdlib {
        let path = resolve_path(path, &[]);
        loader.import_file(&path, Span::NO_TARGET, None, ImportKind::Import, true);
    }
    loader.loading_compiler = false;
    let entry = resolve_path(entry_path, &[]);
    loader.import_file(&entry, Span::NO_TARGET, None, ImportKind::Entrypoint, false);
    let mut entry_root = None;
    if let Some(entry) = loader.sources.get_by_path(&entry).map(|file| file.id) {
        loader.sources.set_entry(entry);
        entry_root = loader
            .script_files
            .iter()
            .find(|(_, file)| **file == entry)
            .map(|(root, _)| *root);
    }
    loader.finish(entry_root)
}

#[derive(Clone, Copy, PartialEq)]
enum ImportKind {
    Import,
    Entrypoint,
}

struct SourceLoader<'h> {
    host: &'h mut dyn SourceHost,
    options: ParseOptions,
    arena: AstArena,
    scripts: Vec<NodeId>,
    library_scripts: Vec<NodeId>,
    compiler_scripts: Vec<NodeId>,
    /// Set while loading the standard library: every file reached from it is
    /// in the compiler location context.
    loading_compiler: bool,
    script_files: HashMap<NodeId, SourceFileId>,
    diagnostics: Vec<Diagnostic>,
    comments: Vec<NodeId>,
    sources: ProgramSources,
    import_graph: ImportGraph,
    seen: HashSet<String>,
}

impl SourceLoader<'_> {
    fn finish(mut self, entry_root: Option<NodeId>) -> ProgramParseResult {
        let root = match entry_root.or_else(|| self.scripts.first().copied()) {
            Some(root) => root,
            None => {
                // No file could be loaded; provide an empty (unlisted) script so
                // `ParseResult::root` stays valid.
                let empty = SourceFile::new(String::new());
                let mut parser =
                    Parser::new_in(&empty, self.options, std::mem::take(&mut self.arena));
                let root = parser.parse_type_spec_script();
                self.arena = parser.into_result(root).arena;
                root
            }
        };
        ProgramParseResult {
            parse_result: ParseResult {
                arena: self.arena,
                root,
                scripts: self.scripts,
                library_scripts: self.library_scripts,
                compiler_scripts: self.compiler_scripts,
                diagnostics: self.diagnostics,
                comments: self.comments,
            },
            sources: self.sources,
            import_graph: self.import_graph,
        }
    }

    fn import_file(
        &mut self,
        path: &str,
        target: Span,
        from: Option<SourceFileId>,
        kind: ImportKind,
        library: bool,
    ) {
        match get_any_extension_from_path(path).as_str() {
            ".tsp" => {
                if let Some(id) = self.load_typespec_file(path, target, library) {
                    if let Some(from) = from {
                        self.import_graph.add(from, id);
                    }
                }
            }
            ".js" | ".mjs" | ".ts" | ".mts" => {
                // JS libraries (decorator implementations) are not executed by
                // the Rust checker; record the file so the import graph is complete.
                let id = self.sources.add(
                    path.to_string(),
                    SourceFile::new(String::new()),
                    SourceFileKind::Js,
                );
                self.import_graph.add_file(id);
                if let Some(from) = from {
                    self.import_graph.add(from, id);
                }
            }
            _ => {
                let (code, message) = match kind {
                    ImportKind::Import => (
                        "invalid-import",
                        "Import paths must reference either a directory, a .tsp file, or .js file",
                    ),
                    ImportKind::Entrypoint => (
                        "invalid-main",
                        "Main file must either be a .tsp file or a .js file.",
                    ),
                };
                self.report(
                    DiagnosticSeverity::Error,
                    code,
                    message.to_string(),
                    Span::NO_TARGET,
                );
            }
        }
    }

    fn load_typespec_file(
        &mut self,
        path: &str,
        target: Span,
        library: bool,
    ) -> Option<SourceFileId> {
        if !self.seen.insert(path.to_string()) {
            return self.sources.get_by_path(path).map(|file| file.id);
        }
        let Some(text) = self.host.read_file(path) else {
            self.report(
                DiagnosticSeverity::Error,
                "file-not-found",
                format!("File {path} not found."),
                target,
            );
            return None;
        };

        let file = SourceFile::new(text);
        let base = self.sources.next_base();
        let start = self.arena.len();
        let mut parser = Parser::new_in(&file, self.options, std::mem::take(&mut self.arena));
        let root = parser.parse_type_spec_script();
        let result = parser.into_result(root);
        self.arena = result.arena;
        self.arena.shift_positions(start, base);
        self.diagnostics
            .extend(result.diagnostics.into_iter().map(|diagnostic| Diagnostic {
                target: shift_span(diagnostic.target, base),
                ..diagnostic
            }));
        self.comments.extend(result.comments);

        let id = self
            .sources
            .add(path.to_string(), file, SourceFileKind::TypeSpec);
        self.import_graph.add_file(id);
        self.scripts.push(root);
        if library {
            self.library_scripts.push(root);
        }
        if self.loading_compiler {
            self.compiler_scripts.push(root);
        }
        self.script_files.insert(root, id);
        self.load_script_imports(id, root, library);
        Some(id)
    }

    fn load_script_imports(&mut self, file_id: SourceFileId, root: NodeId, library: bool) {
        let path = self.sources.get(file_id).path.clone();
        let base_dir = get_directory_path(&path);
        let NodeData::TypeSpecScript { statements, .. } = &self.arena.get(root).data else {
            return;
        };
        let statements = statements.clone();

        let mut imports: Vec<(String, NodeId)> = Vec::new();
        let mut duplicates: Vec<(String, Vec<NodeId>)> = Vec::new();
        for statement in statements {
            let NodeData::ImportStatement { path: path_node } = &self.arena.get(statement).data
            else {
                continue;
            };
            let NodeData::StringLiteral { value } = &self.arena.get(*path_node).data else {
                continue;
            };
            let import_path = value.clone();

            if import_path.starts_with('.') && resolve_path(&base_dir, &[&import_path]) == path {
                self.report(
                    DiagnosticSeverity::Warning,
                    "self-import",
                    "A file cannot import itself.".to_string(),
                    self.node_span(statement),
                );
                continue;
            }

            match duplicates.iter_mut().find(|(key, _)| *key == import_path) {
                Some((_, nodes)) => nodes.push(statement),
                None => duplicates.push((import_path.clone(), vec![statement])),
            }
            imports.push((import_path, statement));
        }

        for (import_path, nodes) in &duplicates {
            for duplicate in nodes.iter().skip(1) {
                self.report(
                    DiagnosticSeverity::Warning,
                    "duplicate-import",
                    format!("Duplicate import of \"{import_path}\""),
                    self.node_span(*duplicate),
                );
            }
        }

        for (import_path, statement) in imports {
            let target = self.node_span(statement);
            self.import_path(&import_path, target, &base_dir, file_id, library);
        }
    }

    fn import_path(
        &mut self,
        specifier: &str,
        target: Span,
        base_dir: &str,
        from: SourceFileId,
        library: bool,
    ) {
        match self.host.resolve_module(specifier, base_dir) {
            Ok(module) => {
                let path = resolve_path(&module.path, &[]);
                self.import_file(
                    &path,
                    target,
                    Some(from),
                    ImportKind::Import,
                    library || module.library,
                );
            }
            Err(ModuleResolutionError::NotFound { manifest_path }) => {
                let target = self.manifest_target(manifest_path, target);
                self.report(
                    DiagnosticSeverity::Error,
                    "import-not-found",
                    format!("Couldn't resolve import \"{specifier}\""),
                    target,
                );
            }
            Err(ModuleResolutionError::InvalidLibrary {
                message,
                manifest_path,
            }) => {
                let target = self.manifest_target(manifest_path, target);
                self.report(
                    DiagnosticSeverity::Error,
                    "library-invalid",
                    format!("Library \"{specifier}\" is invalid: {message}"),
                    target,
                );
            }
        }
    }

    /// TS reports package resolution errors on the package's `package.json`.
    fn manifest_target(&mut self, manifest_path: Option<String>, fallback: Span) -> Span {
        let Some(manifest_path) = manifest_path else {
            return fallback;
        };
        let path = resolve_path(&manifest_path, &[]);
        let id = self.sources.add(
            path,
            SourceFile::new(String::new()),
            SourceFileKind::External,
        );
        let base = self.sources.get(id).base;
        Span {
            pos: base,
            end: base,
        }
    }

    fn node_span(&self, node: NodeId) -> Span {
        let node = self.arena.get(node);
        Span {
            pos: node.pos,
            end: node.end,
        }
    }

    fn report(
        &mut self,
        severity: DiagnosticSeverity,
        code: &'static str,
        message: String,
        target: Span,
    ) {
        self.diagnostics.push(Diagnostic {
            severity,
            code,
            message,
            target,
        });
    }
}

fn shift_span(span: Span, base: u32) -> Span {
    if span.is_no_target() {
        return span;
    }
    Span {
        pos: span.pos + base,
        end: span.end + base,
    }
}

/// In-memory [`SourceHost`] for tests: resolves relative/absolute specifiers,
/// directories with a `main.tsp`, and bare package names as
/// `/node_modules/<name>` (file or `main.tsp`). Records every read.
#[derive(Debug, Default)]
pub struct InMemorySourceHost {
    pub files: HashMap<String, String>,
    pub reads: Vec<String>,
}

impl InMemorySourceHost {
    pub fn new(files: &[(&str, &str)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(path, text)| (path.to_string(), text.to_string()))
                .collect(),
            reads: Vec::new(),
        }
    }
}

impl SourceHost for InMemorySourceHost {
    fn read_file(&mut self, path: &str) -> Option<String> {
        self.reads.push(path.to_string());
        self.files.get(path).cloned()
    }

    fn resolve_module(
        &mut self,
        specifier: &str,
        base_dir: &str,
    ) -> Result<ResolvedModule, ModuleResolutionError> {
        let library = !(specifier.starts_with('.') || specifier.starts_with('/'));
        let candidate = if !library {
            resolve_path(base_dir, &[specifier])
        } else {
            format!("/node_modules/{specifier}")
        };
        if self.files.contains_key(&candidate) {
            return Ok(ResolvedModule {
                path: candidate,
                library,
            });
        }
        let main = resolve_path(&candidate, &["main.tsp"]);
        if self.files.contains_key(&main) {
            return Ok(ResolvedModule {
                path: main,
                library,
            });
        }
        Err(ModuleResolutionError::NotFound {
            manifest_path: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(result: &ProgramParseResult) -> Vec<&str> {
        result
            .sources
            .files()
            .iter()
            .filter(|file| file.kind == SourceFileKind::TypeSpec)
            .map(|file| file.path.as_str())
            .collect()
    }

    fn codes(result: &ProgramParseResult) -> Vec<&'static str> {
        result
            .parse_result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn loads_imports_depth_first_in_statement_order() {
        let mut host = InMemorySourceHost::new(&[
            (
                "/main.tsp",
                "import \"./a.tsp\"; import \"./b.tsp\"; model Main {}",
            ),
            ("/a.tsp", "import \"./c.tsp\"; model A {}"),
            ("/b.tsp", "model B {}"),
            ("/c.tsp", "model C {}"),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        assert_eq!(paths(&result), ["/main.tsp", "/a.tsp", "/c.tsp", "/b.tsp"]);
        assert_eq!(result.parse_result.scripts.len(), 4);
        assert_eq!(result.parse_result.root, result.parse_result.scripts[0]);
        assert_eq!(result.sources.entry().map(|id| id.as_u32()), Some(0));
        assert!(codes(&result).is_empty());
        assert!(!result.import_graph.has_cycle());
    }

    #[test]
    fn circular_imports_load_each_file_once() {
        let mut host = InMemorySourceHost::new(&[
            ("/main.tsp", "import \"./a.tsp\"; model Main {}"),
            ("/a.tsp", "import \"./main.tsp\"; model A {}"),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        assert_eq!(paths(&result), ["/main.tsp", "/a.tsp"]);
        assert_eq!(host.reads, ["/main.tsp", "/a.tsp"]);
        assert!(result.import_graph.has_cycle());
        assert_eq!(result.import_graph.edges().len(), 2);
    }

    #[test]
    fn shifts_node_positions_into_program_space() {
        let mut host = InMemorySourceHost::new(&[
            ("/main.tsp", "import \"./a.tsp\";"),
            ("/a.tsp", "model A {}"),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        let a_script = result.parse_result.scripts[1];
        let NodeData::TypeSpecScript { statements, .. } =
            &result.parse_result.arena.get(a_script).data
        else {
            panic!("expected script");
        };
        let model = result.parse_result.arena.get(statements[0]);
        let a = result.sources.get_by_path("/a.tsp").unwrap();
        assert_eq!(a.base, 18);
        assert_eq!((model.pos, model.end), (18, 28));
        assert_eq!(
            result.sources.locate_span(Span {
                pos: model.pos,
                end: model.end
            }),
            Some((a.id, Span { pos: 0, end: 10 }))
        );
    }

    #[test]
    fn reports_import_not_found_at_import_statement() {
        let mut host =
            InMemorySourceHost::new(&[("/main.tsp", "model A {}\nimport \"./missing.tsp\";")]);
        let result = parse_program("/main.tsp", &mut host);
        assert_eq!(codes(&result), ["import-first", "import-not-found"]);
        let diagnostic = &result.parse_result.diagnostics[1];
        assert_eq!(
            diagnostic.message,
            "Couldn't resolve import \"./missing.tsp\""
        );
        assert_eq!(diagnostic.target, Span { pos: 11, end: 34 });
    }

    #[test]
    fn loads_stdlib_first_in_compiler_context() {
        let mut host = InMemorySourceHost::new(&[
            ("/main.tsp", "import \"./a.tsp\";"),
            ("/a.tsp", ""),
            ("/lib/intrinsics.tsp", "import \"./prototypes.tsp\";"),
            ("/lib/prototypes.tsp", ""),
            ("/lib/std/main.tsp", ""),
        ]);
        let result = parse_program_with_stdlib(
            "/main.tsp",
            &mut host,
            ParseOptions::default(),
            &[
                "/lib/intrinsics.tsp".to_string(),
                "/lib/std/main.tsp".to_string(),
            ],
        );
        assert_eq!(
            paths(&result),
            [
                "/lib/intrinsics.tsp",
                "/lib/prototypes.tsp",
                "/lib/std/main.tsp",
                "/main.tsp",
                "/a.tsp"
            ]
        );
        let parse = &result.parse_result;
        assert_eq!(parse.compiler_scripts, parse.scripts[..3]);
        assert_eq!(parse.library_scripts, parse.scripts[..3]);
        assert_eq!(parse.root, parse.scripts[3]);
        let entry = result.sources.entry().unwrap();
        assert_eq!(result.sources.get(entry).path, "/main.tsp");
    }

    #[test]
    fn marks_files_loaded_through_packages_as_library() {
        let mut host = InMemorySourceHost::new(&[
            ("/main.tsp", "import \"lib\"; import \"./local.tsp\";"),
            ("/local.tsp", ""),
            ("/node_modules/lib/main.tsp", "import \"./inner.tsp\";"),
            ("/node_modules/lib/inner.tsp", ""),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        let library_paths = result
            .parse_result
            .library_scripts
            .iter()
            .map(|script| {
                let pos = result.parse_result.arena.get(*script).pos;
                result
                    .sources
                    .get(result.sources.locate(pos).unwrap().0)
                    .path
                    .as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            library_paths,
            ["/node_modules/lib/main.tsp", "/node_modules/lib/inner.tsp"]
        );
    }

    #[test]
    fn reports_self_duplicate_and_invalid_imports() {
        let mut host = InMemorySourceHost::new(&[
            (
                "/main.tsp",
                "import \"./main.tsp\"; import \"./a.tsp\"; import \"./a.tsp\"; import \"./x.txt\";",
            ),
            ("/a.tsp", ""),
            ("/x.txt", ""),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        assert_eq!(
            codes(&result),
            ["self-import", "duplicate-import", "invalid-import"]
        );
        assert!(result.parse_result.diagnostics[2].target.is_no_target());
        assert_eq!(paths(&result), ["/main.tsp", "/a.tsp"]);
    }

    #[test]
    fn reports_missing_entry_without_target() {
        let mut host = InMemorySourceHost::new(&[]);
        let result = parse_program("/main.tsp", &mut host);
        assert_eq!(codes(&result), ["file-not-found"]);
        assert_eq!(
            result.parse_result.diagnostics[0].message,
            "File /main.tsp not found."
        );
        assert!(result.parse_result.scripts.is_empty());
    }

    #[test]
    fn records_js_imports_and_directory_packages() {
        let mut host = InMemorySourceHost::new(&[
            (
                "/main.tsp",
                "import \"./lib.js\"; import \"./dir\"; import \"pkg\";",
            ),
            ("/lib.js", ""),
            ("/dir/main.tsp", "model D {}"),
            ("/node_modules/pkg/main.tsp", "model P {}"),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        assert!(codes(&result).is_empty());
        assert_eq!(
            paths(&result),
            ["/main.tsp", "/dir/main.tsp", "/node_modules/pkg/main.tsp"]
        );
        let js = result.sources.get_by_path("/lib.js").unwrap();
        assert_eq!(js.kind, SourceFileKind::Js);
        assert!(!host.reads.contains(&"/lib.js".to_string()));
    }

    #[test]
    fn shifts_parse_diagnostics_of_imported_files() {
        let mut host = InMemorySourceHost::new(&[
            ("/main.tsp", "import \"./a.tsp\";"),
            ("/a.tsp", "model {}"),
        ]);
        let result = parse_program("/main.tsp", &mut host);
        let diagnostic = &result.parse_result.diagnostics[0];
        let (file, span) = result.sources.locate_span(diagnostic.target).unwrap();
        assert_eq!(result.sources.get(file).path, "/a.tsp");
        assert_eq!(span.pos, 6);
    }
}
