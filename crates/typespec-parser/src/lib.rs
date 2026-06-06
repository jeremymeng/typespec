#![doc = "Parser crate for the TypeSpec Rust port."]

pub mod ast;
pub mod syntax_kind;

use std::cell::RefCell;
use std::rc::Rc;

use ast::{AstArena, NamespaceBody, Node, NodeData, NodeFlags, NodeId};
use syntax_kind::SyntaxKind;
use typespec_scanner::token::{
    is_keyword, is_punctuation, is_reserved_keyword, is_statement_keyword, token_display, Token,
};
use typespec_scanner::{Scanner, TokenFlags};
use typespec_source::{Diagnostic, DiagnosticSeverity, SourceFile, Span};

#[derive(Debug)]
pub struct ParseResult {
    pub arena: AstArena,
    pub root: NodeId,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone)]
struct Annotations {
    pos: u32,
    decorators: Vec<NodeId>,
    directives: Vec<NodeId>,
    docs: Vec<NodeId>,
}

#[derive(Clone, Copy)]
enum ListKind {
    OperationParameters,
    DecoratorArguments,
    FunctionArguments,
    ModelProperties,
    ObjectLiteralProperties,
    InterfaceMembers,
    ScalarMembers,
    UnionVariants,
    EnumMembers,
    TemplateParameters,
    TemplateArguments,
    Heritage,
    Tuple,
    ArrayLiteral,
    FunctionParameters,
}

#[derive(Clone, Copy)]
struct ListSpec {
    allow_empty: bool,
    open: Option<Token>,
    close: Option<Token>,
    delimiter: Token,
    tolerated_delimiter: Token,
    tolerated_delimiter_is_valid: bool,
    invalid_annotation_target: Option<&'static str>,
    allowed_statement_keyword: Token,
}

impl ListKind {
    fn spec(self) -> ListSpec {
        let properties = ListSpec {
            allow_empty: true,
            open: Some(Token::OpenBrace),
            close: Some(Token::CloseBrace),
            delimiter: Token::Semicolon,
            tolerated_delimiter: Token::Comma,
            tolerated_delimiter_is_valid: true,
            invalid_annotation_target: None,
            allowed_statement_keyword: Token::None,
        };
        let expressions = ListSpec {
            allow_empty: true,
            open: Some(Token::OpenParen),
            close: Some(Token::CloseParen),
            delimiter: Token::Comma,
            tolerated_delimiter: Token::Semicolon,
            tolerated_delimiter_is_valid: false,
            invalid_annotation_target: Some("expression"),
            allowed_statement_keyword: Token::None,
        };
        match self {
            ListKind::OperationParameters => ListSpec {
                open: Some(Token::OpenParen),
                close: Some(Token::CloseParen),
                delimiter: Token::Comma,
                tolerated_delimiter: Token::Semicolon,
                ..properties
            },
            ListKind::DecoratorArguments | ListKind::FunctionArguments => expressions,
            ListKind::ModelProperties | ListKind::EnumMembers => properties,
            ListKind::ObjectLiteralProperties => ListSpec {
                open: Some(Token::HashBrace),
                close: Some(Token::CloseBrace),
                delimiter: Token::Comma,
                tolerated_delimiter: Token::Comma,
                invalid_annotation_target: Some("object literal property"),
                ..properties
            },
            ListKind::InterfaceMembers => ListSpec {
                tolerated_delimiter_is_valid: false,
                allowed_statement_keyword: Token::OpKeyword,
                ..properties
            },
            ListKind::ScalarMembers => ListSpec {
                tolerated_delimiter_is_valid: false,
                allowed_statement_keyword: Token::InitKeyword,
                ..properties
            },
            ListKind::UnionVariants => properties,
            ListKind::TemplateParameters | ListKind::TemplateArguments => ListSpec {
                allow_empty: false,
                open: Some(Token::LessThan),
                close: Some(Token::GreaterThan),
                invalid_annotation_target: Some("template parameter"),
                ..expressions
            },
            ListKind::Heritage => ListSpec {
                allow_empty: false,
                open: None,
                close: None,
                ..expressions
            },
            ListKind::Tuple => ListSpec {
                open: Some(Token::OpenBracket),
                close: Some(Token::CloseBracket),
                ..expressions
            },
            ListKind::ArrayLiteral => ListSpec {
                open: Some(Token::HashBracket),
                close: Some(Token::CloseBracket),
                ..expressions
            },
            ListKind::FunctionParameters => ListSpec {
                open: Some(Token::OpenParen),
                close: Some(Token::CloseParen),
                ..expressions
            },
        }
    }
}

#[derive(Clone)]
struct ListResult {
    items: Vec<NodeId>,
    range: Span,
}

pub struct Parser<'src> {
    scanner: Scanner<'src>,
    arena: AstArena,
    diagnostics: Rc<RefCell<Vec<Diagnostic>>>,
    parse_error_in_next_finished_node: bool,
    previous_token_end: i64,
    real_position_of_last_error: i64,
    missing_identifier_counter: u32,
    tree_printable: bool,
    new_line_is_trivia: bool,
}

pub fn parse(source: &SourceFile) -> ParseResult {
    let mut parser = Parser::new(source);
    let root = parser.parse_type_spec_script();
    parser.into_result(root)
}

