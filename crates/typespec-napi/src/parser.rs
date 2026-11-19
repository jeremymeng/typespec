use napi_derive::napi;
use typespec_parser::ast::{NamespaceBody, NodeData};
use typespec_parser::{parse_with_options, ParseOptions as RustParseOptions};
use typespec_source::{Diagnostic, DiagnosticSeverity, SourceFile};

use crate::scanner::DiagnosticInfo;

#[napi(object)]
pub struct SerializedNode {
    pub kind: u32,
    pub pos: u32,
    pub end: u32,
    pub flags: u32,
    pub children: Vec<u32>,
    pub sv: Option<String>,
    pub value: Option<String>,
    pub value_as_string: Option<String>,
    pub numeric_value: Option<f64>,
    pub boolean_value: Option<bool>,
    pub id: Option<u32>,
    pub path: Option<u32>,
    pub target: Option<u32>,
    pub target_type: Option<u32>,
    pub base: Option<u32>,
    pub selector: Option<String>,
    pub name: Option<u32>,
    pub argument: Option<u32>,
    pub arguments: Option<Vec<u32>>,
    pub statements: Option<Vec<u32>>,
    pub decorators: Option<Vec<u32>>,
    pub directives: Option<Vec<u32>>,
    pub docs: Option<Vec<u32>>,
    pub modifiers: Option<Vec<u32>>,
    pub modifier_flags: Option<u32>,
    pub template_parameters: Option<Vec<u32>>,
    pub template_parameters_range_pos: Option<u32>,
    pub template_parameters_range_end: Option<u32>,
    pub properties: Option<Vec<u32>>,
    pub body_range_pos: Option<u32>,
    pub body_range_end: Option<u32>,
    pub extends: Option<Vec<u32>>,
    pub extends_single: Option<u32>,
    pub is: Option<u32>,
    pub operations: Option<Vec<u32>>,
    pub options: Option<Vec<u32>>,
    pub members: Option<Vec<u32>>,
    pub value_node: Option<u32>,
    pub default: Option<u32>,
    pub optional: Option<bool>,
    pub element_type: Option<u32>,
    pub values: Option<Vec<u32>>,
    pub head: Option<u32>,
    pub spans: Option<Vec<u32>>,
    pub expression: Option<u32>,
    pub literal: Option<u32>,
    pub token_flags: Option<u32>,
    pub constraint: Option<u32>,
    pub type_ref: Option<u32>,
    pub parameters: Option<Vec<u32>>,
    pub return_type: Option<u32>,
    pub rest: Option<bool>,
    pub parsed_as_docs: Option<bool>,
}

#[napi(object)]
pub struct SerializedAst {
    pub root: u32,
    pub nodes: Vec<SerializedNode>,
    pub diagnostics: Vec<DiagnosticInfo>,
    pub comments: Vec<u32>,
}

#[napi(object)]
#[derive(Default)]
pub struct ParseOptions {
    pub comments: Option<bool>,
    pub docs: Option<bool>,
}

#[napi]
pub fn parse(source: String, options: Option<ParseOptions>) -> SerializedAst {
    let source = SourceFile::new(source);
    let options = options.unwrap_or_default();
    let result = parse_with_options(
        &source,
        RustParseOptions {
            comments: options.comments.unwrap_or(false),
            docs: options.docs.unwrap_or(true),
        },
    );
    let comments = result.comments.iter().map(|id| id.as_u32()).collect();
    let nodes = result.arena.nodes().iter().map(serialize_node).collect();
    SerializedAst {
        root: result.root.as_u32(),
        nodes,
        diagnostics: result
            .diagnostics
            .into_iter()
            .map(convert_diagnostic)
            .collect(),
        comments,
    }
}

