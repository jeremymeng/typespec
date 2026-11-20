use napi::bindgen_prelude::{BigInt, Object};
use napi::Env;
use napi_derive::napi;
use typespec_checker::{
    bind as bind_parse_result, check as check_parse_result, check_with_decorator_runner,
    check_with_options, resolve_names,
    type_relation::{get_effective_model_type, type_name, TypeRelationChecker},
    CheckOptions, CheckResult, DecoratorValue, EnumMemberValue, Type, TypeId,
};
use typespec_parser::{parse, ParseResult};
use typespec_source::{bigint_to_napi_words, Diagnostic, DiagnosticSeverity, Numeric, SourceFile};

use crate::decorator::NapiDecoratorRunner;
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
    /// Indexer value type.
    pub indexer_type_id: Option<u32>,
    pub indexer_key_type_id: Option<u32>,
    pub namespace_id: Option<u32>,
    pub property_type_id: Option<u32>,
    pub optional: Option<bool>,
    pub default_type_id: Option<u32>,
    pub source_property_id: Option<u32>,
    pub effective_type_id: Option<u32>,
    pub decorator_node_ids: Vec<u32>,
    pub member_ids: Vec<u32>,
    pub member_names: Vec<String>,
    pub operation_ids: Vec<u32>,
    pub operation_names: Vec<String>,
    pub source_interface_ids: Vec<u32>,
    pub model_ids: Vec<u32>,
    pub model_names: Vec<String>,
    pub scalar_ids: Vec<u32>,
    pub scalar_names: Vec<String>,
    pub enum_ids: Vec<u32>,
    pub enum_names: Vec<String>,
    pub union_ids: Vec<u32>,
    pub union_names: Vec<String>,
    pub alias_ids: Vec<u32>,
    pub alias_names: Vec<String>,
    pub interface_ids: Vec<u32>,
    pub interface_names: Vec<String>,
    pub namespace_ids: Vec<u32>,
    pub namespace_names: Vec<String>,
    pub parameters_type_id: Option<u32>,
    pub return_type_id: Option<u32>,
    pub interface_id: Option<u32>,
    pub source_operation_id: Option<u32>,
    pub variant_type_id: Option<u32>,
    pub target_id: Option<u32>,
    pub literal_string: Option<String>,
    pub literal_numeric: Option<f64>,
    pub literal_numeric_bigint: Option<BigInt>,
    pub literal_numeric_string: Option<String>,
    pub literal_numeric_number: Option<f64>,
    pub literal_boolean: Option<bool>,
    pub enum_member_value_string: Option<String>,
    pub enum_member_value_numeric: Option<f64>,
    pub enum_member_value_numeric_bigint: Option<BigInt>,
    pub enum_member_value_numeric_string: Option<String>,
    pub enum_member_value_numeric_number: Option<f64>,
    pub expression: Option<bool>,
    pub template_parameter_ids: Vec<u32>,
    pub template_argument_ids: Vec<u32>,
    pub is_template_declaration: bool,
    pub constraint_id: Option<u32>,
    pub template_default_id: Option<u32>,
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
    pub decorator_state: Vec<DecoratorStateEntry>,
}

#[napi(object)]
pub struct DecoratorStateEntry {
    pub type_id: u32,
    pub key: String,
    pub value_string: Option<String>,
    pub value_number: Option<f64>,
    pub value_boolean: Option<bool>,
    pub value_type_id: Option<u32>,
}

#[napi(object)]
pub struct RelationCheckResult {
    pub related: bool,
    pub source_type_id: Option<u32>,
    pub target_type_id: Option<u32>,
    pub diagnostics: Vec<DiagnosticInfo>,
}

#[napi]
pub fn check(env: Env, source: String, decorators: Option<Object>) -> CheckResultJs {
    let source = SourceFile::new(source);
    let parse_result = parse(&source);
    check_parsed(&env, &parse_result, decorators)
}

/// Bind, resolve and check an already parsed (single- or multi-file) program.
pub(crate) fn check_parsed<'env>(
    env: &'env Env,
    parse_result: &ParseResult,
    decorators: Option<Object<'env>>,
) -> CheckResultJs {
    let (result, resolution_diagnostics) =
        run_check(env, parse_result, decorators, CheckOptions::default());
    serialize_check_result(result, resolution_diagnostics)
}

