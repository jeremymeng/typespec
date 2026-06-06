use std::collections::{HashMap, HashSet};

use typespec_parser::ast::{NamespaceBody, NodeData, NodeId};
use typespec_parser::ParseResult;
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use crate::types::{ScopeId, SymbolFlags, SymbolId, SymbolTable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionResult {
    Resolved(SymbolId),
    Ambiguous(Vec<SymbolId>),
    Unknown,
}

#[derive(Debug, Clone)]
pub struct NameResolution {
    pub resolutions: HashMap<NodeId, ResolutionResult>,
    pub diagnostics: Vec<Diagnostic>,
    pub unused_usings: Vec<NodeId>,
}

pub fn resolve_names(parse_result: &ParseResult, table: &mut SymbolTable) -> NameResolution {
    let mut resolver = NameResolver::new(parse_result, table);
    resolver.resolve();
    resolver.finish()
}

pub struct NameResolver<'p, 't> {
    parse_result: &'p ParseResult,
    table: &'t mut SymbolTable,
    current_scope: ScopeId,
    resolutions: HashMap<NodeId, ResolutionResult>,
    diagnostics: Vec<Diagnostic>,
    used_usings: HashSet<NodeId>,
}

impl<'p, 't> NameResolver<'p, 't> {
    pub fn new(parse_result: &'p ParseResult, table: &'t mut SymbolTable) -> Self {
        let current_scope = table.global_scope;
        Self {
            parse_result,
            table,
            current_scope,
            resolutions: HashMap::new(),
            diagnostics: Vec::new(),
            used_usings: HashSet::new(),
        }
    }

    fn resolve(&mut self) {
        self.resolve_using_statements();
        self.resolve_script(self.parse_result.root);
        self.report_unused_usings();
    }

    fn finish(self) -> NameResolution {
        NameResolution {
            resolutions: self.resolutions,
            diagnostics: self.diagnostics,
            unused_usings: self
                .table
                .using_statements
                .iter()
                .copied()
                .filter(|id| !self.used_usings.contains(id))
                .collect(),
        }
    }

    fn resolve_using_statements(&mut self) {
        let usings = self.table.using_statements.clone();
        for using in usings {
            let Some(scope) = self.using_scope(using) else {
                continue;
            };
            let NodeData::UsingStatement { name } = self.node(using).data.clone() else {
                continue;
            };
            let result = self.resolve_qualified_name_in_scope(name, scope, false, false);
            let ResolutionResult::Resolved(sym) = result else {
                self.record_resolution(name, result);
                continue;
            };
            self.record_resolution(name, ResolutionResult::Resolved(sym));
            if !self.table.symbols[sym.index()]
                .flags
                .contains(SymbolFlags::Namespace)
            {
                self.diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: "using-invalid-ref",
                    message: "Using must refer to a namespace".to_string(),
                    target: self.node_span(name),
                });
                continue;
            }
            let Some(target_scope) = self.table.symbols[sym.index()].members else {
                continue;
            };
            if self.table.scopes[scope.index()]
                .usings
                .contains(&target_scope)
            {
                self.diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: "duplicate-using",
                    message: format!(
                        "duplicate using of \"{}\" namespace",
                        self.expression_to_string(name)
                    ),
                    target: self.node_span(using),
                });
                continue;
            }
            self.table.scopes[scope.index()].usings.push(target_scope);
        }
    }

    fn resolve_script(&mut self, node_id: NodeId) {
        self.current_scope = self.table.global_scope;
        let NodeData::TypeSpecScript { statements, .. } = self.node(node_id).data.clone() else {
            return;
        };
        for statement in statements {
            self.resolve_node(statement);
        }
    }

    fn resolve_node(&mut self, node_id: NodeId) {
        match self.node(node_id).data.clone() {
            NodeData::NamespaceStatement {
                decorators,
                statements,
                ..
            } => {
                let scope = self.table.node_scopes.get(&node_id).copied();
                self.resolve_nodes(&decorators);
                match (scope, statements) {
                    (Some(scope), Some(NamespaceBody::Statements(statements))) => {
                        self.with_scope(scope, |resolver| resolver.resolve_nodes(&statements));
                    }
                    (Some(scope), Some(NamespaceBody::Nested(nested))) => {
                        self.with_scope(scope, |resolver| resolver.resolve_node(nested));
                    }
                    (Some(scope), None) => {
                        self.current_scope = scope;
                    }
                    _ => {}
                }
            }
            NodeData::ModelStatement {
                decorators,
                template_parameters,
                extends,
                is,
                properties,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&decorators);
                resolver.resolve_template_parameters(&template_parameters);
                resolver.resolve_optional_type_ref(extends);
                resolver.resolve_optional_type_ref(is);
                resolver.resolve_nodes(&properties);
            }),
            NodeData::ModelExpression { properties, .. } => self.with_node_scope(node_id, |r| {
                r.resolve_nodes(&properties);
            }),
            NodeData::ModelProperty {
                value,
                decorators,
                default,
                ..
            } => {
                self.resolve_nodes(&decorators);
                self.resolve_type_ref(value);
                self.resolve_optional_type_ref(default);
            }
            NodeData::ModelSpreadProperty { target }
            | NodeData::EnumSpreadMember { target }
            | NodeData::ObjectLiteralSpreadProperty { target }
            | NodeData::ValueOfExpression { target }
            | NodeData::TypeOfExpression { target } => {
                self.resolve_type_ref(target);
            }
            NodeData::ScalarStatement {
                decorators,
                template_parameters,
                extends,
                members,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&decorators);
                resolver.resolve_template_parameters(&template_parameters);
                resolver.resolve_optional_type_ref(extends);
                resolver.resolve_nodes(&members);
            }),
            NodeData::ScalarConstructor { parameters, .. } => self.resolve_nodes(&parameters),
            NodeData::InterfaceStatement {
                decorators,
                template_parameters,
                extends,
                operations,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&decorators);
                resolver.resolve_template_parameters(&template_parameters);
                for item in extends {
                    resolver.resolve_type_ref(item);
                }
                resolver.resolve_nodes(&operations);
            }),
            NodeData::UnionStatement {
                decorators,
                template_parameters,
                options,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&decorators);
                resolver.resolve_template_parameters(&template_parameters);
                resolver.resolve_nodes(&options);
            }),
            NodeData::UnionVariant {
                value, decorators, ..
            } => {
                self.resolve_nodes(&decorators);
                self.resolve_type_ref(value);
            }
            NodeData::EnumStatement {
                decorators,
                members,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&decorators);
                resolver.resolve_nodes(&members);
            }),
            NodeData::EnumMember {
                value, decorators, ..
            } => {
                self.resolve_nodes(&decorators);
                self.resolve_optional_type_ref(value);
            }
            NodeData::AliasStatement {
                template_parameters,
                value,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_template_parameters(&template_parameters);
                resolver.resolve_type_ref(value);
            }),
            NodeData::ConstStatement {
                value, type_ref, ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_optional_type_ref(type_ref);
                resolver.resolve_value_expr(value);
            }),
            NodeData::OperationStatement {
                decorators,
                template_parameters,
                signature,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&decorators);
                resolver.resolve_template_parameters(&template_parameters);
                resolver.resolve_node(signature);
            }),
            NodeData::OperationSignatureDeclaration {
                parameters,
                return_type,
            } => {
                self.resolve_node(parameters);
                self.resolve_type_ref(return_type);
            }
            NodeData::OperationSignatureReference { base_operation } => {
                self.resolve_type_ref(base_operation);
            }
            NodeData::TemplateParameterDeclaration {
                constraint,
                default,
                ..
            } => {
                self.resolve_optional_type_ref(constraint);
                self.resolve_optional_type_ref(default);
            }
            NodeData::UsingStatement { .. } => {}
            NodeData::AugmentDecoratorStatement {
                target,
                target_type,
                arguments,
            } => {
                self.resolve_decorator_target(target);
                let result = self.resolve_type_ref(target_type);
                if !matches!(result, ResolutionResult::Resolved(_)) {
                    self.diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: "augment-decorator-target",
                        message: "Augment decorator first argument must be a type reference."
                            .to_string(),
                        target: self.node_span(target_type),
                    });
                }
                for arg in arguments {
                    self.resolve_value_expr(arg);
                }
            }
            NodeData::DecoratorDeclarationStatement {
                target, parameters, ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_node(target);
                resolver.resolve_nodes(&parameters);
            }),
            NodeData::FunctionDeclarationStatement {
                parameters,
                return_type,
                ..
            } => self.with_node_scope(node_id, |resolver| {
                resolver.resolve_nodes(&parameters);
                resolver.resolve_optional_type_ref(return_type);
            }),
            NodeData::FunctionParameter { type_ref, .. } => {
                self.resolve_optional_type_ref(type_ref);
            }
            NodeData::FunctionTypeExpression {
                parameters,
                return_type,
            } => {
                self.resolve_nodes(&parameters);
                self.resolve_optional_type_ref(return_type);
            }
            NodeData::DecoratorExpression { target, arguments } => {
                self.resolve_decorator_target(target);
                for arg in arguments {
                    self.resolve_value_expr(arg);
                }
            }
            NodeData::CallExpression { target, arguments } => {
                self.resolve_type_ref(target);
                for arg in arguments {
                    self.resolve_value_expr(arg);
                }
            }
            NodeData::DirectiveExpression { arguments, .. } => self.resolve_nodes(&arguments),
            NodeData::TypeReference { .. } | NodeData::MemberExpression { .. } => {
                self.resolve_type_ref(node_id);
            }
            NodeData::UnionExpression { options }
            | NodeData::IntersectionExpression { options } => {
                for option in options {
                    self.resolve_type_ref(option);
                }
            }
            NodeData::TupleExpression { values } | NodeData::ArrayLiteral { values } => {
                for value in values {
                    self.resolve_value_expr(value);
                }
            }
            NodeData::ArrayExpression { element_type } => {
                self.resolve_type_ref(element_type);
            }
            NodeData::StringTemplateExpression { head, spans } => {
                self.resolve_node(head);
                self.resolve_nodes(&spans);
            }
            NodeData::StringTemplateSpan {
                expression,
                literal,
            } => {
                self.resolve_value_expr(expression);
                self.resolve_node(literal);
            }
            NodeData::TemplateArgument { name, argument } => {
                self.resolve_optional_type_ref(name);
                self.resolve_type_ref(argument);
            }
            NodeData::ObjectLiteral { properties, .. } => self.resolve_nodes(&properties),
            NodeData::ObjectLiteralProperty { value, .. } => self.resolve_value_expr(value),
            NodeData::ImportStatement { .. }
            | NodeData::Identifier { .. }
            | NodeData::InvalidStatement { .. }
            | NodeData::EmptyStatement
            | NodeData::StringLiteral { .. }
            | NodeData::NumericLiteral { .. }
            | NodeData::BooleanLiteral { .. }
            | NodeData::StringTemplateLiteral { .. }
            | NodeData::Keyword
            | NodeData::Doc { .. }
            | NodeData::DocText { .. }
            | NodeData::DocTag { .. }
            | NodeData::Comment { .. }
            | NodeData::TypeSpecScript { .. } => {}
        }
    }

    fn resolve_type_ref(&mut self, node_id: NodeId) -> ResolutionResult {
        let result = self.resolve_qualified_name_in_scope(node_id, self.current_scope, false, true);
        self.record_resolution(node_id, result.clone());
        result
    }

    fn resolve_decorator_target(&mut self, node_id: NodeId) -> ResolutionResult {
        let result = self.resolve_qualified_name_in_scope(node_id, self.current_scope, true, true);
        self.record_resolution(node_id, result.clone());
        result
    }

    fn resolve_value_expr(&mut self, node_id: NodeId) {
        match self.node(node_id).data.clone() {
            NodeData::TypeReference { .. }
            | NodeData::MemberExpression { .. }
            | NodeData::Identifier { .. } => {
                self.resolve_type_ref(node_id);
            }
            _ => self.resolve_node(node_id),
        }
    }

    fn resolve_optional_type_ref(&mut self, node: Option<NodeId>) {
        if let Some(node) = node {
            self.resolve_type_ref(node);
        }
    }

    fn resolve_template_parameters(&mut self, nodes: &[NodeId]) {
        for node in nodes {
            self.resolve_node(*node);
        }
    }

    fn resolve_qualified_name_in_scope(
        &mut self,
        node_id: NodeId,
        scope: ScopeId,
        resolve_decorator: bool,
        report: bool,
    ) -> ResolutionResult {
        match self.node(node_id).data.clone() {
            NodeData::TypeReference { target, arguments } => {
                let result =
                    self.resolve_qualified_name_in_scope(target, scope, resolve_decorator, report);
                for arg in arguments {
                    self.resolve_node(arg);
                }
                result
            }
            NodeData::Identifier { sv } => {
                let lookup = if resolve_decorator {
                    self.lookup_name(scope, &format!("@{sv}"))
                        .or_else(|| self.lookup_name(scope, &sv))
                } else {
                    self.lookup_name(scope, &sv)
                };
                let result = lookup.unwrap_or(ResolutionResult::Unknown);
                if report {
                    self.report_resolution_failure(node_id, &result, resolve_decorator, None);
                }
                result
            }
            NodeData::MemberExpression { base, id, .. } => {
                let base_result = self.resolve_qualified_name_in_scope(base, scope, false, report);
                let ResolutionResult::Resolved(base_sym) = base_result else {
                    return base_result;
                };
                let name = self.identifier_name(id);
                let result = self.lookup_member(base_sym, &name);
                if report {
                    let ns = self.table.symbols[base_sym.index()].name.clone();
                    self.report_resolution_failure(id, &result, resolve_decorator, Some(&ns));
                }
                self.record_resolution(id, result.clone());
                result
            }
            _ => ResolutionResult::Unknown,
        }
    }

    fn lookup_name(&mut self, start: ScopeId, name: &str) -> Option<ResolutionResult> {
        let mut scope = Some(start);
        while let Some(scope_id) = scope {
            if let Some(symbol) = self.table.scopes[scope_id.index()]
                .symbols
                .get(name)
                .copied()
            {
                return Some(ResolutionResult::Resolved(symbol));
            }

            let mut using_matches = Vec::new();
            let usings = self.table.scopes[scope_id.index()].usings.clone();
            for using_scope in usings {
                if let Some(symbol) = self.table.scopes[using_scope.index()]
                    .symbols
                    .get(name)
                    .copied()
                {
                    using_matches.push(symbol);
                    self.mark_using_used(scope_id, using_scope);
                }
            }
            using_matches.sort_by_key(|id| id.as_u32());
            using_matches.dedup();
            if using_matches.len() == 1 {
                return Some(ResolutionResult::Resolved(using_matches[0]));
            } else if using_matches.len() > 1 {
                return Some(ResolutionResult::Ambiguous(using_matches));
            }
            scope = self.table.scopes[scope_id.index()].parent;
        }
        None
    }

    fn lookup_member(&self, base: SymbolId, name: &str) -> ResolutionResult {
        let Some(scope) = self.table.symbols[base.index()].members else {
            return ResolutionResult::Unknown;
        };
        self.table.scopes[scope.index()]
            .symbols
            .get(name)
            .copied()
            .map(ResolutionResult::Resolved)
            .unwrap_or(ResolutionResult::Unknown)
    }

    fn report_resolution_failure(
        &mut self,
        node_id: NodeId,
        result: &ResolutionResult,
        decorator: bool,
        namespace: Option<&str>,
    ) {
        match result {
            ResolutionResult::Resolved(_) => {}
            ResolutionResult::Unknown => {
                let id = self.identifier_name(node_id);
                let (message, target) = if let Some(ns) = namespace {
                    (
                        format!("Namespace {ns} doesn't have member {id}"),
                        self.node_span(node_id),
                    )
                } else if decorator {
                    (
                        format!("Unknown decorator @{id}"),
                        Span {
                            pos: self.node(node_id).pos.saturating_sub(1),
                            end: self.node(node_id).end,
                        },
                    )
                } else {
                    (format!("Unknown identifier {id}"), self.node_span(node_id))
                };
                self.diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: "invalid-ref",
                    message,
                    target,
                });
            }
            ResolutionResult::Ambiguous(symbols) => {
                let name = self.identifier_name(node_id);
                let duplicate_names = symbols
                    .iter()
                    .map(|id| self.fully_qualified_name(*id))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: "ambiguous-symbol",
                    message: format!(
                        "\"{name}\" is an ambiguous name between {duplicate_names}. Try using fully qualified name instead: {duplicate_names}"
                    ),
                    target: self.node_span(node_id),
                });
            }
        }
    }

    fn report_unused_usings(&mut self) {
        let usings = self.table.using_statements.clone();
        for using in usings {
            if self.used_usings.contains(&using) {
                continue;
            }
            let NodeData::UsingStatement { name } = self.node(using).data.clone() else {
                continue;
            };
            self.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "unused-using",
                message: format!(
                    "'using {}' is declared but never used.",
                    self.expression_to_string(name)
                ),
                target: self.node_span(using),
            });
        }
    }

    fn mark_using_used(&mut self, scope: ScopeId, using_scope: ScopeId) {
        for using in &self.table.scopes[scope.index()].using_statements {
            let NodeData::UsingStatement { name } = self.node(*using).data.clone() else {
                continue;
            };
            if let Some(ResolutionResult::Resolved(sym)) = self.resolutions.get(&name) {
                if self.table.symbols[sym.index()].members == Some(using_scope) {
                    self.used_usings.insert(*using);
                }
            }
        }
    }

    fn record_resolution(&mut self, node_id: NodeId, result: ResolutionResult) {
        self.resolutions.insert(node_id, result);
    }

    fn using_scope(&self, using: NodeId) -> Option<ScopeId> {
        self.table
            .scopes
            .iter()
            .enumerate()
            .find_map(|(index, scope)| {
                scope
                    .using_statements
                    .contains(&using)
                    .then_some(ScopeId::new(index as u32))
            })
    }

    fn with_node_scope<F>(&mut self, node_id: NodeId, f: F)
    where
        F: FnOnce(&mut Self),
    {
        if let Some(scope) = self.table.node_scopes.get(&node_id).copied() {
            self.with_scope(scope, f);
        } else {
            f(self);
        }
    }

    fn with_scope<F>(&mut self, scope: ScopeId, f: F)
    where
        F: FnOnce(&mut Self),
    {
        let previous = self.current_scope;
        self.current_scope = scope;
        f(self);
        self.current_scope = previous;
    }

    fn resolve_nodes(&mut self, nodes: &[NodeId]) {
        for node in nodes {
            self.resolve_node(*node);
        }
    }

    fn fully_qualified_name(&self, symbol: SymbolId) -> String {
        let sym = &self.table.symbols[symbol.index()];
        let mut parts = vec![sym.name.trim_start_matches('@').to_string()];
        let mut parent = sym.parent_symbol;
        while let Some(parent_id) = parent {
            let parent_sym = &self.table.symbols[parent_id.index()];
            if !parent_sym.name.starts_with("<") && parent_sym.name != "-" {
                parts.push(parent_sym.name.trim_start_matches('@').to_string());
            }
            parent = parent_sym.parent_symbol;
        }
        parts.reverse();
        parts.join(".")
    }

    fn expression_to_string(&self, node_id: NodeId) -> String {
        match &self.node(node_id).data {
            NodeData::Identifier { sv } => sv.clone(),
            NodeData::TypeReference { target, .. } => self.expression_to_string(*target),
            NodeData::MemberExpression { base, id, selector } => {
                format!(
                    "{}{}{}",
                    self.expression_to_string(*base),
                    selector,
                    self.expression_to_string(*id)
                )
            }
            _ => "<unknown>".to_string(),
        }
    }

    fn identifier_name(&self, node_id: NodeId) -> String {
        match &self.node(node_id).data {
            NodeData::Identifier { sv } => sv.clone(),
            _ => self.expression_to_string(node_id),
        }
    }

    fn node(&self, node_id: NodeId) -> &typespec_parser::ast::Node {
        self.parse_result.arena.get(node_id)
    }

    fn node_span(&self, node_id: NodeId) -> Span {
        let node = self.node(node_id);
        Span {
            pos: node.pos,
            end: node.end,
        }
    }
}
