use napi_derive::napi;
use typespec_checker::{bind as bind_parse_result, resolve_names, ResolutionResult};
use typespec_parser::parse;
use typespec_source::{Diagnostic, DiagnosticSeverity, SourceFile};

use crate::scanner::DiagnosticInfo;

#[napi(object)]
pub struct SerializedSymbol {
    pub id: u32,
    pub name: String,
    pub flags: u32,
    pub declaration_node_ids: Vec<u32>,
    pub parent_scope_id: u32,
    pub members_scope_id: Option<u32>,
}

#[napi(object)]
pub struct SerializedScope {
    pub id: u32,
    pub kind: String,
    pub parent_id: Option<u32>,
    pub owning_node_id: Option<u32>,
    pub local_symbol_ids: Vec<u32>,
    pub local_symbol_names: Vec<String>,
    pub using_scope_ids: Vec<u32>,
}

#[napi(object)]
pub struct SerializedResolution {
    pub node_id: u32,
    pub kind: String,
    pub symbol_id: Option<u32>,
    pub ambiguous_symbol_ids: Vec<u32>,
}

#[napi(object)]
pub struct BindResult {
    pub symbols: Vec<SerializedSymbol>,
    pub scopes: Vec<SerializedScope>,
    pub resolutions: Vec<SerializedResolution>,
    pub global_scope_id: u32,
    pub diagnostics: Vec<DiagnosticInfo>,
}

#[napi]
pub fn bind(source: String) -> BindResult {
    let source = SourceFile::new(source);
    let parse_result = parse(&source);
    let mut table = bind_parse_result(&parse_result);
    let resolution = resolve_names(&parse_result, &mut table);

    let mut diagnostics = Vec::new();
    diagnostics.extend(table.diagnostics.iter().cloned());
    diagnostics.extend(resolution.diagnostics);

    let symbols = table
        .symbols
        .iter()
        .enumerate()
        .map(|(id, symbol)| SerializedSymbol {
            id: id as u32,
            name: symbol.name.clone(),
            flags: symbol.flags.bits(),
            declaration_node_ids: symbol.declarations.iter().map(|id| id.as_u32()).collect(),
            parent_scope_id: symbol.parent_scope.as_u32(),
            members_scope_id: symbol.members.map(|id| id.as_u32()),
        })
        .collect();

    let scopes = table
        .scopes
        .iter()
        .enumerate()
        .map(|(id, scope)| {
            let mut entries = scope
                .symbols
                .iter()
                .map(|(name, symbol)| (name.clone(), symbol.as_u32()))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            SerializedScope {
                id: id as u32,
                kind: scope.kind.as_str().to_string(),
                parent_id: scope.parent.map(|id| id.as_u32()),
                owning_node_id: scope.owning_node.map(|id| id.as_u32()),
                local_symbol_names: entries.iter().map(|(name, _)| name.clone()).collect(),
                local_symbol_ids: entries.iter().map(|(_, symbol)| *symbol).collect(),
                using_scope_ids: scope.usings.iter().map(|id| id.as_u32()).collect(),
            }
        })
        .collect();

    let mut resolutions = resolution
        .resolutions
        .into_iter()
        .map(|(node_id, result)| match result {
            ResolutionResult::Resolved(symbol) => SerializedResolution {
                node_id: node_id.as_u32(),
                kind: "resolved".to_string(),
                symbol_id: Some(symbol.as_u32()),
                ambiguous_symbol_ids: Vec::new(),
            },
            ResolutionResult::Ambiguous(symbols) => SerializedResolution {
                node_id: node_id.as_u32(),
                kind: "ambiguous".to_string(),
                symbol_id: None,
                ambiguous_symbol_ids: symbols.iter().map(|id| id.as_u32()).collect(),
            },
            ResolutionResult::Unknown => SerializedResolution {
                node_id: node_id.as_u32(),
                kind: "unknown".to_string(),
                symbol_id: None,
                ambiguous_symbol_ids: Vec::new(),
            },
        })
        .collect::<Vec<_>>();
    resolutions.sort_by_key(|resolution| resolution.node_id);

    BindResult {
        symbols,
        scopes,
        resolutions,
        global_scope_id: table.global_scope.as_u32(),
        diagnostics: diagnostics.into_iter().map(convert_diagnostic).collect(),
    }
}

fn convert_diagnostic(diagnostic: Diagnostic) -> DiagnosticInfo {
    DiagnosticInfo {
        severity: match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Hint => "hint",
        }
        .to_string(),
        code: diagnostic.code.to_string(),
        message: diagnostic.message,
        pos: diagnostic.target.pos,
        end: diagnostic.target.end,
    }
}