/// Bind, resolve and check, returning the raw result (M11: options and the
/// collected decorator applications are needed by `checkProgram`).
pub(crate) fn run_check<'env>(
    env: &'env Env,
    parse_result: &ParseResult,
    decorators: Option<Object<'env>>,
    options: CheckOptions,
) -> (CheckResult, Vec<Diagnostic>) {
    let mut table = bind_parse_result(parse_result);
    let resolution = resolve_names(parse_result, &mut table);
    let result = if decorators.is_some() {
        match NapiDecoratorRunner::new(env, decorators) {
            Ok(mut runner) => check_with_decorator_runner(
                parse_result,
                &table,
                &resolution.resolutions,
                &mut runner,
            ),
            Err(error) => {
                let mut result = check_parse_result(parse_result, &table, &resolution.resolutions);
                result.diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: "decorator-registry",
                    message: format!("Invalid decorator registry: {error}"),
                    target: typespec_source::Span { pos: 0, end: 0 },
                });
                result
            }
        }
    } else {
        check_with_options(parse_result, &table, &resolution.resolutions, options)
    };
    (result, resolution.diagnostics)
}

#[napi]
pub fn check_relation(
    source: String,
    source_type_name: String,
    target_type_name: String,
) -> RelationCheckResult {
    let source_file = SourceFile::new(source);
    let parse_result = parse(&source_file);
    let mut table = bind_parse_result(&parse_result);
    let resolution = resolve_names(&parse_result, &mut table);
    let mut result = check_parse_result(&parse_result, &table, &resolution.resolutions);
    result.diagnostics.extend(resolution.diagnostics);

    let source_type = find_type_by_name(&result, &source_type_name);
    let target_type = find_type_by_name(&result, &target_type_name);
    let mut diagnostics = std::mem::take(&mut result.diagnostics);
    let related = if diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error)
    {
        false
    } else if let (Some(source_type), Some(target_type)) = (source_type, target_type) {
        let relation = TypeRelationChecker::new(&result.type_arena);
        let relation_diagnostics = relation.check_type_assignable_span(
            source_type,
            target_type,
            typespec_source::Span { pos: 0, end: 0 },
        );
        let related = relation_diagnostics.is_empty();
        diagnostics.extend(relation_diagnostics);
        related
    } else {
        diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "invalid-ref",
            message: format!(
                "Cannot find relation query types '{}' and '{}'",
                source_type_name, target_type_name
            ),
            target: typespec_source::Span { pos: 0, end: 0 },
        });
        false
    };

    RelationCheckResult {
        related,
        source_type_id: source_type.map(|id| id.as_u32()),
        target_type_id: target_type.map(|id| id.as_u32()),
        diagnostics: diagnostics.into_iter().map(convert_diagnostic).collect(),
    }
}

pub(crate) fn serialize_check_result(
    result: CheckResult,
    resolution_diagnostics: Vec<Diagnostic>,
) -> CheckResultJs {
    let decorator_state = serialize_decorator_state(&result);
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

    let mut types = result
        .type_arena
        .types()
        .iter()
        .enumerate()
        .map(|(id, ty)| serialize_type(id as u32, ty, &result.type_arena))
        .collect::<Vec<_>>();
    // Intrinsic scalars declared by the standard library (`scalar string;`).
    for (type_id, declaration) in &result.intrinsic_declarations {
        let serialized = &mut types[type_id.index()];
        serialized.node_id = Some(declaration.node.as_u32());
        serialized.symbol_id = Some(declaration.symbol.as_u32());
        serialized.namespace_id = declaration.namespace.map(|id| id.as_u32());
        serialized.base_type_id = declaration.base_scalar.map(|id| id.as_u32());
        serialized.decorator_node_ids = declaration
            .decorators
            .iter()
            .map(|d| d.node.as_u32())
            .collect();
    }

    CheckResultJs {
        types,
        symbol_to_type,
        node_to_type,
        diagnostics: diagnostics.into_iter().map(convert_diagnostic).collect(),
        decorator_state,
    }
}

