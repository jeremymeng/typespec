//! Multi-file program checking (M9). Module resolution and file reads are
//! delegated to synchronous JS callbacks; see `packages/compiler-rust/src/host.ts`.

use napi::bindgen_prelude::{FnArgs, Function, Object};
use napi::{Env, Result};
use napi_derive::napi;
use typespec_parser::{
    parse_program_with_stdlib, ModuleResolutionError, ParseOptions, ResolvedModule, SourceHost,
};
use typespec_source::{ProgramSources, SourceFileKind, Span};

use crate::checker::{check_parsed, DecoratorStateEntry, NodeToType, SerializedType, SymbolToType};
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
    let stdlib = options
        .and_then(|options| options.stdlib)
        .unwrap_or_default();
    let program =
        parse_program_with_stdlib(&entry_path, &mut host, ParseOptions::default(), &stdlib);
    if let Some(error) = host.error {
        return Err(error);
    }
    let result = check_parsed(env, &program.parse_result, decorators);
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
    })
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
