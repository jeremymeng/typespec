use bumpalo::Bump;
use typespec_source::Span;

use crate::syntax_kind::SyntaxKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

impl NodeId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeFlags(u32);

impl NodeFlags {
    pub const NONE: Self = Self(0);
    pub const DESCENDANT_ERRORS_EXAMINED: Self = Self(1 << 0);
    pub const THIS_NODE_HAS_ERROR: Self = Self(1 << 1);
    pub const DESCENDANT_HAS_ERROR: Self = Self(1 << 2);
    pub const SYNTHETIC: Self = Self(1 << 3);

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Debug)]
pub struct AstArena {
    pub bump: Bump,
    nodes: Vec<Node>,
}

impl AstArena {
    pub fn new() -> Self {
        Self {
            bump: Bump::new(),
            nodes: Vec::new(),
        }
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        let id = NodeId::new(self.nodes.len() as u32);
        self.nodes.push(node);
        id
    }

    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Shift `pos`/`end` of every node allocated at or after `from` by `offset`.
    /// Used to move a freshly parsed file into the program position space.
    pub fn shift_positions(&mut self, from: usize, offset: u32) {
        if offset == 0 {
            return;
        }
        for node in &mut self.nodes[from..] {
            node.pos += offset;
            node.end += offset;
        }
    }
}

impl Default for AstArena {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: SyntaxKind,
    pub pos: u32,
    pub end: u32,
    pub flags: NodeFlags,
    pub directives: Vec<NodeId>,
    pub docs: Vec<NodeId>,
    pub data: NodeData,
}

impl Node {
    pub fn new(kind: SyntaxKind, pos: u32, end: u32, flags: NodeFlags, data: NodeData) -> Self {
        Self {
            kind,
            pos,
            end,
            flags,
            directives: Vec::new(),
            docs: Vec::new(),
            data,
        }
    }

    pub fn children(&self) -> Vec<NodeId> {
        let mut children = Vec::new();
        children.extend(self.directives.iter().copied());
        children.extend(self.docs.iter().copied());
        self.data.push_children(&mut children);
        children
    }
}

#[derive(Debug, Clone)]
pub enum NamespaceBody {
    Statements(Vec<NodeId>),
    Nested(NodeId),
}

