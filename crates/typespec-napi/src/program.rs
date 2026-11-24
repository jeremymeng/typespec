//! Multi-file program checking (M9). Module resolution and file reads are
//! delegated to synchronous JS callbacks; see `packages/compiler-rust/src/host.ts`.

use napi::bindgen_prelude::{FnArgs, Function, Object};
use napi::{Env, Result};
use napi_derive::napi;
use std::collections::HashMap;

use typespec_parser::ast::{NodeData, NodeId};
use typespec_parser::{
    parse_program_with_stdlib, ModuleResolutionError, ParseOptions, ParseResult, ResolvedModule,
    SourceHost,
};
use typespec_source::{ProgramSources, SourceFileKind, Span};

use typespec_checker::CheckOptions;

use crate::checker::{
    check_parsed, run_check, serialize_check_result, DecoratorStateEntry, NodeToType,
    SerializedType, SymbolToType,
};
use crate::decorator_apps::{
    retain_arena, serialize_decorators, DecoratorApplicationJs, ExternDeclarationJs, NodeInfoJs,
};
use crate::scanner::DiagnosticInfo;

/// Result of the JS `resolveModule(specifier, baseDir)` callback.
#[napi(object)]
pub struct ModuleResolutionJs {
    /// Final file to load (directory/package entrypoints already resolved).
    pub resolved_path: Option<String>,
    /// Resolved through a package manifest (TS `type: "module"`): the file is a library.
    pub library: Option<bool>,
    /// `"not-found"` or `"invalid"` when resolution failed.
    pub error: Option<String>,
    pub message: Option<String>,
    /// `package.json` the error should be reported on.
    pub manifest_path: Option<String>,
}

#[napi(object)]
pub struct ProgramDiagnosticInfo {
    pub severity: String,
    pub code: String,
    pub message: String,
    /// Absolute path of the file the diagnostic is reported in; absent for
    /// diagnostics without a target.
    pub file: Option<String>,
    /// Position local to `file`.
    pub pos: u32,
    pub end: u32,
}

#[napi(object)]
pub struct ProgramSourceFileInfo {
    pub path: String,
    /// `"typespec"`, `"js"` or `"external"`.
    pub kind: String,
}

#[napi(object)]
pub struct CheckProgramOptionsJs {
    /// Standard library entrypoints (`lib/intrinsics.tsp`, `lib/std/main.tsp`)
    /// loaded before the entry file in the compiler context (M10). Omitted or
    /// empty: no standard library (built-in intrinsic stubs).
    pub stdlib: Option<Vec<String>>,
    /// TS `auto-decorators` compiler feature (M11).
    pub auto_decorators: Option<bool>,
    /// TS `function-declarations` feature (default `false`: `fn` declarations warn).
    pub function_declarations: Option<bool>,
    /// Collect validated decorator applications so the JS host can run the real
    /// decorator implementations (M11). Ignored when a `decorators` registry
    /// (M6 bridge) is passed.
    pub collect_decorator_applications: Option<bool>,
    /// TS compiler features enabled in project files (project `tspconfig.yaml` `features`), M13.
    pub features: Option<Vec<String>>,
    /// Features enabled by libraries' own `tspconfig.yaml`, by library root directory (M13).
    pub library_features: Option<Vec<LibraryFeaturesJs>>,
}

#[napi(object)]
pub struct LibraryFeaturesJs {
    /// Library package root directory: its files get `features`.
    pub root: String,
    pub features: Vec<String>,
}

/// `#suppress "code" "message"` directive (M13): the JS host applies suppressions
/// to every diagnostic located inside the owner node, like TS `findDirectiveSuppressingOnNode`.
#[napi(object)]
pub struct SuppressDirectiveJs {
    /// Suppressed diagnostic code (first argument).
    pub code: String,
    pub file: String,
    /// Directive location (local to `file`).
    pub pos: u32,
    pub end: u32,
    /// Location of the node the directive is attached to.
    pub owner_pos: u32,
    pub owner_end: u32,
    /// Directive is in a project file (TS location context `"project"`).
    pub project: bool,
}