impl<'src> Parser<'src> {
    pub fn new(source: &'src SourceFile) -> Self {
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&diagnostics);
        let mut parser = Self {
            scanner: Scanner::new(source, move |diagnostic| sink.borrow_mut().push(diagnostic)),
            arena: AstArena::new(),
            diagnostics,
            parse_error_in_next_finished_node: false,
            previous_token_end: -1,
            real_position_of_last_error: -1,
            missing_identifier_counter: 0,
            tree_printable: true,
            new_line_is_trivia: true,
        };
        parser.next_token();
        parser
    }

    fn into_result(self, root: NodeId) -> ParseResult {
        let diagnostics = self.diagnostics.borrow().clone();
        ParseResult {
            arena: self.arena,
            root,
            diagnostics,
        }
    }

    pub fn parse_type_spec_script(&mut self) -> NodeId {
        let statements = self.parse_type_spec_script_item_list();
        let id = self.add_node_with_flags(
            SyntaxKind::Identifier,
            0,
            0,
            NodeFlags::SYNTHETIC,
            NodeData::Identifier {
                sv: "<anonymous>".to_string(),
            },
        );
        let (end, flags) = self.finish_node(0);
        self.add_node_with_flags(
            SyntaxKind::TypeSpecScript,
            0,
            end,
            flags,
            NodeData::TypeSpecScript {
                id,
                statements,
                namespaces: Vec::new(),
                usings: Vec::new(),
                printable: self.tree_printable,
            },
        )
    }

    fn parse_type_spec_script_item_list(&mut self) -> Vec<NodeId> {
        let mut statements = Vec::new();
        while self.token() != Token::EndOfFile {
            let annotations = self.parse_annotations();
            let tok = self.token();
            let item = match tok {
                Token::AtAt => {
                    self.report_invalid_decorators(
                        &annotations.decorators,
                        "augment decorator statement",
                    );
                    self.parse_augment_decorator()
                }
                Token::ImportKeyword => {
                    self.report_invalid_decorators(&annotations.decorators, "import statement");
                    self.parse_import_statement()
                }
                Token::UsingKeyword => {
                    self.report_invalid_decorators(&annotations.decorators, "using statement");
                    self.parse_using_statement(annotations.pos)
                }
                Token::Semicolon => {
                    self.report_invalid_decorators(&annotations.decorators, "empty statement");
                    self.parse_empty_statement(annotations.pos)
                }
                Token::NamespaceKeyword
                | Token::ModelKeyword
                | Token::ScalarKeyword
                | Token::InterfaceKeyword
                | Token::UnionKeyword
                | Token::OpKeyword
                | Token::EnumKeyword
                | Token::AliasKeyword
                | Token::ConstKeyword
                | Token::ExternKeyword
                | Token::InternalKeyword
                | Token::FnKeyword
                | Token::DecKeyword => self.parse_declaration(
                    annotations.pos,
                    annotations.decorators,
                    annotations.docs.clone(),
                    annotations.directives.clone(),
                ),
                _ => self.parse_invalid_statement(annotations.pos, annotations.decorators),
            };
            if tok != Token::NamespaceKeyword {
                self.attach_annotations(item, annotations.docs, annotations.directives);
            }
            statements.push(item);
        }
        statements
    }

    fn parse_statement_list(&mut self) -> Vec<NodeId> {
        let mut statements = Vec::new();
        while self.token() != Token::CloseBrace {
            let annotations = self.parse_annotations();
            let tok = self.token();
            let item = match tok {
                Token::AtAt => {
                    self.report_invalid_decorators(
                        &annotations.decorators,
                        "augment decorator statement",
                    );
                    self.parse_augment_decorator()
                }
                Token::ImportKeyword => {
                    self.report_invalid_decorators(&annotations.decorators, "import statement");
                    let node = self.parse_import_statement();
                    self.error_at("import-first", self.node_span(node));
                    node
                }
                Token::UsingKeyword => {
                    self.report_invalid_decorators(&annotations.decorators, "using statement");
                    self.parse_using_statement(annotations.pos)
                }
                Token::ModelKeyword
                | Token::ScalarKeyword
                | Token::NamespaceKeyword
                | Token::InterfaceKeyword
                | Token::UnionKeyword
                | Token::OpKeyword
                | Token::EnumKeyword
                | Token::AliasKeyword
                | Token::ConstKeyword
                | Token::ExternKeyword
                | Token::InternalKeyword
                | Token::FnKeyword
                | Token::DecKeyword => self.parse_declaration(
                    annotations.pos,
                    annotations.decorators,
                    annotations.docs.clone(),
                    annotations.directives.clone(),
                ),
                Token::EndOfFile => {
                    self.parse_expected(Token::CloseBrace);
                    return statements;
                }
                Token::Semicolon => {
                    self.report_invalid_decorators(&annotations.decorators, "empty statement");
                    self.parse_empty_statement(annotations.pos)
                }
                _ => self.parse_invalid_statement(annotations.pos, annotations.decorators),
            };
            self.attach_annotations(item, annotations.docs, annotations.directives);
            statements.push(item);
        }
        statements
    }

    fn parse_annotations(&mut self) -> Annotations {
        let pos = self.token_pos();
        let mut decorators = Vec::new();
        let mut directives = Vec::new();
        while matches!(self.token(), Token::Hash | Token::At) {
            if self.token() == Token::Hash {
                directives.push(self.parse_directive_expression());
            } else {
                decorators.push(self.parse_decorator_expression());
            }
        }
        Annotations {
            pos,
            decorators,
            directives,
            docs: Vec::new(),
        }
    }

    fn parse_declaration(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        docs: Vec<NodeId>,
        directives: Vec<NodeId>,
    ) -> NodeId {
        let modifiers = self.parse_modifiers();
        match self.token() {
            Token::ModelKeyword => self.parse_model_statement(pos, decorators, modifiers),
            Token::ScalarKeyword => self.parse_scalar_statement(pos, decorators, modifiers),
            Token::NamespaceKeyword => {
                self.parse_namespace_statement(pos, decorators, modifiers, docs, directives)
            }
            Token::InterfaceKeyword => self.parse_interface_statement(pos, decorators, modifiers),
            Token::UnionKeyword => self.parse_union_statement(pos, decorators, modifiers),
            Token::OpKeyword => {
                self.parse_operation_statement(pos, decorators, Some(modifiers), false)
            }
            Token::EnumKeyword => self.parse_enum_statement(pos, decorators, modifiers),
            Token::AliasKeyword => {
                self.report_invalid_decorators(&decorators, "alias statement");
                self.parse_alias_statement(pos, modifiers)
            }
            Token::ConstKeyword => {
                self.report_invalid_decorators(&decorators, "const statement");
                self.parse_const_statement(pos, modifiers)
            }
            Token::DecKeyword => self.parse_decorator_declaration_statement(pos, modifiers),
            Token::FnKeyword => self.parse_function_declaration_statement(pos, modifiers),
            _ => self.parse_invalid_statement(pos, Vec::new()),
        }
    }

    fn parse_modifiers(&mut self) -> Vec<NodeId> {
        let mut modifiers = Vec::new();
        while let Some(modifier) = self.parse_modifier() {
            modifiers.push(modifier);
        }
        modifiers
    }

    fn parse_modifier(&mut self) -> Option<NodeId> {
        match self.token() {
            Token::ExternKeyword => {
                Some(self.parse_keyword(SyntaxKind::ExternKeyword, Token::ExternKeyword))
            }
            Token::InternalKeyword => {
                Some(self.parse_keyword(SyntaxKind::InternalKeyword, Token::InternalKeyword))
            }
            _ => None,
        }
    }

    fn modifier_flags(&self, modifiers: &[NodeId]) -> u32 {
        let mut flags = 0;
        for modifier in modifiers {
            match self.arena.get(*modifier).kind {
                SyntaxKind::ExternKeyword => flags |= 1 << 1,
                SyntaxKind::InternalKeyword => flags |= 1 << 2,
                _ => {}
            }
        }
        flags
    }

    fn parse_namespace_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
        docs: Vec<NodeId>,
        directives: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::NamespaceKeyword);
        let mut current_name = self.parse_identifier_or_member_expression(false, true, None);
        let mut segments = Vec::new();
        while self.arena.get(current_name).kind != SyntaxKind::Identifier {
            if let NodeData::MemberExpression { id, base, .. } =
                self.arena.get(current_name).data.clone()
            {
                segments.push(id);
                current_name = base;
            } else {
                break;
            }
        }
        segments.push(current_name);
        let next = self.parse_expected_one_of(&[Token::Semicolon, Token::OpenBrace]);
        let statements = if next == Token::OpenBrace {
            let statements = self.parse_statement_list();
            self.parse_expected(Token::CloseBrace);
            Some(NamespaceBody::Statements(statements))
        } else {
            None
        };

        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        let mut outer = self.add_node_with_flags(
            SyntaxKind::NamespaceStatement,
            pos,
            end,
            flags,
            NodeData::NamespaceStatement {
                id: segments[0],
                decorators,
                modifiers,
                modifier_flags,
                statements,
            },
        );
        self.attach_annotations(outer, docs, directives);

        for segment in segments.iter().skip(1) {
            let (end, flags) = self.finish_node(pos);
            outer = self.add_node_with_flags(
                SyntaxKind::NamespaceStatement,
                pos,
                end,
                flags,
                NodeData::NamespaceStatement {
                    id: *segment,
                    decorators: Vec::new(),
                    modifiers: Vec::new(),
                    modifier_flags: 0,
                    statements: Some(NamespaceBody::Nested(outer)),
                },
            );
        }
        outer
    }

    fn parse_interface_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::InterfaceKeyword);
        let id = self.parse_identifier(None, false, false);
        let template = self.parse_template_parameter_list();
        let extends = if self.parse_optional(Token::ExtendsKeyword) {
            self.parse_list(ListKind::Heritage, |parser, _, _| {
                parser.parse_reference_expression(None)
            })
            .items
        } else {
            Vec::new()
        };
        let operations = self.parse_list(ListKind::InterfaceMembers, |parser, pos, decorators| {
            parser.parse_operation_statement(pos, decorators, None, true)
        });
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::InterfaceStatement,
            pos,
            end,
            flags,
            NodeData::InterfaceStatement {
                id,
                decorators,
                modifiers,
                modifier_flags,
                template_parameters: template.items,
                template_parameters_range: template.range,
                extends,
                operations: operations.items,
                body_range: operations.range,
            },
        )
    }

    fn parse_union_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::UnionKeyword);
        let id = self.parse_identifier(None, false, false);
        let template = self.parse_template_parameter_list();
        let options = self.parse_list(ListKind::UnionVariants, |parser, pos, decorators| {
            parser.parse_union_variant(pos, decorators)
        });
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::UnionStatement,
            pos,
            end,
            flags,
            NodeData::UnionStatement {
                id,
                decorators,
                modifiers,
                modifier_flags,
                template_parameters: template.items,
                template_parameters_range: template.range,
                options: options.items,
            },
        )
    }

    fn parse_union_variant(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        let id_or_expr = self.parse_expression();
        if self.parse_optional(Token::Colon) {
            let id = self.expression_as_identifier(id_or_expr);
            let value = self.parse_expression();
            let (end, flags) = self.finish_node(pos);
            return self.add_node_with_flags(
                SyntaxKind::UnionVariant,
                pos,
                end,
                flags,
                NodeData::UnionVariant {
                    id,
                    value,
                    decorators,
                },
            );
        }
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::UnionVariant,
            pos,
            end,
            flags,
            NodeData::UnionVariant {
                id: None,
                value: id_or_expr,
                decorators,
            },
        )
    }

    fn parse_using_statement(&mut self, pos: u32) -> NodeId {
        self.parse_expected(Token::UsingKeyword);
        let name = self.parse_identifier_or_member_expression(false, false, None);
        self.parse_expected(Token::Semicolon);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::UsingStatement,
            pos,
            end,
            flags,
            NodeData::UsingStatement { name },
        )
    }

    fn parse_operation_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Option<Vec<NodeId>>,
        in_interface: bool,
    ) -> NodeId {
        let modifiers = if in_interface {
            let modifiers = self.parse_modifiers();
            self.parse_optional(Token::OpKeyword);
            modifiers
        } else {
            self.parse_expected(Token::OpKeyword);
            modifiers.unwrap_or_default()
        };
        let id = self.parse_identifier(None, false, false);
        let template = self.parse_template_parameter_list();
        let token = self.expect_token_is_one_of(&[Token::OpenParen, Token::IsKeyword]);
        let signature_pos = self.token_pos();
        let signature = if token == Token::OpenParen {
            let parameters = self.parse_operation_parameters();
            self.parse_expected(Token::Colon);
            let return_type = self.parse_expression();
            let (end, flags) = self.finish_node(signature_pos);
            self.add_node_with_flags(
                SyntaxKind::OperationSignatureDeclaration,
                signature_pos,
                end,
                flags,
                NodeData::OperationSignatureDeclaration {
                    parameters,
                    return_type,
                },
            )
        } else {
            self.parse_expected(Token::IsKeyword);
            let base_operation = self.parse_reference_expression(None);
            let (end, flags) = self.finish_node(signature_pos);
            self.add_node_with_flags(
                SyntaxKind::OperationSignatureReference,
                signature_pos,
                end,
                flags,
                NodeData::OperationSignatureReference { base_operation },
            )
        };
        if !in_interface {
            self.parse_expected(Token::Semicolon);
        }
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::OperationStatement,
            pos,
            end,
            flags,
            NodeData::OperationStatement {
                id,
                decorators,
                modifiers,
                modifier_flags,
                template_parameters: template.items,
                template_parameters_range: template.range,
                signature,
            },
        )
    }

    fn parse_operation_parameters(&mut self) -> NodeId {
        let pos = self.token_pos();
        let properties = self
            .parse_list(ListKind::OperationParameters, |parser, pos, decorators| {
                parser.parse_model_property_or_spread(pos, decorators)
            });
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ModelExpression,
            pos,
            end,
            flags,
            NodeData::ModelExpression {
                properties: properties.items,
                body_range: properties.range,
            },
        )
    }

    fn parse_model_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::ModelKeyword);
        let id = self.parse_identifier(None, false, false);
        let template = self.parse_template_parameter_list();
        self.expect_token_is_one_of(&[
            Token::OpenBrace,
            Token::Equals,
            Token::ExtendsKeyword,
            Token::IsKeyword,
        ]);
        let extends = if self.parse_optional(Token::ExtendsKeyword) {
            Some(self.parse_expression())
        } else {
            None
        };
        let is = if extends.is_none() && self.parse_optional(Token::IsKeyword) {
            Some(self.parse_expression())
        } else {
            None
        };
        let prop_detail = if is.is_some() {
            let tok = self.expect_token_is_one_of(&[Token::Semicolon, Token::OpenBrace]);
            if tok == Token::Semicolon {
                self.next_token();
                self.create_empty_list()
            } else {
                self.parse_list(ListKind::ModelProperties, |parser, pos, decorators| {
                    parser.parse_model_property_or_spread(pos, decorators)
                })
            }
        } else {
            self.parse_list(ListKind::ModelProperties, |parser, pos, decorators| {
                parser.parse_model_property_or_spread(pos, decorators)
            })
        };
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ModelStatement,
            pos,
            end,
            flags,
            NodeData::ModelStatement {
                id,
                decorators,
                modifiers,
                modifier_flags,
                template_parameters: template.items,
                template_parameters_range: template.range,
                extends,
                is,
                properties: prop_detail.items,
                body_range: prop_detail.range,
            },
        )
    }

    fn parse_template_parameter_list(&mut self) -> ListResult {
        self.parse_optional_list(ListKind::TemplateParameters, |parser, _, _| {
            parser.parse_template_parameter()
        })
    }

    fn parse_template_parameter(&mut self) -> NodeId {
        let pos = self.token_pos();
        let id = self.parse_identifier(None, false, false);
        let constraint = if self.parse_optional(Token::ExtendsKeyword) {
            Some(self.parse_mixed_constraint())
        } else {
            None
        };
        let default = if self.parse_optional(Token::Equals) {
            Some(self.parse_expression())
        } else {
            None
        };
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::TemplateParameterDeclaration,
            pos,
            end,
            flags,
            NodeData::TemplateParameterDeclaration {
                id,
                constraint,
                default,
            },
        )
    }

    fn parse_model_property_or_spread(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        if self.token() == Token::Ellipsis {
            self.parse_model_spread_property(pos, decorators)
        } else {
            self.parse_model_property(pos, decorators)
        }
    }

    fn parse_model_spread_property(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        self.parse_expected(Token::Ellipsis);
        self.report_invalid_decorators(&decorators, "spread property");
        let target = self.parse_reference_expression(None);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ModelSpreadProperty,
            pos,
            end,
            flags,
            NodeData::ModelSpreadProperty { target },
        )
    }

    fn parse_model_property(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        let id = self.parse_identifier(Some("property"), true, true);
        let optional = self.parse_optional(Token::Question);
        self.parse_expected(Token::Colon);
        let value = self.parse_expression();
        let default = if self.parse_optional(Token::Equals) {
            Some(self.parse_expression())
        } else {
            None
        };
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ModelProperty,
            pos,
            end,
            flags,
            NodeData::ModelProperty {
                id,
                value,
                decorators,
                optional,
                default,
            },
        )
    }

    fn parse_scalar_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::ScalarKeyword);
        let id = self.parse_identifier(None, false, false);
        let template = self.parse_template_parameter_list();
        let extends = if self.parse_optional(Token::ExtendsKeyword) {
            Some(self.parse_reference_expression(None))
        } else {
            None
        };
        let members = if self.token() == Token::Semicolon {
            self.next_token();
            self.create_empty_list()
        } else {
            self.parse_list(ListKind::ScalarMembers, |parser, pos, decorators| {
                parser.parse_scalar_member(pos, decorators)
            })
        };
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ScalarStatement,
            pos,
            end,
            flags,
            NodeData::ScalarStatement {
                id,
                decorators,
                modifiers,
                modifier_flags,
                template_parameters: template.items,
                template_parameters_range: template.range,
                extends,
                members: members.items,
                body_range: members.range,
            },
        )
    }

    fn parse_scalar_member(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        self.report_invalid_decorators(&decorators, "scalar member");
        self.parse_expected(Token::InitKeyword);
        let id = self.parse_identifier(None, false, false);
        let parameters = self.parse_function_parameters();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ScalarConstructor,
            pos,
            end,
            flags,
            NodeData::ScalarConstructor {
                id,
                parameters: parameters.items,
            },
        )
    }

    fn parse_enum_statement(
        &mut self,
        pos: u32,
        decorators: Vec<NodeId>,
        modifiers: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::EnumKeyword);
        let id = self.parse_identifier(None, false, false);
        let members = self.parse_list(ListKind::EnumMembers, |parser, pos, decorators| {
            parser.parse_enum_member_or_spread(pos, decorators)
        });
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::EnumStatement,
            pos,
            end,
            flags,
            NodeData::EnumStatement {
                id,
                decorators,
                modifiers,
                modifier_flags,
                members: members.items,
            },
        )
    }

    fn parse_enum_member_or_spread(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        if self.token() == Token::Ellipsis {
            self.parse_enum_spread_member(pos, decorators)
        } else {
            self.parse_enum_member(pos, decorators)
        }
    }

    fn parse_enum_spread_member(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        self.parse_expected(Token::Ellipsis);
        self.report_invalid_decorators(&decorators, "spread enum");
        let target = self.parse_reference_expression(None);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::EnumSpreadMember,
            pos,
            end,
            flags,
            NodeData::EnumSpreadMember { target },
        )
    }

    fn parse_enum_member(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        let id = self.parse_identifier(Some("enumMember"), true, true);
        let value = if self.parse_optional(Token::Colon) {
            let expr = self.parse_expression();
            if matches!(
                self.arena.get(expr).kind,
                SyntaxKind::StringLiteral | SyntaxKind::NumericLiteral
            ) {
                Some(expr)
            } else {
                self.error_at("token-expected", self.node_span(expr));
                None
            }
        } else {
            None
        };
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::EnumMember,
            pos,
            end,
            flags,
            NodeData::EnumMember {
                id,
                value,
                decorators,
            },
        )
    }

    fn parse_alias_statement(&mut self, pos: u32, modifiers: Vec<NodeId>) -> NodeId {
        self.parse_expected(Token::AliasKeyword);
        let id = self.parse_identifier(None, false, false);
        let template = self.parse_template_parameter_list();
        self.parse_expected(Token::Equals);
        let value = self.parse_expression();
        self.parse_expected(Token::Semicolon);
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::AliasStatement,
            pos,
            end,
            flags,
            NodeData::AliasStatement {
                id,
                modifiers,
                modifier_flags,
                template_parameters: template.items,
                template_parameters_range: template.range,
                value,
            },
        )
    }

    fn parse_const_statement(&mut self, pos: u32, modifiers: Vec<NodeId>) -> NodeId {
        self.parse_expected(Token::ConstKeyword);
        let id = self.parse_identifier(None, false, false);
        let type_ref = if self.parse_optional(Token::Colon) {
            Some(self.parse_expression())
        } else {
            None
        };
        self.parse_expected(Token::Equals);
        let value = self.parse_expression();
        self.parse_expected(Token::Semicolon);
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ConstStatement,
            pos,
            end,
            flags,
            NodeData::ConstStatement {
                id,
                modifiers,
                modifier_flags,
                value,
                type_ref,
            },
        )
    }

    fn parse_expression(&mut self) -> NodeId {
        self.parse_union_expression_or_higher()
    }

    fn parse_union_expression_or_higher(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_optional(Token::Bar);
        let node = self.parse_intersection_expression_or_higher();
        if self.token() != Token::Bar {
            return node;
        }
        let mut options = vec![node];
        while self.parse_optional(Token::Bar) {
            options.push(self.parse_intersection_expression_or_higher());
        }
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::UnionExpression,
            pos,
            end,
            flags,
            NodeData::UnionExpression { options },
        )
    }

    fn parse_intersection_expression_or_higher(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_optional(Token::Ampersand);
        let node = self.parse_array_expression_or_higher();
        if self.token() != Token::Ampersand {
            return node;
        }
        let mut options = vec![node];
        while self.parse_optional(Token::Ampersand) {
            options.push(self.parse_array_expression_or_higher());
        }
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::IntersectionExpression,
            pos,
            end,
            flags,
            NodeData::IntersectionExpression { options },
        )
    }

    fn parse_array_expression_or_higher(&mut self) -> NodeId {
        let pos = self.token_pos();
        let mut expr = self.parse_primary_expression();
        while self.parse_optional(Token::OpenBracket) {
            self.parse_expected(Token::CloseBracket);
            let (end, flags) = self.finish_node(pos);
            expr = self.add_node_with_flags(
                SyntaxKind::ArrayExpression,
                pos,
                end,
                flags,
                NodeData::ArrayExpression { element_type: expr },
            );
        }
        expr
    }

    fn parse_mixed_constraint(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_optional(Token::Bar);
        let node = if self.token() == Token::ValueOfKeyword {
            self.parse_value_of_expression()
        } else if self.parse_optional(Token::OpenParen) {
            let expr = self.parse_mixed_constraint();
            self.parse_expected(Token::CloseParen);
            expr
        } else {
            self.parse_intersection_expression_or_higher()
        };
        if self.token() != Token::Bar {
            return node;
        }
        let mut options = vec![node];
        while self.parse_optional(Token::Bar) {
            options.push(if self.token() == Token::ValueOfKeyword {
                self.parse_value_of_expression()
            } else {
                self.parse_intersection_expression_or_higher()
            });
        }
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::UnionExpression,
            pos,
            end,
            flags,
            NodeData::UnionExpression { options },
        )
    }

    fn parse_value_of_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::ValueOfKeyword);
        let target = self.parse_expression();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ValueOfExpression,
            pos,
            end,
            flags,
            NodeData::ValueOfExpression { target },
        )
    }

    fn parse_type_of_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::TypeOfKeyword);
        let target = self.parse_type_of_target();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::TypeOfExpression,
            pos,
            end,
            flags,
            NodeData::TypeOfExpression { target },
        )
    }

    fn parse_type_of_target(&mut self) -> NodeId {
        match self.token() {
            Token::TypeOfKeyword => self.parse_type_of_expression(),
            Token::Identifier => self.parse_call_or_reference_expression(None),
            Token::StringLiteral => self.parse_string_literal(),
            Token::StringTemplateHead => self.parse_string_template_expression(),
            Token::TrueKeyword | Token::FalseKeyword => self.parse_boolean_literal(),
            Token::NumericLiteral => self.parse_numeric_literal(),
            Token::OpenParen => {
                self.parse_expected(Token::OpenParen);
                let target = self.parse_type_of_target();
                self.parse_expected(Token::CloseParen);
                target
            }
            _ => self.parse_reference_expression(Some("typeofTarget")),
        }
    }

    fn parse_function_type_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::FnKeyword);
        let parameters = self.parse_function_parameters();
        let return_type = if self.parse_optional(Token::EqualsGreaterThan) {
            Some(self.parse_mixed_constraint())
        } else {
            None
        };
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::FunctionTypeExpression,
            pos,
            end,
            flags,
            NodeData::FunctionTypeExpression {
                parameters: parameters.items,
                return_type,
            },
        )
    }

    fn parse_reference_expression(&mut self, message: Option<&'static str>) -> NodeId {
        let pos = self.token_pos();
        let target = self.parse_identifier_or_member_expression(false, true, message);
        self.parse_reference_expression_internal(target, pos)
    }

    fn parse_call_or_reference_expression(&mut self, message: Option<&'static str>) -> NodeId {
        let pos = self.token_pos();
        let target = self.parse_identifier_or_member_expression(false, true, message);
        if self.token() == Token::OpenParen {
            let args = self.parse_list(ListKind::FunctionArguments, |parser, _, _| {
                parser.parse_expression()
            });
            let (end, flags) = self.finish_node(pos);
            return self.add_node_with_flags(
                SyntaxKind::CallExpression,
                pos,
                end,
                flags,
                NodeData::CallExpression {
                    target,
                    arguments: args.items,
                },
            );
        }
        self.parse_reference_expression_internal(target, pos)
    }

    fn parse_reference_expression_internal(&mut self, target: NodeId, pos: u32) -> NodeId {
        let args = self.parse_optional_list(ListKind::TemplateArguments, |parser, _, _| {
            parser.parse_template_argument()
        });
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::TypeReference,
            pos,
            end,
            flags,
            NodeData::TypeReference {
                target,
                arguments: args.items,
            },
        )
    }

    fn parse_template_argument(&mut self) -> NodeId {
        let pos = self.token_pos();
        if self.token() == Token::Equals {
            self.error("token-expected", Some("identifier"));
            self.next_token();
            let name = self.create_missing_identifier();
            let argument = self.parse_expression();
            let (end, flags) = self.finish_node(pos);
            return self.add_node_with_flags(
                SyntaxKind::TemplateArgument,
                pos,
                end,
                flags,
                NodeData::TemplateArgument {
                    name: Some(name),
                    argument,
                },
            );
        }
        let expr = self.parse_expression();
        if self.parse_optional(Token::Equals) {
            let name = self.expression_as_identifier(expr);
            if name.is_none() {
                self.error_at("invalid-template-argument-name", self.node_span(expr));
            }
            let name = name.unwrap_or_else(|| self.create_missing_identifier());
            let argument = self.parse_expression();
            let (end, flags) = self.finish_node(pos);
            self.add_node_with_flags(
                SyntaxKind::TemplateArgument,
                pos,
                end,
                flags,
                NodeData::TemplateArgument {
                    name: Some(name),
                    argument,
                },
            )
        } else {
            let (end, flags) = self.finish_node(pos);
            self.add_node_with_flags(
                SyntaxKind::TemplateArgument,
                pos,
                end,
                flags,
                NodeData::TemplateArgument {
                    name: None,
                    argument: expr,
                },
            )
        }
    }

    fn parse_augment_decorator(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::AtAt);
        let target = self.parse_identifier_or_member_expression(true, true, None);
        let args = self.parse_optional_list(ListKind::DecoratorArguments, |parser, _, _| {
            parser.parse_expression()
        });
        let (target_type, arguments) = if args.items.is_empty() {
            self.error("augment-decorator-target", None);
            (self.create_missing_type_reference(), Vec::new())
        } else {
            let mut items = args.items;
            let target_type = items.remove(0);
            if self.arena.get(target_type).kind != SyntaxKind::TypeReference {
                self.error_at("augment-decorator-target", self.node_span(target_type));
                (self.create_missing_type_reference(), items)
            } else {
                (target_type, items)
            }
        };
        self.parse_expected(Token::Semicolon);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::AugmentDecoratorStatement,
            pos,
            end,
            flags,
            NodeData::AugmentDecoratorStatement {
                target,
                target_type,
                arguments,
            },
        )
    }

    fn parse_import_statement(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::ImportKeyword);
        let path = self.parse_string_literal();
        self.parse_expected(Token::Semicolon);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ImportStatement,
            pos,
            end,
            flags,
            NodeData::ImportStatement { path },
        )
    }

    fn parse_decorator_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::At);
        let target = self.parse_identifier_or_member_expression(true, true, None);
        let args = self.parse_optional_list(ListKind::DecoratorArguments, |parser, _, _| {
            parser.parse_expression()
        });
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::DecoratorExpression,
            pos,
            end,
            flags,
            NodeData::DecoratorExpression {
                target,
                arguments: args.items,
            },
        )
    }

    fn parse_directive_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::Hash);
        let target = self.parse_identifier(None, false, false);
        self.new_line_is_trivia = false;
        let mut arguments = Vec::new();
        while self.token() != Token::NewLine && self.token() != Token::EndOfFile {
            match self.token() {
                Token::Identifier => arguments.push(self.parse_identifier(None, false, false)),
                Token::StringLiteral => arguments.push(self.parse_string_literal()),
                _ => {
                    self.error("token-expected", Some("unexpected"));
                    self.next_token();
                }
            }
        }
        self.new_line_is_trivia = true;
        self.next_token();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::DirectiveExpression,
            pos,
            end,
            flags,
            NodeData::DirectiveExpression { target, arguments },
        )
    }

    fn parse_identifier_or_member_expression(
        &mut self,
        allow_reserved_identifier: bool,
        allow_reserved_identifier_in_member: bool,
        message: Option<&'static str>,
    ) -> NodeId {
        let pos = self.token_pos();
        let mut base = self.parse_identifier(message, false, allow_reserved_identifier);
        while self.token() != Token::EndOfFile {
            if self.parse_optional(Token::Dot) {
                let id = self.parse_identifier(None, false, allow_reserved_identifier_in_member);
                let (end, flags) = self.finish_node(pos);
                base = self.add_node_with_flags(
                    SyntaxKind::MemberExpression,
                    pos,
                    end,
                    flags,
                    NodeData::MemberExpression {
                        base,
                        id,
                        selector: ".".to_string(),
                    },
                );
            } else if self.parse_optional(Token::ColonColon) {
                let id = self.parse_identifier(None, false, false);
                let (end, flags) = self.finish_node(pos);
                base = self.add_node_with_flags(
                    SyntaxKind::MemberExpression,
                    pos,
                    end,
                    flags,
                    NodeData::MemberExpression {
                        base,
                        id,
                        selector: "::".to_string(),
                    },
                );
            } else {
                break;
            }
        }
        base
    }

    fn parse_primary_expression(&mut self) -> NodeId {
        loop {
            match self.token() {
                Token::TypeOfKeyword => return self.parse_type_of_expression(),
                Token::FnKeyword => return self.parse_function_type_expression(),
                Token::Identifier => return self.parse_call_or_reference_expression(None),
                Token::StringLiteral => return self.parse_string_literal(),
                Token::StringTemplateHead => return self.parse_string_template_expression(),
                Token::TrueKeyword | Token::FalseKeyword => return self.parse_boolean_literal(),
                Token::NumericLiteral => return self.parse_numeric_literal(),
                Token::OpenBrace => return self.parse_model_expression(),
                Token::OpenBracket => return self.parse_tuple_expression(),
                Token::OpenParen => return self.parse_parenthesized_expression(),
                Token::At => {
                    let decorators = self.parse_decorator_list();
                    self.report_invalid_decorators(&decorators, "expression");
                }
                Token::Hash => {
                    let directives = self.parse_directive_list();
                    self.report_invalid_directive(&directives, "expression");
                }
                Token::HashBrace => return self.parse_object_literal(),
                Token::HashBracket => return self.parse_array_literal(),
                Token::VoidKeyword => {
                    return self.parse_keyword(SyntaxKind::VoidKeyword, Token::VoidKeyword)
                }
                Token::NeverKeyword => {
                    return self.parse_keyword(SyntaxKind::NeverKeyword, Token::NeverKeyword);
                }
                Token::UnknownKeyword => {
                    return self.parse_keyword(SyntaxKind::UnknownKeyword, Token::UnknownKeyword);
                }
                _ => return self.parse_reference_expression(Some("expression")),
            }
        }
    }

    fn parse_parenthesized_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(Token::OpenParen);
        let expr = self.parse_expression();
        self.parse_expected(Token::CloseParen);
        let end = self.previous_end_u32();
        let flags = self.arena.get(expr).flags;
        self.arena.get_mut(expr).pos = pos;
        self.arena.get_mut(expr).end = end;
        self.arena.get_mut(expr).flags = flags;
        expr
    }

    fn parse_tuple_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        let values = self.parse_list(ListKind::Tuple, |parser, _, _| parser.parse_expression());
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::TupleExpression,
            pos,
            end,
            flags,
            NodeData::TupleExpression {
                values: values.items,
            },
        )
    }

    fn parse_model_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        let properties = self.parse_list(ListKind::ModelProperties, |parser, pos, decorators| {
            parser.parse_model_property_or_spread(pos, decorators)
        });
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ModelExpression,
            pos,
            end,
            flags,
            NodeData::ModelExpression {
                properties: properties.items,
                body_range: properties.range,
            },
        )
    }

    fn parse_object_literal(&mut self) -> NodeId {
        let pos = self.token_pos();
        let properties = self.parse_list(
            ListKind::ObjectLiteralProperties,
            |parser, pos, decorators| {
                parser.report_invalid_decorators(&decorators, "object literal property");
                if parser.token() == Token::Ellipsis {
                    parser.parse_object_literal_spread_property(pos)
                } else {
                    parser.parse_object_literal_property(pos)
                }
            },
        );
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ObjectLiteral,
            pos,
            end,
            flags,
            NodeData::ObjectLiteral {
                properties: properties.items,
                body_range: properties.range,
            },
        )
    }

    fn parse_object_literal_spread_property(&mut self, pos: u32) -> NodeId {
        self.parse_expected(Token::Ellipsis);
        let target = self.parse_reference_expression(None);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ObjectLiteralSpreadProperty,
            pos,
            end,
            flags,
            NodeData::ObjectLiteralSpreadProperty { target },
        )
    }

    fn parse_object_literal_property(&mut self, pos: u32) -> NodeId {
        let id = self.parse_identifier(Some("property"), false, true);
        self.parse_expected(Token::Colon);
        let value = self.parse_expression();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ObjectLiteralProperty,
            pos,
            end,
            flags,
            NodeData::ObjectLiteralProperty { id, value },
        )
    }

    fn parse_array_literal(&mut self) -> NodeId {
        let pos = self.token_pos();
        let values = self.parse_list(ListKind::ArrayLiteral, |parser, _, _| {
            parser.parse_expression()
        });
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::ArrayLiteral,
            pos,
            end,
            flags,
            NodeData::ArrayLiteral {
                values: values.items,
            },
        )
    }

    fn parse_string_literal(&mut self) -> NodeId {
        let pos = self.token_pos();
        let value = self.current_token_value();
        self.parse_expected(Token::StringLiteral);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::StringLiteral,
            pos,
            end,
            flags,
            NodeData::StringLiteral { value },
        )
    }

    fn parse_string_template_expression(&mut self) -> NodeId {
        let pos = self.token_pos();
        let head = self.parse_string_template_head();
        let head_flags = match &self.arena.get(head).data {
            NodeData::StringTemplateLiteral { token_flags, .. } => *token_flags,
            _ => 0,
        };
        let mut spans = Vec::new();
        loop {
            let span = self.parse_template_type_span(TokenFlags::NONE);
            let literal = match &self.arena.get(span).data {
                NodeData::StringTemplateSpan { literal, .. } => *literal,
                _ => unreachable!(),
            };
            let done = self.arena.get(literal).kind == SyntaxKind::StringTemplateTail;
            spans.push(span);
            if done || head_flags == u32::MAX {
                break;
            }
        }
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::StringTemplateExpression,
            pos,
            end,
            flags,
            NodeData::StringTemplateExpression { head, spans },
        )
    }

    fn parse_string_template_head(&mut self) -> NodeId {
        let pos = self.token_pos();
        let flags_bits = self.scanner.token_flags().bits();
        let value = self.current_token_value();
        self.parse_expected(Token::StringTemplateHead);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::StringTemplateHead,
            pos,
            end,
            flags,
            NodeData::StringTemplateLiteral {
                value,
                token_flags: flags_bits,
            },
        )
    }

    fn parse_template_type_span(&mut self, _head_token_flags: TokenFlags) -> NodeId {
        let pos = self.token_pos();
        let expression = self.parse_expression();
        let literal = self.parse_literal_of_template_span(TokenFlags::NONE);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::StringTemplateSpan,
            pos,
            end,
            flags,
            NodeData::StringTemplateSpan {
                expression,
                literal,
            },
        )
    }

    fn parse_literal_of_template_span(&mut self, token_flags: TokenFlags) -> NodeId {
        if self.token() == Token::CloseBrace {
            self.scanner.rescan_string_template(token_flags);
            return self.parse_template_middle_or_tail();
        }
        self.parse_template_tail()
    }

    fn parse_template_middle_or_tail(&mut self) -> NodeId {
        let pos = self.token_pos();
        let flags_bits = self.scanner.token_flags().bits();
        let value = self.current_token_value();
        let kind = if self.token() == Token::StringTemplateMiddle {
            SyntaxKind::StringTemplateMiddle
        } else {
            SyntaxKind::StringTemplateTail
        };
        self.next_token();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            kind,
            pos,
            end,
            flags,
            NodeData::StringTemplateLiteral {
                value,
                token_flags: flags_bits,
            },
        )
    }

    fn parse_template_tail(&mut self) -> NodeId {
        let pos = self.token_pos();
        let flags_bits = self.scanner.token_flags().bits();
        let value = self.current_token_value();
        self.parse_expected(Token::StringTemplateTail);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::StringTemplateTail,
            pos,
            end,
            flags,
            NodeData::StringTemplateLiteral {
                value,
                token_flags: flags_bits,
            },
        )
    }

    fn parse_numeric_literal(&mut self) -> NodeId {
        let pos = self.token_pos();
        let value_as_string = self.current_token_value();
        let value = parse_js_number(&value_as_string);
        self.parse_expected(Token::NumericLiteral);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::NumericLiteral,
            pos,
            end,
            flags,
            NodeData::NumericLiteral {
                value,
                value_as_string,
            },
        )
    }

    fn parse_boolean_literal(&mut self) -> NodeId {
        let pos = self.token_pos();
        let tok = self.parse_expected_one_of(&[Token::TrueKeyword, Token::FalseKeyword]);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::BooleanLiteral,
            pos,
            end,
            flags,
            NodeData::BooleanLiteral {
                value: tok == Token::TrueKeyword,
            },
        )
    }

    fn parse_identifier(
        &mut self,
        message: Option<&'static str>,
        allow_string_literal: bool,
        allow_reserved_identifier: bool,
    ) -> NodeId {
        if is_keyword(self.token()) {
            if !(is_modifier(self.token()) && allow_reserved_identifier) {
                self.error("reserved-identifier", None);
                return self.create_missing_identifier();
            }
        } else if is_reserved_keyword(self.token()) {
            if !allow_reserved_identifier {
                self.error("reserved-identifier", None);
            }
        } else if self.token() != Token::Identifier
            && (!allow_string_literal || self.token() != Token::StringLiteral)
        {
            self.error("token-expected", message.or(Some("identifier")));
            return self.create_missing_identifier();
        }
        let pos = self.token_pos();
        let sv = self.current_token_value();
        self.next_token();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::Identifier,
            pos,
            end,
            flags,
            NodeData::Identifier { sv },
        )
    }

    fn parse_decorator_declaration_statement(
        &mut self,
        pos: u32,
        modifiers: Vec<NodeId>,
    ) -> NodeId {
        self.parse_expected(Token::DecKeyword);
        let id = self.parse_identifier(None, false, false);
        let all_params = self.parse_function_parameters();
        let mut params = all_params.items;
        let target = if params.is_empty() {
            self.error_at(
                "decorator-decl-target",
                Span {
                    pos,
                    end: self.previous_end_u32(),
                },
            );
            let missing_id = self.create_missing_identifier();
            let missing_type = self.create_missing_type_reference();
            let (end, flags) = self.finish_node(pos);
            self.add_node_with_flags(
                SyntaxKind::FunctionParameter,
                pos,
                end,
                flags,
                NodeData::FunctionParameter {
                    id: missing_id,
                    type_ref: Some(missing_type),
                    optional: false,
                    rest: false,
                },
            )
        } else {
            params.remove(0)
        };
        self.parse_expected(Token::Semicolon);
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::DecoratorDeclarationStatement,
            pos,
            end,
            flags,
            NodeData::DecoratorDeclarationStatement {
                id,
                modifiers,
                modifier_flags,
                target,
                parameters: params,
            },
        )
    }

    fn parse_function_declaration_statement(&mut self, pos: u32, modifiers: Vec<NodeId>) -> NodeId {
        self.parse_expected(Token::FnKeyword);
        let id = self.parse_identifier(None, false, false);
        let parameters = self.parse_function_parameters();
        let return_type = if self.parse_optional(Token::Colon) {
            Some(self.parse_mixed_constraint())
        } else {
            None
        };
        self.parse_expected(Token::Semicolon);
        let modifier_flags = self.modifier_flags(&modifiers);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::FunctionDeclarationStatement,
            pos,
            end,
            flags,
            NodeData::FunctionDeclarationStatement {
                id,
                modifiers,
                modifier_flags,
                parameters: parameters.items,
                return_type,
            },
        )
    }

    fn parse_function_parameters(&mut self) -> ListResult {
        self.parse_list(ListKind::FunctionParameters, |parser, _, _| {
            parser.parse_function_parameter()
        })
    }

    fn parse_function_parameter(&mut self) -> NodeId {
        let pos = self.token_pos();
        let rest = self.parse_optional(Token::Ellipsis);
        let id = self.parse_identifier(Some("property"), false, true);
        let optional = self.parse_optional(Token::Question);
        let type_ref = if self.parse_optional(Token::Colon) {
            Some(self.parse_mixed_constraint())
        } else {
            None
        };
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::FunctionParameter,
            pos,
            end,
            flags,
            NodeData::FunctionParameter {
                id,
                type_ref,
                optional,
                rest,
            },
        )
    }

    fn parse_keyword(&mut self, kind: SyntaxKind, expected: Token) -> NodeId {
        let pos = self.token_pos();
        self.parse_expected(expected);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(kind, pos, end, flags, NodeData::Keyword)
    }

    fn parse_decorator_list(&mut self) -> Vec<NodeId> {
        let mut decorators = Vec::new();
        while self.token() == Token::At {
            decorators.push(self.parse_decorator_expression());
        }
        decorators
    }

    fn parse_directive_list(&mut self) -> Vec<NodeId> {
        let mut directives = Vec::new();
        while self.token() == Token::Hash {
            directives.push(self.parse_directive_expression());
        }
        directives
    }

    fn parse_list<F>(&mut self, kind: ListKind, mut parse_item: F) -> ListResult
    where
        F: FnMut(&mut Self, u32, Vec<NodeId>) -> NodeId,
    {
        let spec = kind.spec();
        let mut result = self.create_empty_list();
        if let Some(open) = spec.open {
            let t = self.token_pos();
            if self.parse_expected(open) {
                result.range.pos = t;
            }
        }
        if spec.allow_empty {
            if let Some(close) = spec.close {
                if self.parse_optional(close) {
                    result.range.end = self.previous_end_u32();
                    return result;
                }
            }
        }
        loop {
            let starting_pos = self.token_pos();
            let annotations = self.parse_annotations();
            if let Some(target) = spec.invalid_annotation_target {
                self.report_invalid_decorators(&annotations.decorators, target);
                self.report_invalid_directive(&annotations.directives, target);
            }
            if annotations.directives.is_empty()
                && annotations.decorators.is_empty()
                && self.at_end_of_list_with_error(spec)
            {
                if let Some(close) = spec.close {
                    if self.parse_expected(close) {
                        result.range.end = self.previous_end_u32();
                    }
                }
                break;
            }
            let item = parse_item(self, annotations.pos, annotations.decorators);
            if spec.invalid_annotation_target.is_none() {
                self.attach_annotations(item, annotations.docs, annotations.directives);
            }
            result.items.push(item);
            if self.parse_optional_delimiter(spec) {
                if let Some(close) = spec.close {
                    if self.parse_optional(close) {
                        result.range.end = self.previous_end_u32();
                        break;
                    }
                }
                continue;
            } else if spec.close.is_none() {
                break;
            } else if self.parse_optional(spec.close.unwrap()) {
                result.range.end = self.previous_end_u32();
                break;
            } else if self.at_end_of_list_with_error(spec) {
                if let Some(close) = spec.close {
                    if self.parse_expected(close) {
                        result.range.end = self.previous_end_u32();
                    }
                }
                break;
            } else {
                self.parse_expected(spec.delimiter);
            }
            if starting_pos == self.token_pos() {
                if let Some(close) = spec.close {
                    if self.parse_expected(close) {
                        result.range.end = self.previous_end_u32();
                    }
                }
                self.next_token();
                result.items.pop();
                break;
            }
        }
        result
    }

    fn parse_optional_list<F>(&mut self, kind: ListKind, parse_item: F) -> ListResult
    where
        F: FnMut(&mut Self, u32, Vec<NodeId>) -> NodeId,
    {
        let spec = kind.spec();
        if spec.open.is_some_and(|open| self.token() == open) {
            self.parse_list(kind, parse_item)
        } else {
            self.create_empty_list()
        }
    }

    fn parse_optional_delimiter(&mut self, spec: ListSpec) -> bool {
        if self.parse_optional(spec.delimiter) {
            return true;
        }
        if self.token() == spec.tolerated_delimiter {
            if !spec.tolerated_delimiter_is_valid {
                self.parse_expected(spec.delimiter);
            }
            self.next_token();
            return true;
        }
        false
    }

    fn at_end_of_list_with_error(&self, spec: ListSpec) -> bool {
        spec.close.is_some()
            && (is_statement_keyword(self.token()) || self.token() == Token::EndOfFile)
            && self.token() != spec.allowed_statement_keyword
    }

    fn parse_empty_statement(&mut self, pos: u32) -> NodeId {
        self.parse_expected(Token::Semicolon);
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::EmptyStatement,
            pos,
            end,
            flags,
            NodeData::EmptyStatement,
        )
    }

    fn parse_invalid_statement(&mut self, pos: u32, decorators: Vec<NodeId>) -> NodeId {
        loop {
            self.next_token();
            if is_statement_keyword(self.token())
                || self.token() == Token::At
                || self.token() == Token::Semicolon
                || self.token() == Token::EndOfFile
            {
                break;
            }
        }
        self.error_at(
            "token-expected",
            Span {
                pos,
                end: self.previous_end_u32(),
            },
        );
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::InvalidStatement,
            pos,
            end,
            flags,
            NodeData::InvalidStatement { decorators },
        )
    }

    fn token(&self) -> Token {
        self.scanner.token()
    }

    fn token_pos(&self) -> u32 {
        self.scanner.token_position()
    }

    fn token_end(&self) -> u32 {
        self.scanner.position()
    }

    fn current_token_value(&mut self) -> String {
        self.scanner
            .token_value()
            .unwrap_or_else(|| self.scanner.token_text())
    }

    fn next_token(&mut self) -> Token {
        self.previous_token_end = self.scanner.position() as i64;
        loop {
            let token = self.scanner.scan();
            if !self.is_trivia(token) {
                return token;
            }
            if !self.new_line_is_trivia && token == Token::NewLine {
                return token;
            }
        }
    }

    fn is_trivia(&self, token: Token) -> bool {
        matches!(
            token,
            Token::Whitespace
                | Token::NewLine
                | Token::SingleLineComment
                | Token::MultiLineComment
                | Token::ConflictMarker
        )
    }

    fn create_missing_identifier(&mut self) -> NodeId {
        let pos = self.token_pos();
        self.previous_token_end = pos as i64;
        self.missing_identifier_counter += 1;
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::Identifier,
            pos,
            end,
            flags,
            NodeData::Identifier {
                sv: format!("<missing identifier>{}", self.missing_identifier_counter),
            },
        )
    }

    fn create_missing_type_reference(&mut self) -> NodeId {
        let pos = self.token_pos();
        let target = self.create_missing_identifier();
        let (end, flags) = self.finish_node(pos);
        self.add_node_with_flags(
            SyntaxKind::TypeReference,
            pos,
            end,
            flags,
            NodeData::TypeReference {
                target,
                arguments: Vec::new(),
            },
        )
    }

    fn finish_node(&mut self, pos: u32) -> (u32, NodeFlags) {
        let flags = if self.parse_error_in_next_finished_node {
            NodeFlags::THIS_NODE_HAS_ERROR
        } else {
            NodeFlags::NONE
        };
        self.parse_error_in_next_finished_node = false;
        (self.previous_end_u32().max(pos), flags)
    }

    fn previous_end_u32(&self) -> u32 {
        if self.previous_token_end < 0 {
            0
        } else {
            self.previous_token_end as u32
        }
    }

    fn add_node_with_flags(
        &mut self,
        kind: SyntaxKind,
        pos: u32,
        end: u32,
        flags: NodeFlags,
        data: NodeData,
    ) -> NodeId {
        self.arena.add(Node::new(kind, pos, end, flags, data))
    }

    fn create_empty_list(&self) -> ListResult {
        ListResult {
            items: Vec::new(),
            range: Span {
                pos: u32::MAX,
                end: u32::MAX,
            },
        }
    }

    fn attach_annotations(&mut self, node: NodeId, docs: Vec<NodeId>, directives: Vec<NodeId>) {
        let node = self.arena.get_mut(node);
        node.docs = docs;
        node.directives = directives;
    }

    fn expression_as_identifier(&mut self, expr: NodeId) -> Option<NodeId> {
        match self.arena.get(expr).data.clone() {
            NodeData::TypeReference { target, arguments } if arguments.is_empty() => {
                if self.arena.get(target).kind == SyntaxKind::Identifier {
                    Some(target)
                } else {
                    None
                }
            }
            NodeData::StringLiteral { value } => {
                let span = self.node_span(expr);
                Some(self.add_node_with_flags(
                    SyntaxKind::Identifier,
                    span.pos,
                    span.end,
                    self.arena.get(expr).flags,
                    NodeData::Identifier { sv: value },
                ))
            }
            _ => None,
        }
    }

    fn node_span(&self, node: NodeId) -> Span {
        let node = self.arena.get(node);
        Span {
            pos: node.pos,
            end: node.end,
        }
    }

    fn parse_expected(&mut self, expected: Token) -> bool {
        if self.token() == expected {
            self.next_token();
            return true;
        }
        let target = if is_punctuation(expected) {
            Some(Span {
                pos: self.previous_end_u32(),
                end: self.previous_end_u32() + 1,
            })
        } else {
            None
        };
        self.error("token-expected", Some(token_display(expected)));
        if let Some(target) = target {
            self.retarget_last_diagnostic(target);
        }
        false
    }

    fn expect_token_is_one_of(&mut self, expected: &[Token]) -> Token {
        let tok = self.token();
        if expected.contains(&tok) {
            tok
        } else {
            self.error("token-expected", Some("one of expected tokens"));
            Token::None
        }
    }

    fn parse_expected_one_of(&mut self, expected: &[Token]) -> Token {
        let tok = self.expect_token_is_one_of(expected);
        if tok != Token::None {
            self.next_token();
        }
        tok
    }

    fn parse_optional(&mut self, optional: Token) -> bool {
        if self.token() == optional {
            self.next_token();
            true
        } else {
            false
        }
    }

    fn error(&mut self, code: &'static str, detail: Option<&str>) {
        let span = Span {
            pos: self.token_pos(),
            end: self.token_end(),
        };
        self.error_at_with_message(code, span, detail);
    }

    fn error_at(&mut self, code: &'static str, span: Span) {
        self.error_at_with_message(code, span, None);
    }

    fn error_at_with_message(&mut self, code: &'static str, span: Span, detail: Option<&str>) {
        self.parse_error_in_next_finished_node = true;
        self.tree_printable = false;
        if self.real_position_of_last_error == span.pos as i64 {
            return;
        }
        self.real_position_of_last_error = span.pos as i64;
        let message = match detail {
            Some(detail) => format!("{code}: {detail}"),
            None => code.to_string(),
        };
        self.diagnostics.borrow_mut().push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code,
            message,
            target: span,
        });
    }

    fn retarget_last_diagnostic(&mut self, span: Span) {
        if let Some(last) = self.diagnostics.borrow_mut().last_mut() {
            last.target = span;
        }
    }

    fn report_invalid_decorators(&mut self, decorators: &[NodeId], node_name: &str) {
        for decorator in decorators {
            self.error_at_with_message(
                "invalid-decorator-location",
                self.node_span(*decorator),
                Some(node_name),
            );
        }
    }

    fn report_invalid_directive(&mut self, directives: &[NodeId], node_name: &str) {
        for directive in directives {
            self.error_at_with_message(
                "invalid-directive-location",
                self.node_span(*directive),
                Some(node_name),
            );
        }
    }
}

