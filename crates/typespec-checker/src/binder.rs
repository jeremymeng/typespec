use typespec_parser::ast::{NamespaceBody, NodeData, NodeId};
use typespec_parser::ParseResult;
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use crate::types::{ScopeId, ScopeKind, SymbolFlags, SymbolId, SymbolTable};

pub fn bind(parse_result: &ParseResult) -> SymbolTable {
    let mut binder = Binder::new(parse_result);
    for script in &parse_result.scripts {
        binder.bind_script(*script);
    }
    binder.finish()
}

pub struct Binder<'p> {
    parse_result: &'p ParseResult,
    table: SymbolTable,
    current_scope: ScopeId,
    /// File scope of the script being bound.
    current_file_scope: Option<ScopeId>,
    /// Nesting depth of block scopes; `0` means file level (global or a
    /// blockless namespace).
    scope_depth: usize,
}

impl<'p> Binder<'p> {
    pub fn new(parse_result: &'p ParseResult) -> Self {
        let table = SymbolTable::new();
        let current_scope = table.global_scope;
        let mut binder = Self {
            parse_result,
            table,
            current_scope,
            current_file_scope: None,
            scope_depth: 0,
        };
        binder.add_intrinsics();
        binder
    }

    fn stdlib_loaded(&self) -> bool {
        !self.parse_result.compiler_scripts.is_empty()
    }

    fn finish(mut self) -> SymbolTable {
        if self.stdlib_loaded() {
            self.link_typespec_namespace();
        }
        self.table
            .diagnostics
            .extend(self.parse_result.diagnostics.iter().cloned());
        self.table
    }

    /// With the standard library loaded, the `TypeSpec` namespace declared by
    /// `lib/intrinsics.tsp` replaces the pre-bound global stubs: every file
    /// implicitly uses it (TS `name-resolver` adds a `TypeSpec` using to each
    /// file), and members TS binds from JS (`$decorators` in
    /// `lib/intrinsic/tsp-index.ts`) or adds itself (`null`) are declared on it.
    fn link_typespec_namespace(&mut self) {
        let global = self.table.global_scope;
        let Some(typespec) = self.table.scopes[global.index()]
            .symbols
            .get("TypeSpec")
            .copied()
        else {
            return;
        };
        let Some(members) = self.table.symbols[typespec.index()].members else {
            return;
        };
        let previous = self.current_scope;
        self.current_scope = members;
        for (name, flags) in [
            ("null", SymbolFlags::Scalar | SymbolFlags::Declaration),
            (
                "@indexer",
                SymbolFlags::Decorator | SymbolFlags::Declaration | SymbolFlags::Internal,
            ),
            (
                "@docFromComment",
                SymbolFlags::Decorator | SymbolFlags::Declaration | SymbolFlags::Internal,
            ),
        ] {
            if !self.table.scopes[members.index()]
                .symbols
                .contains_key(name)
            {
                self.declare_prebound(name, flags);
            }
        }
        self.current_scope = previous;
        self.table.implicit_using = Some(members);
    }

    fn add_intrinsics(&mut self) {
        if self.stdlib_loaded() {
            return;
        }
        for name in [
            "string",
            "boolean",
            "bytes",
            "numeric",
            "integer",
            "int8",
            "int16",
            "int32",
            "int64",
            "safeint",
            "uint8",
            "uint16",
            "uint32",
            "uint64",
            "float",
            "float32",
            "float64",
            "decimal",
            "decimal128",
            "plainDate",
            "plainTime",
            "utcDateTime",
            "offsetDateTime",
            "duration",
            "url",
            "void",
            "never",
            "unknown",
            "null",
            "Type",
            "Value",
            "StringLiteral",
            "Numeric",
            "Boolean",
        ] {
            self.declare_prebound(name, SymbolFlags::Scalar | SymbolFlags::Declaration);
        }
        for name in [
            "@doc",
            "@summary",
            "@deprecated",
            "@key",
            "@format",
            "@minLength",
            "@maxLength",
        ] {
            self.declare_prebound(name, SymbolFlags::Decorator | SymbolFlags::Declaration);
        }
    }

