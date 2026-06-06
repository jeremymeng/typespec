use napi_derive::napi;
use typespec_checker::{
    bind as bind_parse_result, check as check_parse_result, resolve_names, CheckResult,
    EnumMemberValue, Type,
};
use typespec_parser::parse;
use typespec_source::{Diagnostic, DiagnosticSeverity, SourceFile};

use crate::scanner::DiagnosticInfo;

#[napi(object)]
pub struct SerializedType {
    pub id: u32,
    pub kind: String,
    pub name: String,
    pub node_id: Option<u32>,
    pub symbol_id: Option<u32>,
    pub property_ids: Vec<u32>,
    pub property_names: Vec<String>,
    pub base_type_id: Option<u32>,
    pub source_model_id: Option<u32>,
    pub indexer_type_id: Option<u32>,
    pub property_type_id: Option<u32>,
    pub optional: Option<bool>,
    pub default_type_id: Option<u32>,
    pub decorator_node_ids: Vec<u32>,
    pub member_ids: Vec<u32>,
    pub member_names: Vec<String>,
    pub variant_type_id: Option<u32>,
    pub target_id: Option<u32>,
    pub literal_string: Option<String>,
    pub literal_numeric: Option<f64>,
    pub literal_boolean: Option<bool>,
    pub enum_member_value_string: Option<String>,
    pub enum_member_value_numeric: Option<f64>,
    pub expression: Option<bool>,
}

#[napi(object)]
pub struct SymbolToType {
    pub symbol_id: u32,
    pub type_id: u32,
}

#[napi(object)]
pub struct NodeToType {
    pub node_id: u32,
    pub type_id: u32,
}

#[napi(object)]
pub struct CheckResultJs {
    pub types: Vec<SerializedType>,
    pub symbol_to_type: Vec<SymbolToType>,
    pub node_to_type: Vec<NodeToType>,
    pub diagnostics: Vec<DiagnosticInfo>,
}

#[napi]
pub fn check(source: String) -> CheckResultJs {
    let source = SourceFile::new(source);
    let parse_result = parse(&source);
    let mut table = bind_parse_result(&parse_result);
    let resolution = resolve_names(&parse_result, &mut table);
    let result = check_parse_result(&parse_result, &table, &resolution.resolutions);
    serialize_check_result(result, resolution.diagnostics)
}

fn serialize_check_result(
    result: CheckResult,
    resolution_diagnostics: Vec<Diagnostic>,
) -> CheckResultJs {
    let mut diagnostics = result.diagnostics;
    diagnostics.extend(resolution_diagnostics);

    let mut symbol_to_type = result
        .symbol_to_type
        .iter()
        .map(|(symbol_id, type_id)| SymbolToType {
            symbol_id: symbol_id.as_u32(),
            type_id: type_id.as_u32(),
        })
        .collect::<Vec<_>>();
    symbol_to_type.sort_by_key(|entry| (entry.symbol_id, entry.type_id));

    let mut node_to_type = result
        .node_to_type
        .iter()
        .map(|(node_id, type_id)| NodeToType {
            node_id: node_id.as_u32(),
            type_id: type_id.as_u32(),
        })
        .collect::<Vec<_>>();
    node_to_type.sort_by_key(|entry| (entry.node_id, entry.type_id));

    CheckResultJs {
        types: result
            .type_arena
            .types()
            .iter()
            .enumerate()
            .map(|(id, ty)| serialize_type(id as u32, ty))
            .collect(),
        symbol_to_type,
        node_to_type,
        diagnostics: diagnostics.into_iter().map(convert_diagnostic).collect(),
    }
}