#[derive(Debug, Clone)]
pub enum NodeData {
    TypeSpecScript {
        id: NodeId,
        statements: Vec<NodeId>,
        namespaces: Vec<NodeId>,
        usings: Vec<NodeId>,
        printable: bool,
    },
    ImportStatement {
        path: NodeId,
    },
    Identifier {
        sv: String,
    },
    AugmentDecoratorStatement {
        target: NodeId,
        target_type: NodeId,
        arguments: Vec<NodeId>,
    },
    DecoratorExpression {
        target: NodeId,
        arguments: Vec<NodeId>,
    },
    DirectiveExpression {
        target: NodeId,
        arguments: Vec<NodeId>,
    },
    MemberExpression {
        base: NodeId,
        id: NodeId,
        selector: String,
    },
    NamespaceStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        statements: Option<NamespaceBody>,
    },
    UsingStatement {
        name: NodeId,
    },
    OperationStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        signature: NodeId,
    },
    OperationSignatureDeclaration {
        parameters: NodeId,
        return_type: NodeId,
    },
    OperationSignatureReference {
        base_operation: NodeId,
    },
    ModelStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Option<NodeId>,
        is: Option<NodeId>,
        properties: Vec<NodeId>,
        body_range: Span,
    },
    ModelExpression {
        properties: Vec<NodeId>,
        body_range: Span,
    },
    ModelProperty {
        id: NodeId,
        value: NodeId,
        decorators: Vec<NodeId>,
        optional: bool,
        default: Option<NodeId>,
    },
    ModelSpreadProperty {
        target: NodeId,
    },
    ScalarStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Option<NodeId>,
        members: Vec<NodeId>,
        body_range: Span,
    },
    ScalarConstructor {
        id: NodeId,
        parameters: Vec<NodeId>,
    },
    InterfaceStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Vec<NodeId>,
        operations: Vec<NodeId>,
        body_range: Span,
    },
    UnionStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Option<NodeId>,
        options: Vec<NodeId>,
    },
    ModelDeclarationExpression {
        id: Option<NodeId>,
        decorators: Vec<NodeId>,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Option<NodeId>,
        is: Option<NodeId>,
        properties: Vec<NodeId>,
        body_range: Span,
    },
    ScalarDeclarationExpression {
        id: Option<NodeId>,
        decorators: Vec<NodeId>,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Option<NodeId>,
        members: Vec<NodeId>,
        body_range: Span,
    },
    UnionDeclarationExpression {
        id: Option<NodeId>,
        decorators: Vec<NodeId>,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        extends: Option<NodeId>,
        options: Vec<NodeId>,
    },
    EnumDeclarationExpression {
        id: Option<NodeId>,
        decorators: Vec<NodeId>,
        members: Vec<NodeId>,
    },
    UnionVariant {
        id: Option<NodeId>,
        value: NodeId,
        decorators: Vec<NodeId>,
    },
    EnumStatement {
        id: NodeId,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        members: Vec<NodeId>,
    },
    EnumMember {
        id: NodeId,
        value: Option<NodeId>,
        decorators: Vec<NodeId>,
    },
    EnumSpreadMember {
        target: NodeId,
    },
    AliasStatement {
        id: NodeId,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        template_parameters: Vec<NodeId>,
        template_parameters_range: Span,
        value: NodeId,
    },
    ConstStatement {
        id: NodeId,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        value: NodeId,
        type_ref: Option<NodeId>,
    },
    CallExpression {
        target: NodeId,
        arguments: Vec<NodeId>,
    },
    InvalidStatement {
        decorators: Vec<NodeId>,
    },
    EmptyStatement,
    UnionExpression {
        options: Vec<NodeId>,
    },
    IntersectionExpression {
        options: Vec<NodeId>,
    },
    TupleExpression {
        values: Vec<NodeId>,
    },
    ArrayExpression {
        element_type: NodeId,
    },
    StringLiteral {
        value: String,
    },
    NumericLiteral {
        value: f64,
        value_as_string: String,
    },
    BooleanLiteral {
        value: bool,
    },
    StringTemplateExpression {
        head: NodeId,
        spans: Vec<NodeId>,
    },
    StringTemplateSpan {
        expression: NodeId,
        literal: NodeId,
    },
    StringTemplateLiteral {
        value: String,
        token_flags: u32,
    },
    Keyword,
    ValueOfExpression {
        target: NodeId,
    },
    TypeOfExpression {
        target: NodeId,
    },
    TypeReference {
        target: NodeId,
        arguments: Vec<NodeId>,
    },
    TemplateArgument {
        name: Option<NodeId>,
        argument: NodeId,
    },
    TemplateParameterDeclaration {
        id: NodeId,
        constraint: Option<NodeId>,
        default: Option<NodeId>,
    },
    ObjectLiteral {
        properties: Vec<NodeId>,
        body_range: Span,
    },
    ObjectLiteralProperty {
        id: NodeId,
        value: NodeId,
    },
    ObjectLiteralSpreadProperty {
        target: NodeId,
    },
    ArrayLiteral {
        values: Vec<NodeId>,
    },
    DecoratorDeclarationStatement {
        id: NodeId,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        target: NodeId,
        parameters: Vec<NodeId>,
    },
    FunctionDeclarationStatement {
        id: NodeId,
        modifiers: Vec<NodeId>,
        modifier_flags: u32,
        parameters: Vec<NodeId>,
        return_type: Option<NodeId>,
    },
    FunctionParameter {
        id: NodeId,
        type_ref: Option<NodeId>,
        optional: bool,
        rest: bool,
    },
    FunctionTypeExpression {
        parameters: Vec<NodeId>,
        return_type: Option<NodeId>,
    },
    Doc {
        content: Vec<NodeId>,
        tags: Vec<NodeId>,
    },
    DocText {
        text: String,
    },
    DocTag {
        tag_name: NodeId,
        param_name: Option<NodeId>,
        content: Vec<NodeId>,
    },
    Comment {
        parsed_as_docs: bool,
    },
}

impl NodeData {
    /// Node TS `getSourceLocation(target, { locateId: true })` reports instead of the whole
    /// node: a type reference's target, otherwise the node's `id` (M13).
    pub fn locate_id(&self) -> Option<NodeId> {
        match self {
            NodeData::TypeReference { target, .. } => Some(*target),
            NodeData::TypeSpecScript { id, .. }
            | NodeData::MemberExpression { id, .. }
            | NodeData::NamespaceStatement { id, .. }
            | NodeData::OperationStatement { id, .. }
            | NodeData::ModelStatement { id, .. }
            | NodeData::ModelProperty { id, .. }
            | NodeData::ScalarStatement { id, .. }
            | NodeData::ScalarConstructor { id, .. }
            | NodeData::InterfaceStatement { id, .. }
            | NodeData::UnionStatement { id, .. }
            | NodeData::EnumStatement { id, .. }
            | NodeData::EnumMember { id, .. }
            | NodeData::AliasStatement { id, .. }
            | NodeData::ConstStatement { id, .. }
            | NodeData::TemplateParameterDeclaration { id, .. }
            | NodeData::ObjectLiteralProperty { id, .. }
            | NodeData::DecoratorDeclarationStatement { id, .. }
            | NodeData::FunctionDeclarationStatement { id, .. }
            | NodeData::FunctionParameter { id, .. } => Some(*id),
            NodeData::ModelDeclarationExpression { id, .. }
            | NodeData::ScalarDeclarationExpression { id, .. }
            | NodeData::UnionDeclarationExpression { id, .. }
            | NodeData::EnumDeclarationExpression { id, .. }
            | NodeData::UnionVariant { id, .. } => *id,
            _ => None,
        }
    }