#[napi(object)]
pub struct CheckProgramResultJs {
    pub types: Vec<SerializedType>,
    pub symbol_to_type: Vec<SymbolToType>,
    pub node_to_type: Vec<NodeToType>,
    pub diagnostics: Vec<ProgramDiagnosticInfo>,
    pub decorator_state: Vec<DecoratorStateEntry>,
    /// Loaded files in load order (TypeSpec and JS imports).
    pub source_files: Vec<ProgramSourceFileInfo>,
    /// Validated decorator applications in TS execution order (M11).
    pub decorator_applications: Vec<DecoratorApplicationJs>,
    /// `dec` / `fn` declarations (M11).
    pub extern_declarations: Vec<ExternDeclarationJs>,
    /// Locations of the nodes referenced by types, applications and declarations.
    pub nodes: Vec<NodeInfoJs>,
    /// Handle for `isTypeAssignableTo` (set when collecting decorator applications).
    pub relation_handle: Option<u32>,
    /// Deprecated types (`#deprecated` directive or deprecated base), M11.
    pub deprecations: Vec<DeprecationJs>,
    /// `#suppress` directives in source order (M13).
    pub suppress_directives: Vec<SuppressDirectiveJs>,
}

#[napi(object)]
pub struct DeprecationJs {
    pub type_id: u32,
    pub message: String,
}

struct NapiSourceHost<'a> {
    read_file: Function<'a, String, Option<String>>,
    resolve_module: Function<'a, FnArgs<(String, String)>, ModuleResolutionJs>,
    error: Option<napi::Error>,
}

impl NapiSourceHost<'_> {
    fn record_error(&mut self, error: napi::Error) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

impl SourceHost for NapiSourceHost<'_> {
    fn read_file(&mut self, path: &str) -> Option<String> {
        match self.read_file.call(path.to_string()) {
            Ok(text) => text,
            Err(error) => {
                self.record_error(error);
                None
            }
        }
    }

    fn resolve_module(
        &mut self,
        specifier: &str,
        base_dir: &str,
    ) -> std::result::Result<ResolvedModule, ModuleResolutionError> {
        let result = match self
            .resolve_module
            .call((specifier.to_string(), base_dir.to_string()).into())
        {
            Ok(result) => result,
            Err(error) => {
                self.record_error(error);
                return Err(ModuleResolutionError::NotFound {
                    manifest_path: None,
                });
            }
        };
        match (result.resolved_path, result.error.as_deref()) {
            (Some(path), None) => Ok(ResolvedModule {
                path,
                library: result.library.unwrap_or(false),
            }),
            (_, Some("invalid")) => Err(ModuleResolutionError::InvalidLibrary {
                message: result.message.unwrap_or_default(),
                manifest_path: result.manifest_path,
            }),
            _ => Err(ModuleResolutionError::NotFound {
                manifest_path: result.manifest_path,
            }),
        }
    }
}