fn serialize_type(id: u32, ty: &Type) -> SerializedType {
    let mut serialized = SerializedType {
        id,
        kind: kind_name(ty).to_string(),
        name: String::new(),
        node_id: None,
        symbol_id: None,
        property_ids: Vec::new(),
        property_names: Vec::new(),
        base_type_id: None,
        source_model_id: None,
        indexer_type_id: None,
        property_type_id: None,
        optional: None,
        default_type_id: None,
        decorator_node_ids: Vec::new(),
        member_ids: Vec::new(),
        member_names: Vec::new(),
        variant_type_id: None,
        target_id: None,
        literal_string: None,
        literal_numeric: None,
        literal_boolean: None,
        enum_member_value_string: None,
        enum_member_value_numeric: None,
        expression: None,
    };

    match ty {
        Type::Scalar(scalar) => {
            serialized.name = scalar.name.clone();
            serialized.node_id = scalar.node.map(|id| id.as_u32());
            serialized.symbol_id = scalar.symbol.map(|id| id.as_u32());
            serialized.base_type_id = scalar.base_scalar.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                scalar.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::Model(model) => {
            serialized.name = model.name.clone();
            serialized.node_id = model.node.map(|id| id.as_u32());
            serialized.symbol_id = model.symbol.map(|id| id.as_u32());
            serialized.property_names = model.properties.keys().cloned().collect();
            serialized.property_ids = model.properties.values().map(|id| id.as_u32()).collect();
            serialized.base_type_id = model.base_model.map(|id| id.as_u32());
            serialized.source_model_id = model.source_model.map(|id| id.as_u32());
            serialized.indexer_type_id = model.indexer.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                model.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::ModelProperty(prop) => {
            serialized.name = prop.name.clone();
            serialized.node_id = prop.node.map(|id| id.as_u32());
            serialized.symbol_id = prop.symbol.map(|id| id.as_u32());
            serialized.base_type_id = Some(prop.model.as_u32());
            serialized.property_type_id = Some(prop.property_type.as_u32());
            serialized.optional = Some(prop.optional);
            serialized.default_type_id = prop.default.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                prop.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::Enum(en) => {
            serialized.name = en.name.clone();
            serialized.node_id = en.node.map(|id| id.as_u32());
            serialized.symbol_id = en.symbol.map(|id| id.as_u32());
            serialized.member_names = en.members.keys().cloned().collect();
            serialized.member_ids = en.members.values().map(|id| id.as_u32()).collect();
            serialized.decorator_node_ids = en.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::EnumMember(member) => {
            serialized.name = member.name.clone();
            serialized.node_id = member.node.map(|id| id.as_u32());
            serialized.base_type_id = Some(member.r#enum.as_u32());
            serialized.decorator_node_ids =
                member.decorators.iter().map(|d| d.node.as_u32()).collect();
            match &member.value {
                Some(EnumMemberValue::String(value)) => {
                    serialized.enum_member_value_string = Some(value.clone());
                }
                Some(EnumMemberValue::Numeric(value)) => {
                    serialized.enum_member_value_numeric = Some(*value);
                }
                None => {}
            }
        }
        Type::Union(union) => {
            serialized.name = union.name.clone();
            serialized.node_id = union.node.map(|id| id.as_u32());
            serialized.symbol_id = union.symbol.map(|id| id.as_u32());
            serialized.member_names = union.variants.keys().cloned().collect();
            serialized.member_ids = union.variants.values().map(|id| id.as_u32()).collect();
            serialized.expression = Some(union.expression);
            serialized.decorator_node_ids =
                union.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::UnionVariant(variant) => {
            serialized.name = variant.name.clone();
            serialized.node_id = variant.node.map(|id| id.as_u32());
            serialized.base_type_id = Some(variant.union.as_u32());
            serialized.variant_type_id = Some(variant.variant_type.as_u32());
            serialized.decorator_node_ids =
                variant.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::Alias(alias) => {
            serialized.name = alias.name.clone();
            serialized.node_id = Some(alias.node.as_u32());
            serialized.symbol_id = Some(alias.symbol.as_u32());
            serialized.target_id = Some(alias.target.as_u32());
        }
        Type::StringLiteral(value) => {
            serialized.name = value.value.clone();
            serialized.literal_string = Some(value.value.clone());
        }
        Type::NumericLiteral(value) => {
            serialized.name = format_number(value.value);
            serialized.literal_numeric = Some(value.value);
        }
        Type::BooleanLiteral(value) => {
            serialized.name = value.value.to_string();
            serialized.literal_boolean = Some(value.value);
        }
        _ => {}
    }
    serialized
}

fn kind_name(ty: &Type) -> &'static str {
    match ty {
        Type::Scalar(_) => "Scalar",
        Type::Model(_) => "Model",
        Type::ModelProperty(_) => "ModelProperty",
        Type::Enum(_) => "Enum",
        Type::EnumMember(_) => "EnumMember",
        Type::Union(_) => "Union",
        Type::UnionVariant(_) => "UnionVariant",
        Type::Alias(_) => "Alias",
        Type::String => "String",
        Type::Bytes => "Bytes",
        Type::Int8 => "Int8",
        Type::Int16 => "Int16",
        Type::Int32 => "Int32",
        Type::Int64 => "Int64",
        Type::Uint8 => "Uint8",
        Type::Uint16 => "Uint16",
        Type::Uint32 => "Uint32",
        Type::Uint64 => "Uint64",
        Type::Float => "Float",
        Type::Float32 => "Float32",
        Type::Float64 => "Float64",
        Type::Decimal => "Decimal",
        Type::Decimal128 => "Decimal128",
        Type::Integer => "Integer",
        Type::Numeric => "Numeric",
        Type::SafeInt => "SafeInt",
        Type::Boolean => "Boolean",
        Type::Url => "Url",
        Type::PlainDate => "PlainDate",
        Type::PlainTime => "PlainTime",
        Type::UtcDateTime => "UtcDateTime",
        Type::OffsetDateTime => "OffsetDateTime",
        Type::Duration => "Duration",
        Type::Void => "Void",
        Type::Never => "Never",
        Type::Unknown => "Unknown",
        Type::Null => "Null",
        Type::StringLiteral(_) => "StringLiteral",
        Type::NumericLiteral(_) => "NumericLiteral",
        Type::BooleanLiteral(_) => "BooleanLiteral",
        Type::Error => "Error",
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
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