fn serialize_decorator_state(result: &CheckResult) -> Vec<DecoratorStateEntry> {
    let mut entries = Vec::new();
    for (type_id, state) in &result.decorator_state {
        for (key, value) in state {
            let mut entry = DecoratorStateEntry {
                type_id: type_id.as_u32(),
                key: key.clone(),
                value_string: None,
                value_number: None,
                value_boolean: None,
                value_type_id: None,
            };
            match value {
                DecoratorValue::String(value) => entry.value_string = Some(value.clone()),
                DecoratorValue::Number(value) => entry.value_number = Some(*value),
                DecoratorValue::Boolean(value) => entry.value_boolean = Some(*value),
                DecoratorValue::TypeRef(value) => entry.value_type_id = Some(value.as_u32()),
                DecoratorValue::None => {}
            }
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| (a.type_id, &a.key).cmp(&(b.type_id, &b.key)));
    entries
}

fn find_type_by_name(result: &CheckResult, name: &str) -> Option<TypeId> {
    result
        .type_arena
        .types()
        .iter()
        .enumerate()
        .find_map(|(id, ty)| {
            let type_id = TypeId::new(id as u32);
            let matches = match ty {
                Type::Scalar(scalar) => scalar.name == name,
                Type::Model(model) => model.name == name,
                Type::ModelProperty(prop) => prop.name == name,
                Type::Enum(en) => en.name == name,
                Type::EnumMember(member) => member.name == name,
                Type::Union(union) => union.name == name,
                Type::UnionVariant(variant) => variant.name == name,
                Type::Alias(alias) => alias.name == name,
                Type::Operation(operation) => operation.name == name,
                Type::Interface(interface) => interface.name == name,
                Type::Namespace(namespace) => namespace.name == name,
                _ => type_name(&result.type_arena, type_id) == name,
            };
            matches.then_some(type_id)
        })
}

fn serialize_type(id: u32, ty: &Type, arena: &typespec_checker::TypeArena) -> SerializedType {
    let mut serialized = SerializedType {
        id,
        kind: ty.kind_name().to_string(),
        name: String::new(),
        node_id: None,
        symbol_id: None,
        property_ids: Vec::new(),
        property_names: Vec::new(),
        base_type_id: None,
        source_model_id: None,
        indexer_type_id: None,
        indexer_key_type_id: None,
        namespace_id: None,
        property_type_id: None,
        optional: None,
        default_type_id: None,
        source_property_id: None,
        effective_type_id: None,
        decorator_node_ids: Vec::new(),
        member_ids: Vec::new(),
        member_names: Vec::new(),
        operation_ids: Vec::new(),
        operation_names: Vec::new(),
        source_interface_ids: Vec::new(),
        model_ids: Vec::new(),
        model_names: Vec::new(),
        scalar_ids: Vec::new(),
        scalar_names: Vec::new(),
        enum_ids: Vec::new(),
        enum_names: Vec::new(),
        union_ids: Vec::new(),
        union_names: Vec::new(),
        alias_ids: Vec::new(),
        alias_names: Vec::new(),
        interface_ids: Vec::new(),
        interface_names: Vec::new(),
        namespace_ids: Vec::new(),
        namespace_names: Vec::new(),
        parameters_type_id: None,
        return_type_id: None,
        interface_id: None,
        source_operation_id: None,
        variant_type_id: None,
        target_id: None,
        literal_string: None,
        literal_numeric: None,
        literal_numeric_bigint: None,
        literal_numeric_string: None,
        literal_numeric_number: None,
        literal_boolean: None,
        enum_member_value_string: None,
        enum_member_value_numeric: None,
        enum_member_value_numeric_bigint: None,
        enum_member_value_numeric_string: None,
        enum_member_value_numeric_number: None,
        expression: None,
        template_parameter_ids: Vec::new(),
        template_argument_ids: Vec::new(),
        is_template_declaration: false,
        constraint_id: None,
        template_default_id: None,
    };

    match ty {
        Type::Scalar(scalar) => {
            serialized.name = scalar.name.clone();
            serialized.node_id = scalar.node.map(|id| id.as_u32());
            serialized.symbol_id = scalar.symbol.map(|id| id.as_u32());
            serialized.base_type_id = scalar.base_scalar.map(|id| id.as_u32());
            serialized.expression = Some(scalar.expression);
            serialized.namespace_id = scalar.namespace.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                scalar.decorators.iter().map(|d| d.node.as_u32()).collect();
            serialized.template_parameter_ids = scalar
                .template_parameters
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.template_argument_ids = scalar
                .template_arguments
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.is_template_declaration = scalar.is_template_declaration;
        }
        Type::Model(model) => {
            serialized.name = model.name.clone();
            serialized.node_id = model.node.map(|id| id.as_u32());
            serialized.symbol_id = model.symbol.map(|id| id.as_u32());
            serialized.property_names = model.properties.keys().cloned().collect();
            serialized.property_ids = model.properties.values().map(|id| id.as_u32()).collect();
            serialized.base_type_id = model.base_model.map(|id| id.as_u32());
            serialized.source_model_id = model.source_model.map(|id| id.as_u32());
            serialized.indexer_type_id = model.indexer.map(|indexer| indexer.value.as_u32());
            serialized.indexer_key_type_id = model.indexer.map(|indexer| indexer.key.as_u32());
            serialized.expression = Some(model.expression);
            serialized.effective_type_id =
                Some(get_effective_model_type(arena, TypeId::new(id)).as_u32());
            serialized.namespace_id = model.namespace.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                model.decorators.iter().map(|d| d.node.as_u32()).collect();
            serialized.template_parameter_ids = model
                .template_parameters
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.template_argument_ids = model
                .template_arguments
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.is_template_declaration = model.is_template_declaration;
        }
        Type::ModelProperty(prop) => {
            serialized.name = prop.name.clone();
            serialized.node_id = prop.node.map(|id| id.as_u32());
            serialized.symbol_id = prop.symbol.map(|id| id.as_u32());
            serialized.base_type_id = Some(prop.model.as_u32());
            serialized.property_type_id = Some(prop.property_type.as_u32());
            serialized.optional = Some(prop.optional);
            serialized.default_type_id = prop.default.map(|id| id.as_u32());
            serialized.source_property_id = prop.source_property.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                prop.decorators.iter().map(|d| d.node.as_u32()).collect();
        }
        Type::Enum(en) => {
            serialized.name = en.name.clone();
            serialized.node_id = en.node.map(|id| id.as_u32());
            serialized.symbol_id = en.symbol.map(|id| id.as_u32());
            serialized.member_names = en.members.keys().cloned().collect();
            serialized.member_ids = en.members.values().map(|id| id.as_u32()).collect();
            serialized.namespace_id = en.namespace.map(|id| id.as_u32());
            serialized.expression = Some(en.expression);
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
                    serialized.enum_member_value_numeric = value.as_number();
                    serialized.enum_member_value_numeric_number = value.as_number();
                    if let Some(bigint) = numeric_bigint(value) {
                        serialized.enum_member_value_numeric_bigint = Some(bigint);
                    } else {
                        serialized.enum_member_value_numeric_string = Some(value.to_string());
                    }
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
            serialized.base_type_id = union.base_type.map(|id| id.as_u32());
            serialized.namespace_id = union.namespace.map(|id| id.as_u32());
            serialized.decorator_node_ids =
                union.decorators.iter().map(|d| d.node.as_u32()).collect();
            serialized.template_parameter_ids = union
                .template_parameters
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.template_argument_ids = union
                .template_arguments
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.is_template_declaration = union.is_template_declaration;
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
            serialized.namespace_id = alias.namespace.map(|id| id.as_u32());
            serialized.template_parameter_ids = alias
                .template_parameters
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.template_argument_ids = alias
                .template_arguments
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.is_template_declaration = alias.is_template_declaration;
        }
        Type::Operation(operation) => {
            serialized.name = operation.name.clone();
            serialized.node_id = operation.node.map(|id| id.as_u32());
            serialized.symbol_id = operation.symbol.map(|id| id.as_u32());
            serialized.parameters_type_id = Some(operation.parameters.as_u32());
            serialized.return_type_id = Some(operation.return_type.as_u32());
            serialized.interface_id = operation.interface.map(|id| id.as_u32());
            serialized.namespace_id = operation.namespace.map(|id| id.as_u32());
            serialized.source_operation_id = operation.source_operation.map(|id| id.as_u32());
            serialized.decorator_node_ids = operation
                .decorators
                .iter()
                .map(|d| d.node.as_u32())
                .collect();
            serialized.template_parameter_ids = operation
                .template_parameters
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.template_argument_ids = operation
                .template_arguments
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.is_template_declaration = operation.is_template_declaration;
        }
        Type::Interface(interface) => {
            serialized.name = interface.name.clone();
            serialized.node_id = interface.node.map(|id| id.as_u32());
            serialized.symbol_id = interface.symbol.map(|id| id.as_u32());
            serialized.operation_names = interface.operations.keys().cloned().collect();
            serialized.operation_ids = interface
                .operations
                .values()
                .map(|id| id.as_u32())
                .collect();
            serialized.source_interface_ids = interface
                .source_interfaces
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.namespace_id = interface.namespace.map(|id| id.as_u32());
            serialized.decorator_node_ids = interface
                .decorators
                .iter()
                .map(|d| d.node.as_u32())
                .collect();
            serialized.template_parameter_ids = interface
                .template_parameters
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.template_argument_ids = interface
                .template_arguments
                .iter()
                .map(|id| id.as_u32())
                .collect();
            serialized.is_template_declaration = interface.is_template_declaration;
        }
        Type::Namespace(namespace) => {
            serialized.name = namespace.name.clone();
            serialized.node_id = namespace.node.map(|id| id.as_u32());
            serialized.symbol_id = namespace.symbol.map(|id| id.as_u32());
            serialized.namespace_id = namespace.namespace.map(|id| id.as_u32());
            serialized.model_names = namespace.models.keys().cloned().collect();
            serialized.model_ids = namespace.models.values().map(|id| id.as_u32()).collect();
            serialized.scalar_names = namespace.scalars.keys().cloned().collect();
            serialized.scalar_ids = namespace.scalars.values().map(|id| id.as_u32()).collect();
            serialized.enum_names = namespace.enums.keys().cloned().collect();
            serialized.enum_ids = namespace.enums.values().map(|id| id.as_u32()).collect();
            serialized.union_names = namespace.unions.keys().cloned().collect();
            serialized.union_ids = namespace.unions.values().map(|id| id.as_u32()).collect();
            serialized.alias_names = namespace.aliases.keys().cloned().collect();
            serialized.alias_ids = namespace.aliases.values().map(|id| id.as_u32()).collect();
            serialized.operation_names = namespace.operations.keys().cloned().collect();
            serialized.operation_ids = namespace
                .operations
                .values()
                .map(|id| id.as_u32())
                .collect();
            serialized.interface_names = namespace.interfaces.keys().cloned().collect();
            serialized.interface_ids = namespace
                .interfaces
                .values()
                .map(|id| id.as_u32())
                .collect();
            serialized.namespace_names = namespace.namespaces.keys().cloned().collect();
            serialized.namespace_ids = namespace
                .namespaces
                .values()
                .map(|id| id.as_u32())
                .collect();
            serialized.decorator_node_ids = namespace
                .decorators
                .iter()
                .map(|d| d.node.as_u32())
                .collect();
        }
        Type::StringLiteral(value) => {
            serialized.name = value.value.clone();
            serialized.literal_string = Some(value.value.clone());
        }
        Type::NumericLiteral(value) => {
            serialized.name = value.value.to_string();
            serialized.literal_numeric = value.value.as_number();
            serialized.literal_numeric_number = value.value.as_number();
            if let Some(bigint) = numeric_bigint(&value.value) {
                serialized.literal_numeric_bigint = Some(bigint);
            } else {
                serialized.literal_numeric_string = Some(value.value.to_string());
            }
        }
        Type::BooleanLiteral(value) => {
            serialized.name = value.value.to_string();
            serialized.literal_boolean = Some(value.value);
        }
        Type::TemplateParameter(param) => {
            serialized.name = param.name.clone();
            serialized.node_id = Some(param.node.as_u32());
            serialized.symbol_id = Some(param.symbol.as_u32());
            serialized.constraint_id = param.constraint.map(|id| id.as_u32());
            serialized.template_default_id = param.default.map(|id| id.as_u32());
        }
        _ => {}
    }
    serialized
}

fn numeric_bigint(value: &Numeric) -> Option<BigInt> {
    let bigint = value.as_bigint()?;
    let (sign_bit, words) = bigint_to_napi_words(&bigint);
    Some(BigInt { sign_bit, words })
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