    pub fn push_children(&self, children: &mut Vec<NodeId>) {
        match self {
            NodeData::TypeSpecScript { id, statements, .. } => {
                children.push(*id);
                children.extend(statements.iter().copied());
            }
            NodeData::ImportStatement { path } => children.push(*path),
            NodeData::Identifier { .. } => {}
            NodeData::AugmentDecoratorStatement {
                target,
                target_type,
                arguments,
            } => {
                children.push(*target);
                children.push(*target_type);
                children.extend(arguments.iter().copied());
            }
            NodeData::DecoratorExpression { target, arguments }
            | NodeData::CallExpression { target, arguments } => {
                children.push(*target);
                children.extend(arguments.iter().copied());
            }
            NodeData::DirectiveExpression { target, arguments } => {
                children.push(*target);
                children.extend(arguments.iter().copied());
            }
            NodeData::MemberExpression { base, id, .. } => {
                children.push(*base);
                children.push(*id);
            }
            NodeData::NamespaceStatement {
                modifiers,
                decorators,
                id,
                statements,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                if let Some(body) = statements {
                    match body {
                        NamespaceBody::Statements(statements) => {
                            children.extend(statements.iter().copied());
                        }
                        NamespaceBody::Nested(node) => children.push(*node),
                    }
                }
            }
            NodeData::UsingStatement { name } => children.push(*name),
            NodeData::OperationStatement {
                modifiers,
                decorators,
                id,
                template_parameters,
                signature,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.extend(template_parameters.iter().copied());
                children.push(*signature);
            }
            NodeData::OperationSignatureDeclaration {
                parameters,
                return_type,
            } => {
                children.push(*parameters);
                children.push(*return_type);
            }
            NodeData::OperationSignatureReference { base_operation } => {
                children.push(*base_operation)
            }
            NodeData::ModelStatement {
                modifiers,
                decorators,
                id,
                template_parameters,
                extends,
                is,
                properties,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.extend(template_parameters.iter().copied());
                if let Some(node) = extends {
                    children.push(*node);
                }
                if let Some(node) = is {
                    children.push(*node);
                }
                children.extend(properties.iter().copied());
            }
            NodeData::ModelExpression { properties, .. } => {
                children.extend(properties.iter().copied());
            }
            NodeData::ModelProperty {
                decorators,
                id,
                value,
                default,
                ..
            } => {
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.push(*value);
                if let Some(node) = default {
                    children.push(*node);
                }
            }
            NodeData::ModelSpreadProperty { target }
            | NodeData::EnumSpreadMember { target }
            | NodeData::ObjectLiteralSpreadProperty { target }
            | NodeData::ValueOfExpression { target }
            | NodeData::TypeOfExpression { target } => children.push(*target),
            NodeData::ScalarStatement {
                modifiers,
                decorators,
                id,
                template_parameters,
                members,
                extends,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.extend(template_parameters.iter().copied());
                children.extend(members.iter().copied());
                if let Some(node) = extends {
                    children.push(*node);
                }
            }
            NodeData::ScalarConstructor { id, parameters } => {
                children.push(*id);
                children.extend(parameters.iter().copied());
            }
            NodeData::InterfaceStatement {
                modifiers,
                decorators,
                id,
                template_parameters,
                extends,
                operations,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.extend(template_parameters.iter().copied());
                children.extend(extends.iter().copied());
                children.extend(operations.iter().copied());
            }
            NodeData::UnionStatement {
                modifiers,
                decorators,
                id,
                template_parameters,
                extends,
                options,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.extend(template_parameters.iter().copied());
                children.extend(*extends);
                children.extend(options.iter().copied());
            }
            NodeData::ModelDeclarationExpression {
                decorators,
                id,
                template_parameters,
                extends,
                is,
                properties,
                ..
            } => {
                children.extend(decorators.iter().copied());
                children.extend(*id);
                children.extend(template_parameters.iter().copied());
                children.extend(*extends);
                children.extend(*is);
                children.extend(properties.iter().copied());
            }
            NodeData::ScalarDeclarationExpression {
                decorators,
                id,
                template_parameters,
                members,
                extends,
                ..
            } => {
                children.extend(decorators.iter().copied());
                children.extend(*id);
                children.extend(template_parameters.iter().copied());
                children.extend(members.iter().copied());
                children.extend(*extends);
            }
            NodeData::UnionDeclarationExpression {
                decorators,
                id,
                template_parameters,
                extends,
                options,
                ..
            } => {
                children.extend(decorators.iter().copied());
                children.extend(*id);
                children.extend(template_parameters.iter().copied());
                children.extend(*extends);
                children.extend(options.iter().copied());
            }
            NodeData::EnumDeclarationExpression {
                decorators,
                id,
                members,
            } => {
                children.extend(decorators.iter().copied());
                children.extend(*id);
                children.extend(members.iter().copied());
            }
            NodeData::UnionVariant {
                decorators,
                id,
                value,
            } => {
                children.extend(decorators.iter().copied());
                if let Some(node) = id {
                    children.push(*node);
                }
                children.push(*value);
            }
            NodeData::EnumStatement {
                modifiers,
                decorators,
                id,
                members,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.extend(decorators.iter().copied());
                children.push(*id);
                children.extend(members.iter().copied());
            }
            NodeData::EnumMember {
                decorators,
                id,
                value,
            } => {
                children.extend(decorators.iter().copied());
                children.push(*id);
                if let Some(node) = value {
                    children.push(*node);
                }
            }
            NodeData::AliasStatement {
                modifiers,
                id,
                template_parameters,
                value,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.push(*id);
                children.extend(template_parameters.iter().copied());
                children.push(*value);
            }
            NodeData::ConstStatement {
                modifiers,
                id,
                value,
                type_ref,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.push(*id);
                children.push(*value);
                if let Some(node) = type_ref {
                    children.push(*node);
                }
            }
            NodeData::InvalidStatement { decorators } => {
                children.extend(decorators.iter().copied());
            }
            NodeData::EmptyStatement
            | NodeData::StringLiteral { .. }
            | NodeData::NumericLiteral { .. }
            | NodeData::BooleanLiteral { .. }
            | NodeData::StringTemplateLiteral { .. }
            | NodeData::Keyword
            | NodeData::DocText { .. }
            | NodeData::Comment { .. } => {}
            NodeData::UnionExpression { options }
            | NodeData::IntersectionExpression { options } => {
                children.extend(options.iter().copied());
            }
            NodeData::TupleExpression { values } | NodeData::ArrayLiteral { values } => {
                children.extend(values.iter().copied());
            }
            NodeData::ArrayExpression { element_type } => children.push(*element_type),
            NodeData::StringTemplateExpression { head, spans } => {
                children.push(*head);
                children.extend(spans.iter().copied());
            }
            NodeData::StringTemplateSpan {
                expression,
                literal,
            } => {
                children.push(*expression);
                children.push(*literal);
            }
            NodeData::TypeReference { target, arguments } => {
                children.push(*target);
                children.extend(arguments.iter().copied());
            }
            NodeData::TemplateArgument { name, argument } => {
                if let Some(node) = name {
                    children.push(*node);
                }
                children.push(*argument);
            }
            NodeData::TemplateParameterDeclaration {
                id,
                constraint,
                default,
            } => {
                children.push(*id);
                if let Some(node) = constraint {
                    children.push(*node);
                }
                if let Some(node) = default {
                    children.push(*node);
                }
            }
            NodeData::ObjectLiteral { properties, .. } => {
                children.extend(properties.iter().copied());
            }
            NodeData::ObjectLiteralProperty { id, value } => {
                children.push(*id);
                children.push(*value);
            }
            NodeData::DecoratorDeclarationStatement {
                modifiers,
                id,
                target,
                parameters,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.push(*id);
                children.push(*target);
                children.extend(parameters.iter().copied());
            }
            NodeData::FunctionDeclarationStatement {
                modifiers,
                id,
                parameters,
                return_type,
                ..
            } => {
                children.extend(modifiers.iter().copied());
                children.push(*id);
                children.extend(parameters.iter().copied());
                if let Some(node) = return_type {
                    children.push(*node);
                }
            }
            NodeData::FunctionParameter { id, type_ref, .. } => {
                children.push(*id);
                if let Some(node) = type_ref {
                    children.push(*node);
                }
            }
            NodeData::FunctionTypeExpression {
                parameters,
                return_type,
            } => {
                children.extend(parameters.iter().copied());
                if let Some(node) = return_type {
                    children.push(*node);
                }
            }
            NodeData::Doc { content, tags } => {
                children.extend(content.iter().copied());
                children.extend(tags.iter().copied());
            }
            NodeData::DocTag {
                tag_name,
                param_name,
                content,
            } => {
                children.push(*tag_name);
                if let Some(node) = param_name {
                    children.push(*node);
                }
                children.extend(content.iter().copied());
            }
        }
    }
}