    fn declare_prebound(&mut self, name: &str, flags: SymbolFlags) {
        let sym = self.table.add_symbol(
            name.to_string(),
            flags,
            None,
            self.current_scope,
            self.current_symbol(),
        );
        self.table.scopes[self.current_scope.index()]
            .symbols
            .insert(name.to_string(), sym);
    }

    fn bind_script(&mut self, node_id: NodeId) {
        let global = self.table.global_scope;
        self.current_scope = global;
        self.scope_depth = 0;
        self.table.node_scopes.insert(node_id, global);
        let file_scope = self
            .table
            .add_scope(Some(global), ScopeKind::File, None, None);
        self.table.file_scopes.insert(node_id, file_scope);
        self.current_file_scope = Some(file_scope);
        let NodeData::TypeSpecScript { statements, .. } = self.node(node_id).data.clone() else {
            return;
        };
        self.bind_statement_list(&statements);
        self.current_scope = global;
    }

    fn bind_statement_list(&mut self, statements: &[NodeId]) {
        for statement in statements {
            self.bind_node(*statement);
        }
    }

    fn bind_node(&mut self, node_id: NodeId) {
        match self.node(node_id).data.clone() {
            NodeData::NamespaceStatement {
                id,
                decorators,
                statements,
                ..
            } => self.bind_namespace_statement(node_id, id, decorators, statements),
            NodeData::ModelStatement {
                id,
                decorators,
                template_parameters,
                extends,
                is,
                properties,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    decorators,
                    SymbolFlags::Model | SymbolFlags::Declaration | internal_flag(modifier_flags),
                    ScopeKind::Model,
                    true,
                    |binder| {
                        binder.bind_nodes(&template_parameters);
                        binder.bind_optional(extends);
                        binder.bind_optional(is);
                        binder.bind_nodes(&properties);
                    },
                );
            }
            NodeData::ModelExpression { properties, .. } => {
                self.bind_anonymous_member_container(
                    node_id,
                    SymbolFlags::Model,
                    ScopeKind::ModelExpression,
                    |binder| binder.bind_nodes(&properties),
                );
            }
            NodeData::ModelProperty {
                id,
                value,
                decorators,
                default,
                ..
            } => {
                self.declare_member(node_id, id, SymbolFlags::Member);
                self.bind_nodes(&decorators);
                self.bind_node(value);
                self.bind_optional(default);
            }
            NodeData::ModelSpreadProperty { target }
            | NodeData::EnumSpreadMember { target }
            | NodeData::ObjectLiteralSpreadProperty { target }
            | NodeData::ValueOfExpression { target }
            | NodeData::TypeOfExpression { target } => self.bind_node(target),
            NodeData::ScalarStatement {
                id,
                decorators,
                template_parameters,
                extends,
                members,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    decorators,
                    SymbolFlags::Scalar | SymbolFlags::Declaration | internal_flag(modifier_flags),
                    ScopeKind::Scalar,
                    true,
                    |binder| {
                        binder.bind_nodes(&template_parameters);
                        binder.bind_optional(extends);
                        binder.bind_nodes(&members);
                    },
                );
            }
            NodeData::ScalarConstructor { id, parameters } => {
                let sym = self.declare_member(node_id, id, SymbolFlags::Member);
                let scope = self.table.add_scope(
                    Some(self.current_scope),
                    ScopeKind::Function,
                    Some(node_id),
                    Some(sym),
                );
                self.with_scope(scope, |binder| binder.bind_nodes(&parameters));
            }
            NodeData::InterfaceStatement {
                id,
                decorators,
                template_parameters,
                extends,
                operations,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    decorators,
                    SymbolFlags::Interface
                        | SymbolFlags::Declaration
                        | internal_flag(modifier_flags),
                    ScopeKind::Interface,
                    true,
                    |binder| {
                        binder.bind_nodes(&template_parameters);
                        binder.bind_nodes(&extends);
                        binder.bind_nodes(&operations);
                    },
                );
            }
            NodeData::UnionStatement {
                id,
                decorators,
                template_parameters,
                extends,
                options,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    decorators,
                    SymbolFlags::Union | SymbolFlags::Declaration | internal_flag(modifier_flags),
                    ScopeKind::Union,
                    true,
                    |binder| {
                        binder.bind_nodes(&template_parameters);
                        binder.bind_optional(extends);
                        binder.bind_nodes(&options);
                    },
                );
            }
            NodeData::ModelDeclarationExpression {
                decorators,
                template_parameters,
                extends,
                is,
                properties,
                ..
            } => {
                self.bind_anonymous_member_container(
                    node_id,
                    SymbolFlags::Model,
                    ScopeKind::Model,
                    |binder| {
                        binder.bind_nodes(&decorators);
                        binder.bind_nodes(&template_parameters);
                        binder.bind_optional(extends);
                        binder.bind_optional(is);
                        binder.bind_nodes(&properties);
                    },
                );
            }
            NodeData::ScalarDeclarationExpression {
                decorators,
                template_parameters,
                extends,
                members,
                ..
            } => {
                self.bind_anonymous_member_container(
                    node_id,
                    SymbolFlags::Scalar,
                    ScopeKind::Scalar,
                    |binder| {
                        binder.bind_nodes(&decorators);
                        binder.bind_nodes(&template_parameters);
                        binder.bind_optional(extends);
                        binder.bind_nodes(&members);
                    },
                );
            }
            NodeData::UnionDeclarationExpression {
                decorators,
                template_parameters,
                extends,
                options,
                ..
            } => {
                self.bind_anonymous_member_container(
                    node_id,
                    SymbolFlags::Union,
                    ScopeKind::Union,
                    |binder| {
                        binder.bind_nodes(&decorators);
                        binder.bind_nodes(&template_parameters);
                        binder.bind_optional(extends);
                        binder.bind_nodes(&options);
                    },
                );
            }
            NodeData::EnumDeclarationExpression {
                decorators,
                members,
                ..
            } => {
                self.bind_anonymous_member_container(
                    node_id,
                    SymbolFlags::Enum,
                    ScopeKind::Enum,
                    |binder| {
                        binder.bind_nodes(&decorators);
                        binder.bind_nodes(&members);
                    },
                );
            }
            NodeData::UnionVariant {
                id,
                value,
                decorators,
            } => {
                if let Some(id) = id {
                    self.declare_member(node_id, id, SymbolFlags::Member);
                }
                self.bind_nodes(&decorators);
                self.bind_node(value);
            }
            NodeData::EnumStatement {
                id,
                decorators,
                members,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    decorators,
                    SymbolFlags::Enum | SymbolFlags::Declaration | internal_flag(modifier_flags),
                    ScopeKind::Enum,
                    true,
                    |binder| binder.bind_nodes(&members),
                );
            }
            NodeData::EnumMember {
                id,
                value,
                decorators,
            } => {
                self.declare_member(node_id, id, SymbolFlags::Member);
                self.bind_nodes(&decorators);
                self.bind_optional(value);
            }
            NodeData::AliasStatement {
                id,
                template_parameters,
                value,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    Vec::new(),
                    SymbolFlags::Alias | SymbolFlags::Declaration | internal_flag(modifier_flags),
                    ScopeKind::Alias,
                    false,
                    |binder| {
                        binder.bind_nodes(&template_parameters);
                        binder.bind_node(value);
                    },
                );
            }
            NodeData::ConstStatement {
                id,
                value,
                type_ref,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    Vec::new(),
                    SymbolFlags::Const | SymbolFlags::Declaration | internal_flag(modifier_flags),
                    ScopeKind::Const,
                    false,
                    |binder| {
                        binder.bind_optional(type_ref);
                        binder.bind_node(value);
                    },
                );
            }
            NodeData::OperationStatement {
                id,
                decorators,
                template_parameters,
                signature,
                modifier_flags,
                ..
            } => {
                let flags = SymbolFlags::Operation
                    | SymbolFlags::Declaration
                    | internal_flag(modifier_flags)
                    | if self.table.scopes[self.current_scope.index()].kind == ScopeKind::Interface
                    {
                        SymbolFlags::Member
                    } else {
                        SymbolFlags::None
                    };
                let sym = if flags.contains(SymbolFlags::Member) {
                    self.declare_member(node_id, id, flags)
                } else {
                    self.declare_symbol(node_id, id, flags, None)
                };
                let scope = self.table.add_scope(
                    Some(self.current_scope),
                    ScopeKind::Operation,
                    Some(node_id),
                    Some(sym),
                );
                self.with_scope(scope, |binder| {
                    binder.bind_nodes(&decorators);
                    binder.bind_nodes(&template_parameters);
                    binder.bind_node(signature);
                });
            }
            NodeData::OperationSignatureDeclaration {
                parameters,
                return_type,
            } => {
                self.bind_node(parameters);
                self.bind_node(return_type);
            }
            NodeData::OperationSignatureReference { base_operation } => {
                self.bind_node(base_operation);
            }
            NodeData::TemplateParameterDeclaration {
                id,
                constraint,
                default,
            } => {
                self.declare_symbol(
                    node_id,
                    id,
                    SymbolFlags::TemplateParameter | SymbolFlags::Declaration,
                    None,
                );
                self.bind_optional(constraint);
                self.bind_optional(default);
            }
            NodeData::UsingStatement { name } => {
                self.table.using_statements.push(node_id);
                self.table
                    .using_resolution_scopes
                    .insert(node_id, self.current_scope);
                // File-level usings (global or after a blockless namespace)
                // only apply to the declaring file, like TS `file.locals`.
                let owner = match self.current_file_scope {
                    Some(file_scope) if self.scope_depth == 0 => file_scope,
                    _ => self.current_scope,
                };
                if let Some(file_scope) = self.current_file_scope {
                    self.table.using_files.insert(node_id, file_scope);
                }
                self.table.scopes[owner.index()]
                    .using_statements
                    .push(node_id);
                self.bind_node(name);
            }
            NodeData::AugmentDecoratorStatement {
                target,
                target_type,
                arguments,
            } => {
                self.table.augment_decorators.push(node_id);
                self.bind_node(target);
                self.bind_node(target_type);
                self.bind_nodes(&arguments);
            }
            NodeData::DecoratorDeclarationStatement {
                id,
                target,
                parameters,
                modifier_flags,
                ..
            } => {
                let name = format!("@{}", self.identifier_name(id));
                let sym = self.declare_symbol(
                    node_id,
                    id,
                    SymbolFlags::Decorator
                        | SymbolFlags::Declaration
                        | internal_flag(modifier_flags),
                    Some(name),
                );
                let scope = self.table.add_scope(
                    Some(self.current_scope),
                    ScopeKind::Function,
                    Some(node_id),
                    Some(sym),
                );
                self.with_scope(scope, |binder| {
                    binder.bind_node(target);
                    binder.bind_nodes(&parameters);
                });
            }
            NodeData::FunctionDeclarationStatement {
                id,
                parameters,
                return_type,
                modifier_flags,
                ..
            } => {
                self.bind_declaration_with_scope(
                    node_id,
                    id,
                    Vec::new(),
                    SymbolFlags::Function
                        | SymbolFlags::Declaration
                        | internal_flag(modifier_flags),
                    ScopeKind::Function,
                    false,
                    |binder| {
                        binder.bind_nodes(&parameters);
                        binder.bind_optional(return_type);
                    },
                );
            }
            NodeData::FunctionParameter { id, type_ref, .. } => {
                self.declare_symbol(
                    node_id,
                    id,
                    SymbolFlags::FunctionParameter | SymbolFlags::Declaration,
                    None,
                );
                self.bind_optional(type_ref);
            }
            NodeData::FunctionTypeExpression {
                parameters,
                return_type,
            } => {
                self.bind_nodes(&parameters);
                self.bind_optional(return_type);
            }
            NodeData::TypeReference { target, arguments } => {
                self.bind_node(target);
                self.bind_nodes(&arguments);
            }
            NodeData::MemberExpression { base, id, .. } => {
                self.bind_node(base);
                self.bind_node(id);
            }
            NodeData::DecoratorExpression { target, arguments }
            | NodeData::DirectiveExpression { target, arguments }
            | NodeData::CallExpression { target, arguments } => {
                self.bind_node(target);
                self.bind_nodes(&arguments);
            }
            NodeData::UnionExpression { options }
            | NodeData::IntersectionExpression { options }
            | NodeData::TupleExpression { values: options }
            | NodeData::ArrayLiteral { values: options } => self.bind_nodes(&options),
            NodeData::ArrayExpression { element_type } => self.bind_node(element_type),
            NodeData::StringTemplateExpression { head, spans } => {
                self.bind_node(head);
                self.bind_nodes(&spans);
            }
            NodeData::StringTemplateSpan {
                expression,
                literal,
            } => {
                self.bind_node(expression);
                self.bind_node(literal);
            }
            NodeData::TemplateArgument { name, argument } => {
                self.bind_optional(name);
                self.bind_node(argument);
            }
            NodeData::ObjectLiteral { properties, .. } => self.bind_nodes(&properties),
            NodeData::ObjectLiteralProperty { id, value } => {
                self.bind_node(id);
                self.bind_node(value);
            }
            NodeData::ImportStatement { path } => self.bind_node(path),
            NodeData::InvalidStatement { decorators } => self.bind_nodes(&decorators),
            NodeData::Doc { content, tags } => {
                self.bind_nodes(&content);
                self.bind_nodes(&tags);
            }
            NodeData::DocTag {
                tag_name,
                param_name,
                content,
            } => {
                self.bind_node(tag_name);
                self.bind_optional(param_name);
                self.bind_nodes(&content);
            }
            NodeData::TypeSpecScript { .. }
            | NodeData::Identifier { .. }
            | NodeData::EmptyStatement
            | NodeData::StringLiteral { .. }
            | NodeData::NumericLiteral { .. }
            | NodeData::BooleanLiteral { .. }
            | NodeData::StringTemplateLiteral { .. }
            | NodeData::Keyword
            | NodeData::DocText { .. }
            | NodeData::Comment { .. } => {}
        }
    }

    fn bind_namespace_statement(
        &mut self,
        node_id: NodeId,
        id: NodeId,
        decorators: Vec<NodeId>,
        statements: Option<NamespaceBody>,
    ) {
        let sym = self.declare_namespace(node_id, id);
        let scope = self.ensure_member_scope(sym, ScopeKind::Namespace, Some(node_id));
        self.bind_nodes(&decorators);
        match statements {
            Some(NamespaceBody::Statements(statements)) => {
                self.with_scope(scope, |binder| binder.bind_statement_list(&statements));
            }
            Some(NamespaceBody::Nested(nested)) => {
                let mut blockless = None;
                self.with_scope(scope, |binder| {
                    binder.bind_node(nested);
                    if binder.current_scope != scope {
                        blockless = Some(binder.current_scope);
                    }
                });
                // `namespace A.B;`: the innermost blockless namespace stays in
                // scope for the remaining statements of the file.
                if let Some(inner) = blockless {
                    self.current_scope = inner;
                }
            }
            None => {
                self.current_scope = scope;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_declaration_with_scope<F>(
        &mut self,
        node_id: NodeId,
        id: NodeId,
        decorators: Vec<NodeId>,
        flags: SymbolFlags,
        kind: ScopeKind,
        member_container: bool,
        bind_children: F,
    ) -> SymbolId
    where
        F: FnOnce(&mut Self),
    {
        let sym = self.declare_symbol(node_id, id, flags, None);
        let scope = if member_container {
            self.ensure_member_scope(sym, kind, Some(node_id))
        } else {
            self.table
                .add_scope(Some(self.current_scope), kind, Some(node_id), Some(sym))
        };
        self.with_scope(scope, |binder| {
            binder.bind_nodes(&decorators);
            bind_children(binder);
        });
        sym
    }

    fn bind_anonymous_member_container<F>(
        &mut self,
        node_id: NodeId,
        flags: SymbolFlags,
        kind: ScopeKind,
        bind_children: F,
    ) -> SymbolId
    where
        F: FnOnce(&mut Self),
    {
        let sym = self.table.add_symbol(
            "-".to_string(),
            flags,
            Some(node_id),
            self.current_scope,
            self.current_symbol(),
        );
        let scope = self.ensure_member_scope(sym, kind, Some(node_id));
        self.with_scope(scope, bind_children);
        sym
    }

    fn declare_namespace(&mut self, node_id: NodeId, id: NodeId) -> SymbolId {
        let name = self.identifier_name(id);
        if let Some(existing) = self.table.scopes[self.current_scope.index()]
            .symbols
            .get(&name)
            .copied()
        {
            if self.table.symbols[existing.index()]
                .flags
                .contains(SymbolFlags::Namespace)
            {
                self.table.symbols[existing.index()]
                    .declarations
                    .push(node_id);
                self.table.node_symbols.insert(node_id, existing);
                return existing;
            }
        }
        self.declare_symbol(
            node_id,
            id,
            SymbolFlags::Namespace | SymbolFlags::Declaration,
            Some(name),
        )
    }

    fn declare_symbol(
        &mut self,
        node_id: NodeId,
        id: NodeId,
        flags: SymbolFlags,
        name_override: Option<String>,
    ) -> SymbolId {
        let name = name_override.unwrap_or_else(|| self.identifier_name(id));
        let symbol = self.table.add_symbol(
            name.clone(),
            flags,
            Some(node_id),
            self.current_scope,
            self.current_symbol(),
        );
        self.insert_symbol(self.current_scope, name, symbol);
        symbol
    }

    fn declare_member(&mut self, node_id: NodeId, id: NodeId, flags: SymbolFlags) -> SymbolId {
        let name = self.identifier_name(id);
        let symbol = self.table.add_symbol(
            name.clone(),
            flags,
            Some(node_id),
            self.current_scope,
            self.current_symbol(),
        );
        self.insert_symbol(self.current_scope, name, symbol);
        symbol
    }

    fn insert_symbol(&mut self, scope: ScopeId, name: String, symbol: SymbolId) {
        let existing = self.table.scopes[scope.index()].symbols.get(&name).copied();
        if existing.is_none() {
            self.table.scopes[scope.index()]
                .symbols
                .insert(name, symbol);
            return;
        }
        let existing = existing.expect("checked above");
        if self.table.symbols[existing.index()].declarations.is_empty() {
            self.table.scopes[scope.index()]
                .symbols
                .insert(name, symbol);
            return;
        }
        self.report_duplicate(existing);
        self.report_duplicate(symbol);
    }

    fn report_duplicate(&mut self, symbol: SymbolId) {
        if !self.table.duplicate_symbols_reported.insert(symbol) {
            return;
        }
        let sym = &self.table.symbols[symbol.index()];
        let Some(node) = sym.declarations.first().copied() else {
            return;
        };
        self.table.diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "duplicate-symbol",
            message: format!("Duplicate name: \"{}\"", sym.name),
            target: self.node_span(node),
        });
    }

    fn ensure_member_scope(
        &mut self,
        symbol: SymbolId,
        kind: ScopeKind,
        owning_node: Option<NodeId>,
    ) -> ScopeId {
        if let Some(scope) = self.table.symbols[symbol.index()].members {
            if let Some(node) = owning_node {
                self.table.node_scopes.insert(node, scope);
            }
            return scope;
        }
        let scope = self
            .table
            .add_scope(Some(self.current_scope), kind, owning_node, Some(symbol));
        self.table.symbols[symbol.index()].members = Some(scope);
        scope
    }

    fn with_scope<F>(&mut self, scope: ScopeId, f: F)
    where
        F: FnOnce(&mut Self),
    {
        let previous = self.current_scope;
        self.current_scope = scope;
        self.scope_depth += 1;
        f(self);
        self.scope_depth -= 1;
        self.current_scope = previous;
    }

    fn bind_nodes(&mut self, nodes: &[NodeId]) {
        for node in nodes {
            self.bind_node(*node);
        }
    }

    fn bind_optional(&mut self, node: Option<NodeId>) {
        if let Some(node) = node {
            self.bind_node(node);
        }
    }

    fn current_symbol(&self) -> Option<SymbolId> {
        self.table.scopes[self.current_scope.index()].owning_symbol
    }

    fn identifier_name(&self, node_id: NodeId) -> String {
        match &self.node(node_id).data {
            NodeData::Identifier { sv } => sv.clone(),
            _ => "<missing identifier>".to_string(),
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

fn internal_flag(modifier_flags: u32) -> SymbolFlags {
    if modifier_flags & (1 << 2) != 0 {
        SymbolFlags::Internal
    } else {
        SymbolFlags::None
    }
}