fn serialize_node(node: &typespec_parser::ast::Node) -> SerializedNode {
    let mut serialized = SerializedNode {
        kind: node.kind as u32,
        pos: node.pos,
        end: node.end,
        flags: node.flags.bits(),
        children: node.children().into_iter().map(|id| id.as_u32()).collect(),
        sv: None,
        value: None,
        value_as_string: None,
        numeric_value: None,
        boolean_value: None,
        id: None,
        path: None,
        target: None,
        target_type: None,
        base: None,
        selector: None,
        name: None,
        argument: None,
        arguments: None,
        statements: None,
        decorators: None,
        directives: none_if_empty(&node.directives),
        docs: none_if_empty(&node.docs),
        modifiers: None,
        modifier_flags: None,
        template_parameters: None,
        template_parameters_range_pos: None,
        template_parameters_range_end: None,
        properties: None,
        body_range_pos: None,
        body_range_end: None,
        extends: None,
        extends_single: None,
        is: None,
        operations: None,
        options: None,
        members: None,
        value_node: None,
        default: None,
        optional: None,
        element_type: None,
        values: None,
        head: None,
        spans: None,
        expression: None,
        literal: None,
        token_flags: None,
        constraint: None,
        type_ref: None,
        parameters: None,
        return_type: None,
        rest: None,
        parsed_as_docs: None,
    };

    match &node.data {
        NodeData::TypeSpecScript { id, statements, .. } => {
            serialized.id = Some(id.as_u32());
            serialized.statements = some_ids(statements);
        }
        NodeData::ImportStatement { path } => serialized.path = Some(path.as_u32()),
        NodeData::Identifier { sv } => serialized.sv = Some(sv.clone()),
        NodeData::AugmentDecoratorStatement {
            target,
            target_type,
            arguments,
        } => {
            serialized.target = Some(target.as_u32());
            serialized.target_type = Some(target_type.as_u32());
            serialized.arguments = some_ids(arguments);
        }
        NodeData::DecoratorExpression { target, arguments }
        | NodeData::CallExpression { target, arguments } => {
            serialized.target = Some(target.as_u32());
            serialized.arguments = some_ids(arguments);
        }
        NodeData::DirectiveExpression { target, arguments } => {
            serialized.target = Some(target.as_u32());
            serialized.arguments = some_ids(arguments);
        }
        NodeData::MemberExpression { base, id, selector } => {
            serialized.base = Some(base.as_u32());
            serialized.id = Some(id.as_u32());
            serialized.selector = Some(selector.clone());
        }
        NodeData::NamespaceStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            statements,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            if let Some(body) = statements {
                match body {
                    NamespaceBody::Statements(statements) => {
                        serialized.statements = some_ids(statements)
                    }
                    NamespaceBody::Nested(node) => {
                        serialized.statements = Some(vec![node.as_u32()])
                    }
                }
            }
        }
        NodeData::UsingStatement { name } => serialized.name = Some(name.as_u32()),
        NodeData::OperationStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            template_parameters,
            template_parameters_range,
            signature,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.value_node = Some(signature.as_u32());
        }
        NodeData::OperationSignatureDeclaration {
            parameters,
            return_type,
        } => {
            serialized.parameters = Some(vec![parameters.as_u32()]);
            serialized.return_type = Some(return_type.as_u32());
        }
        NodeData::OperationSignatureReference { base_operation } => {
            serialized.target = Some(base_operation.as_u32());
        }
        NodeData::ModelStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            template_parameters,
            template_parameters_range,
            extends,
            is,
            properties,
            body_range,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends_single = extends.map(|id| id.as_u32());
            serialized.is = is.map(|id| id.as_u32());
            serialized.properties = some_ids(properties);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::ModelExpression {
            properties,
            body_range,
        } => {
            serialized.properties = some_ids(properties);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::ModelProperty {
            id,
            value,
            decorators,
            optional,
            default,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.value_node = Some(value.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.optional = Some(*optional);
            serialized.default = default.map(|id| id.as_u32());
        }
        NodeData::ModelSpreadProperty { target }
        | NodeData::EnumSpreadMember { target }
        | NodeData::ObjectLiteralSpreadProperty { target }
        | NodeData::ValueOfExpression { target }
        | NodeData::TypeOfExpression { target } => serialized.target = Some(target.as_u32()),
        NodeData::ScalarStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            template_parameters,
            template_parameters_range,
            extends,
            members,
            body_range,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends_single = extends.map(|id| id.as_u32());
            serialized.members = some_ids(members);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::ScalarConstructor { id, parameters } => {
            serialized.id = Some(id.as_u32());
            serialized.parameters = some_ids(parameters);
        }
        NodeData::InterfaceStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            template_parameters,
            template_parameters_range,
            extends,
            operations,
            body_range,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends = some_ids(extends);
            serialized.operations = some_ids(operations);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::UnionStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            template_parameters,
            template_parameters_range,
            extends,
            options,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends_single = extends.map(|id| id.as_u32());
            serialized.options = some_ids(options);
        }
        NodeData::ModelDeclarationExpression {
            id,
            decorators,
            template_parameters,
            template_parameters_range,
            extends,
            is,
            properties,
            body_range,
        } => {
            serialized.id = id.map(|id| id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends_single = extends.map(|id| id.as_u32());
            serialized.is = is.map(|id| id.as_u32());
            serialized.properties = some_ids(properties);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::ScalarDeclarationExpression {
            id,
            decorators,
            template_parameters,
            template_parameters_range,
            extends,
            members,
            body_range,
        } => {
            serialized.id = id.map(|id| id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends_single = extends.map(|id| id.as_u32());
            serialized.members = some_ids(members);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::UnionDeclarationExpression {
            id,
            decorators,
            template_parameters,
            template_parameters_range,
            extends,
            options,
        } => {
            serialized.id = id.map(|id| id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.extends_single = extends.map(|id| id.as_u32());
            serialized.options = some_ids(options);
        }
        NodeData::EnumDeclarationExpression {
            id,
            decorators,
            members,
        } => {
            serialized.id = id.map(|id| id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.members = some_ids(members);
        }
        NodeData::UnionVariant {
            id,
            value,
            decorators,
        } => {
            serialized.id = id.map(|id| id.as_u32());
            serialized.value_node = Some(value.as_u32());
            serialized.decorators = some_ids(decorators);
        }
        NodeData::EnumStatement {
            id,
            decorators,
            modifiers,
            modifier_flags,
            members,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.decorators = some_ids(decorators);
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.members = some_ids(members);
        }
        NodeData::EnumMember {
            id,
            value,
            decorators,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.value_node = value.map(|id| id.as_u32());
            serialized.decorators = some_ids(decorators);
        }
        NodeData::AliasStatement {
            id,
            modifiers,
            modifier_flags,
            template_parameters,
            template_parameters_range,
            value,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.template_parameters = some_ids(template_parameters);
            serialized.template_parameters_range_pos = Some(template_parameters_range.pos);
            serialized.template_parameters_range_end = Some(template_parameters_range.end);
            serialized.value_node = Some(value.as_u32());
        }
        NodeData::ConstStatement {
            id,
            modifiers,
            modifier_flags,
            value,
            type_ref,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.value_node = Some(value.as_u32());
            serialized.type_ref = type_ref.map(|id| id.as_u32());
        }
        NodeData::InvalidStatement { decorators } => serialized.decorators = some_ids(decorators),
        NodeData::EmptyStatement | NodeData::Keyword => {}
        NodeData::UnionExpression { options } | NodeData::IntersectionExpression { options } => {
            serialized.options = some_ids(options);
        }
        NodeData::TupleExpression { values } | NodeData::ArrayLiteral { values } => {
            serialized.values = some_ids(values);
        }
        NodeData::ArrayExpression { element_type } => {
            serialized.element_type = Some(element_type.as_u32())
        }
        NodeData::StringLiteral { value } => serialized.value = Some(value.clone()),
        NodeData::NumericLiteral {
            value,
            value_as_string,
        } => {
            serialized.numeric_value = Some(*value);
            serialized.value_as_string = Some(value_as_string.clone());
        }
        NodeData::BooleanLiteral { value } => serialized.boolean_value = Some(*value),
        NodeData::StringTemplateExpression { head, spans } => {
            serialized.head = Some(head.as_u32());
            serialized.spans = some_ids(spans);
        }
        NodeData::StringTemplateSpan {
            expression,
            literal,
        } => {
            serialized.expression = Some(expression.as_u32());
            serialized.literal = Some(literal.as_u32());
        }
        NodeData::StringTemplateLiteral { value, token_flags } => {
            serialized.value = Some(value.clone());
            serialized.token_flags = Some(*token_flags);
        }
        NodeData::TypeReference { target, arguments } => {
            serialized.target = Some(target.as_u32());
            serialized.arguments = some_ids(arguments);
        }
        NodeData::TemplateArgument { name, argument } => {
            serialized.name = name.map(|id| id.as_u32());
            serialized.argument = Some(argument.as_u32());
        }
        NodeData::TemplateParameterDeclaration {
            id,
            constraint,
            default,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.constraint = constraint.map(|id| id.as_u32());
            serialized.default = default.map(|id| id.as_u32());
        }
        NodeData::ObjectLiteral {
            properties,
            body_range,
        } => {
            serialized.properties = some_ids(properties);
            serialized.body_range_pos = Some(body_range.pos);
            serialized.body_range_end = Some(body_range.end);
        }
        NodeData::ObjectLiteralProperty { id, value } => {
            serialized.id = Some(id.as_u32());
            serialized.value_node = Some(value.as_u32());
        }
        NodeData::DecoratorDeclarationStatement {
            id,
            modifiers,
            modifier_flags,
            target,
            parameters,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.target = Some(target.as_u32());
            serialized.parameters = some_ids(parameters);
        }
        NodeData::FunctionDeclarationStatement {
            id,
            modifiers,
            modifier_flags,
            parameters,
            return_type,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.modifiers = some_ids(modifiers);
            serialized.modifier_flags = Some(*modifier_flags);
            serialized.parameters = some_ids(parameters);
            serialized.return_type = return_type.map(|id| id.as_u32());
        }
        NodeData::FunctionParameter {
            id,
            type_ref,
            optional,
            rest,
        } => {
            serialized.id = Some(id.as_u32());
            serialized.type_ref = type_ref.map(|id| id.as_u32());
            serialized.optional = Some(*optional);
            serialized.rest = Some(*rest);
        }
        NodeData::FunctionTypeExpression {
            parameters,
            return_type,
        } => {
            serialized.parameters = some_ids(parameters);
            serialized.return_type = return_type.map(|id| id.as_u32());
        }
        NodeData::Doc { content, tags } => {
            serialized.values = some_ids(content);
            serialized.arguments = some_ids(tags);
        }
        NodeData::DocText { text } => serialized.value = Some(text.clone()),
        NodeData::DocTag {
            tag_name,
            param_name,
            content,
        } => {
            serialized.target = Some(tag_name.as_u32());
            serialized.name = param_name.map(|id| id.as_u32());
            serialized.values = some_ids(content);
        }
        NodeData::Comment { parsed_as_docs } => {
            serialized.parsed_as_docs = parsed_as_docs.then_some(true);
        }
    }
    serialized
}

fn some_ids(ids: &[typespec_parser::ast::NodeId]) -> Option<Vec<u32>> {
    if ids.is_empty() {
        None
    } else {
        Some(ids.iter().map(|id| id.as_u32()).collect())
    }
}

fn none_if_empty(ids: &[typespec_parser::ast::NodeId]) -> Option<Vec<u32>> {
    some_ids(ids)
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