fn is_modifier(token: Token) -> bool {
    matches!(token, Token::ExternKeyword | Token::InternalKeyword)
}

fn parse_js_number(text: &str) -> f64 {
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN)
    } else if let Some(binary) = text.strip_prefix("0b") {
        u64::from_str_radix(binary, 2)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN)
    } else {
        text.parse::<f64>().unwrap_or(f64::NAN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_source(source: &str) -> ParseResult {
        parse(&SourceFile::new(source.to_string()))
    }

    #[test]
    fn builds_arena_node() {
        let mut arena = AstArena::new();
        let id = arena.add(Node::new(
            SyntaxKind::Identifier,
            0,
            3,
            NodeFlags::NONE,
            NodeData::Identifier {
                sv: "Foo".to_string(),
            },
        ));
        assert_eq!(id.as_u32(), 0);
        assert_eq!(arena.get(id).kind, SyntaxKind::Identifier);
    }

    #[test]
    fn parses_empty_file() {
        let result = parse_source("");
        assert_eq!(
            result.arena.get(result.root).kind,
            SyntaxKind::TypeSpecScript
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_model_with_property() {
        let result = parse_source("model Foo { name: string; }");
        assert!(result.diagnostics.is_empty());
        let root = result.arena.get(result.root);
        let NodeData::TypeSpecScript { statements, .. } = &root.data else {
            panic!("expected script");
        };
        assert_eq!(
            result.arena.get(statements[0]).kind,
            SyntaxKind::ModelStatement
        );
    }

    #[test]
    fn parses_enum_with_member() {
        let result = parse_source("enum Color { red, blue }");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn recovers_missing_model_close_brace() {
        let result = parse_source("model Foo { name: string;");
        assert!(!result.diagnostics.is_empty());
    }
}