/// Load, parse and check the program rooted at `entry_path`.
#[napi]
pub fn check_program<'env>(
    env: &'env Env,
    entry_path: String,
    read_file: Function<'env, String, Option<String>>,
    resolve_module: Function<'env, FnArgs<(String, String)>, ModuleResolutionJs>,
    decorators: Option<Object<'env>>,
    options: Option<CheckProgramOptionsJs>,
) -> Result<CheckProgramResultJs> {
    let mut host = NapiSourceHost {
        read_file,
        resolve_module,
        error: None,
    };
    let (stdlib, mut check_options, library_features) = match options {
        Some(options) => (
            options.stdlib.unwrap_or_default(),
            CheckOptions {
                auto_decorators: options.auto_decorators.unwrap_or(false),
                function_declarations: options.function_declarations.unwrap_or(false),
                collect_decorator_applications: decorators.is_none()
                    && options.collect_decorator_applications.unwrap_or(false),
                project_features: options.features.unwrap_or_default(),
                ..CheckOptions::default()
            },
            options.library_features.unwrap_or_default(),
        ),
        None => (Vec::new(), CheckOptions::default(), Vec::new()),
    };
    let program =
        parse_program_with_stdlib(&entry_path, &mut host, ParseOptions::default(), &stdlib);
    if let Some(error) = host.error {
        return Err(error);
    }
    if !library_features.is_empty() {
        check_options.library_features =
            map_library_features(&program.parse_result, &program.sources, &library_features);
    }
    let suppress_directives = collect_suppress_directives(&program.parse_result, &program.sources);
    let collect = check_options.collect_decorator_applications;
    let mut deprecations = Vec::new();
    let (result, decorators_info, relation_handle) = if decorators.is_none() {
        let (mut checked, resolution_diagnostics) =
            run_check(env, &program.parse_result, None, check_options);
        deprecations = std::mem::take(&mut checked.deprecations)
            .into_iter()
            .map(|(type_id, message)| DeprecationJs {
                type_id: type_id.as_u32(),
                message,
            })
            .collect();
        let arena = collect.then(|| checked.type_arena.clone());
        let applications = std::mem::take(&mut checked.decorator_applications);
        let externs = std::mem::take(&mut checked.extern_declarations);
        let serialized = serialize_check_result(checked, resolution_diagnostics);
        let info = serialize_decorators(
            &applications,
            &externs,
            &serialized.types,
            &program.parse_result,
            &program.sources,
        );
        (serialized, Some(info), arena.map(retain_arena))
    } else {
        (
            check_parsed(env, &program.parse_result, decorators),
            None,
            None,
        )
    };
    let (decorator_applications, extern_declarations, nodes) = match decorators_info {
        Some(info) => (info.applications, info.extern_declarations, info.nodes),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    Ok(CheckProgramResultJs {
        types: result.types,
        symbol_to_type: result.symbol_to_type,
        node_to_type: result.node_to_type,
        diagnostics: result
            .diagnostics
            .into_iter()
            .map(|diagnostic| locate_diagnostic(diagnostic, &program.sources))
            .collect(),
        decorator_state: result.decorator_state,
        source_files: program
            .sources
            .files()
            .iter()
            .filter(|file| file.kind != SourceFileKind::External)
            .map(|file| ProgramSourceFileInfo {
                path: file.path.clone(),
                kind: match file.kind {
                    SourceFileKind::TypeSpec => "typespec",
                    SourceFileKind::Js => "js",
                    SourceFileKind::External => "external",
                }
                .to_string(),
            })
            .collect(),
        decorator_applications,
        extern_declarations,
        nodes,
        relation_handle,
        deprecations,
        suppress_directives,
    })
}

/// Library script -> features of the library whose root contains the script's file.
fn map_library_features(
    parse_result: &ParseResult,
    sources: &ProgramSources,
    library_features: &[LibraryFeaturesJs],
) -> HashMap<NodeId, Vec<String>> {
    let mut result = HashMap::new();
    for script in &parse_result.library_scripts {
        let node = parse_result.arena.get(*script);
        let Some((file, _)) = sources.locate_span(Span {
            pos: node.pos,
            end: node.end,
        }) else {
            continue;
        };
        let path = &sources.get(file).path;
        if let Some(library) = library_features
            .iter()
            .find(|library| path.starts_with(&format!("{}/", library.root.trim_end_matches('/'))))
        {
            result.insert(*script, library.features.clone());
        }
    }
    result
}

/// Every well-formed `#suppress` directive (TS `parseDirective`: first argument is the code),
/// sorted by file and position.
fn collect_suppress_directives(
    parse_result: &ParseResult,
    sources: &ProgramSources,
) -> Vec<SuppressDirectiveJs> {
    let arena = &parse_result.arena;
    let in_library = |pos: u32| {
        parse_result.library_scripts.iter().any(|script| {
            let script = arena.get(*script);
            script.pos <= pos && pos <= script.end
        })
    };
    let mut directives = Vec::new();
    for owner in arena.nodes() {
        for directive_id in &owner.directives {
            let directive = arena.get(*directive_id);
            let NodeData::DirectiveExpression { target, arguments } = &directive.data else {
                continue;
            };
            if !matches!(&arena.get(*target).data, NodeData::Identifier { sv } if sv == "suppress")
            {
                continue;
            }
            let code = match arguments.first().map(|arg| &arena.get(*arg).data) {
                Some(NodeData::StringLiteral { value }) => value.clone(),
                Some(NodeData::Identifier { sv }) => sv.clone(),
                _ => continue,
            };
            let Some((file, local)) = sources.locate_span(Span {
                pos: directive.pos,
                end: directive.end,
            }) else {
                continue;
            };
            let offset = directive.pos - local.pos;
            directives.push((
                file,
                SuppressDirectiveJs {
                    code,
                    file: sources.get(file).path.clone(),
                    pos: local.pos,
                    end: local.end,
                    owner_pos: owner.pos.saturating_sub(offset),
                    owner_end: owner.end.saturating_sub(offset),
                    project: !in_library(directive.pos),
                },
            ));
        }
    }
    directives.sort_by_key(|(file, directive)| (*file, directive.pos));
    directives
        .into_iter()
        .map(|(_, directive)| directive)
        .collect()
}

fn locate_diagnostic(
    diagnostic: DiagnosticInfo,
    sources: &ProgramSources,
) -> ProgramDiagnosticInfo {
    let span = Span {
        pos: diagnostic.pos,
        end: diagnostic.end,
    };
    let (file, pos, end) = match sources.locate_span(span) {
        Some((file, local)) => (Some(sources.get(file).path.clone()), local.pos, local.end),
        None => (None, 0, 0),
    };
    ProgramDiagnosticInfo {
        severity: diagnostic.severity,
        code: diagnostic.code,
        message: diagnostic.message,
        file,
        pos,
        end,
    }
}
