use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use typespec_parser::ast::{NamespaceBody, NodeData, NodeId};
use typespec_parser::syntax_kind::SyntaxKind;
use typespec_parser::ParseResult;
use typespec_source::{Diagnostic, DiagnosticSeverity, Numeric, Span};

use crate::decorator::{
    decorator_state_key, CheckOptions, DecoratorApplication, DecoratorArgument, DecoratorContext,
    DecoratorRunner, DecoratorState, DecoratorValue, DecoratorValueEntity, ExternDeclarationInfo,
    ResolvedDecoratorApplication,
};
use crate::instantiation::InstantiationMap;
use crate::intrinsics::{register_intrinsics, Intrinsics};
use crate::name_resolver::ResolutionResult;
use crate::type_relation::{type_name, type_name_only, TypeRelationChecker};
use crate::types::{
    AliasType, BooleanLiteralType, DecoratorRef, EnumMemberType, EnumMemberValue, EnumType,
    InterfaceType, IntrinsicDeclaration, ModelIndexer, ModelPropertyType, ModelType, NamespaceType,
    NumericLiteralType, OperationType, ScalarType, StringLiteralType, StringTemplateSpanType,
    StringTemplateType, SymbolFlags, SymbolId, SymbolTable, TemplateParameterType, TupleType, Type,
    TypeArena, TypeId, UnionType, UnionVariantType,
};

mod decorators;
mod deprecation;
mod values;

pub struct Checker<'p> {
    options: CheckOptions,
    decorator_applications: Vec<ResolvedDecoratorApplication>,
    extern_declarations: Vec<ExternDeclarationInfo>,
    invalid_augment_decorators: HashSet<NodeId>,
    parse_result: &'p ParseResult,
    symbol_table: &'p SymbolTable,
    resolutions: &'p HashMap<NodeId, ResolutionResult>,
    type_arena: TypeArena,
    intrinsics: Intrinsics,
    global_namespace: TypeId,
    diagnostics: Vec<Diagnostic>,
    type_cache: HashMap<NodeId, TypeId>,
    symbol_to_type: HashMap<SymbolId, TypeId>,
    node_to_type: HashMap<NodeId, TypeId>,
    base_stack: Vec<SymbolId>,
    reported_base_cycles: HashSet<SymbolId>,
    literal_cache: HashMap<LiteralKey, TypeId>,
    alias_stack: Vec<SymbolId>,
    reported_alias_cycles: HashSet<SymbolId>,
    alias_structural_depth: usize,
    operation_stack: Vec<SymbolId>,
    reported_operation_cycles: HashSet<SymbolId>,
    template_instantiations: InstantiationMap,
    template_parameter_usage: HashMap<NodeId, bool>,
    /// Spreads of template parameters (`...T`) in template declarations, applied on
    /// instantiation: model -> (property insertion index, template parameter).
    template_parameter_spreads: HashMap<TypeId, Vec<(usize, TypeId)>>,
    /// Template mapper each instantiated type (and its cloned members) was created with, used to
    /// resolve decorator arguments and to skip decorators on instances still inside a template
    /// declaration.
    instance_contexts: HashMap<TypeId, HashMap<TypeId, TypeId>>,
    decorator_state: DecoratorState,
    intrinsic_declarations: HashMap<TypeId, IntrinsicDeclaration>,
    /// Lazily built AST parent links (deprecation context walks).
    node_parents: Option<HashMap<NodeId, NodeId>>,
    reported_deprecations: HashSet<NodeId>,
    /// Object / array value template arguments (and defaults) are not modeled as
    /// values yet; each gets a distinct `unknown` placeholder type remembering its
    /// expression so decorator arguments mapped from it can be evaluated (M11).
    value_placeholders: HashMap<TypeId, NodeId>,
    /// Precise types of object / array / const values (M12).
    value_types: HashMap<NodeId, TypeId>,
    /// Properties of object value types, checked for excess properties.
    object_value_properties: HashSet<TypeId>,
}

#[derive(Debug, Clone, Copy)]
enum NamespaceMemberKind {
    Model,
    Scalar,
    Enum,
    Union,
    Alias,
    Operation,
    Interface,
    Namespace,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LiteralKey {
    String(String),
    Numeric(Numeric),
    Boolean(bool),
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub type_arena: TypeArena,
    pub symbol_to_type: HashMap<SymbolId, TypeId>,
    pub node_to_type: HashMap<NodeId, TypeId>,
    pub diagnostics: Vec<Diagnostic>,
    pub decorator_state: DecoratorState,
    /// Intrinsic scalar type -> its `lib/intrinsics.tsp` declaration (standard
    /// library loaded only).
    pub intrinsic_declarations: HashMap<TypeId, IntrinsicDeclaration>,
    /// Validated decorator applications in TS execution order (M11); only
    /// collected with [`CheckOptions::collect_decorator_applications`].
    pub decorator_applications: Vec<ResolvedDecoratorApplication>,
    /// `dec` / `fn` declarations (M11).
    pub extern_declarations: Vec<ExternDeclarationInfo>,
    /// Deprecated types and their message (TS `TypeSpec.deprecated` state map).
    pub deprecations: Vec<(TypeId, String)>,
    /// Model property default values (TS `ModelProperty.defaultValue`).
    pub default_values: Vec<(TypeId, DecoratorValueEntity)>,
}

pub fn check(
    parse_result: &ParseResult,
    symbol_table: &SymbolTable,
    resolutions: &HashMap<NodeId, ResolutionResult>,
) -> CheckResult {
    let mut checker = Checker::new(parse_result, symbol_table, resolutions);
    for script in &parse_result.scripts {
        checker.check_script(*script);
    }
    checker.finish()
}

pub fn check_with_decorator_runner(
    parse_result: &ParseResult,
    symbol_table: &SymbolTable,
    resolutions: &HashMap<NodeId, ResolutionResult>,
    runner: &mut dyn DecoratorRunner,
) -> CheckResult {
    let mut checker = Checker::new(parse_result, symbol_table, resolutions);
    for script in &parse_result.scripts {
        checker.check_script(*script);
    }
    if runner.is_enabled() {
        checker.apply_decorators(runner);
    }
    checker.finish()
}

/// Check with M11 options (auto-decorator feature, decorator application collection).
pub fn check_with_options(
    parse_result: &ParseResult,
    symbol_table: &SymbolTable,
    resolutions: &HashMap<NodeId, ResolutionResult>,
    options: CheckOptions,
) -> CheckResult {
    let mut checker = Checker::new(parse_result, symbol_table, resolutions);
    checker.options = options;
    for script in &parse_result.scripts {
        checker.check_script(*script);
    }
    checker.finish()
}

impl<'p> Checker<'p> {
    fn new(
        parse_result: &'p ParseResult,
        symbol_table: &'p SymbolTable,
        resolutions: &'p HashMap<NodeId, ResolutionResult>,
    ) -> Self {
        let mut type_arena = TypeArena::new();
        let mut symbol_to_type = HashMap::new();
        let intrinsics = register_intrinsics(&mut type_arena, symbol_table, &mut symbol_to_type);
        let global_namespace = type_arena.add(Type::Namespace(NamespaceType {
            name: String::new(),
            node: None,
            symbol: None,
            namespace: None,
            models: IndexMap::new(),
            scalars: IndexMap::new(),
            enums: IndexMap::new(),
            unions: IndexMap::new(),
            aliases: IndexMap::new(),
            operations: IndexMap::new(),
            interfaces: IndexMap::new(),
            namespaces: IndexMap::new(),
            decorators: Vec::new(),
        }));
        Self {
            options: CheckOptions::default(),
            decorator_applications: Vec::new(),
            extern_declarations: Vec::new(),
            invalid_augment_decorators: HashSet::new(),
            parse_result,
            symbol_table,
            resolutions,
            type_arena,
            intrinsics,
            global_namespace,
            diagnostics: Vec::new(),
            type_cache: HashMap::new(),
            symbol_to_type,
            node_to_type: HashMap::new(),
            base_stack: Vec::new(),
            reported_base_cycles: HashSet::new(),
            literal_cache: HashMap::new(),
            alias_stack: Vec::new(),
            reported_alias_cycles: HashSet::new(),
            alias_structural_depth: 0,
            operation_stack: Vec::new(),
            reported_operation_cycles: HashSet::new(),
            template_instantiations: InstantiationMap::default(),
            instance_contexts: HashMap::new(),
            template_parameter_usage: HashMap::new(),
            template_parameter_spreads: HashMap::new(),
            decorator_state: HashMap::new(),
            intrinsic_declarations: HashMap::new(),
            node_parents: None,
            reported_deprecations: HashSet::new(),
            value_placeholders: HashMap::new(),
            value_types: HashMap::new(),
            object_value_properties: HashSet::new(),
        }
    }

    fn finish(mut self) -> CheckResult {
        self.check_declaration_modifiers();
        self.check_decorator_declarations();
        self.check_deprecation_directives();
        self.check_augment_decorator_targets();
        self.check_decorator_applications();
        self.report_unused_template_parameters();
        let binder_diagnostics = self
            .symbol_table
            .diagnostics
            .iter()
            .filter(|d| {
                // TS reports duplicate model properties in the checker as
                // duplicate-property, not as binder duplicate-symbols.
                d.code != "duplicate-symbol" || !self.is_member_duplicate_diagnostic(d.target)
            })
            .cloned()
            .collect::<Vec<_>>();
        self.diagnostics.extend(binder_diagnostics);
        let deprecations = self.collect_deprecations();
        let default_values = self.collect_default_values();
        CheckResult {
            deprecations,
            default_values,
            type_arena: self.type_arena,
            symbol_to_type: self.symbol_to_type,
            node_to_type: self.node_to_type,
            diagnostics: self.diagnostics,
            decorator_state: self.decorator_state,
            intrinsic_declarations: self.intrinsic_declarations,
            decorator_applications: self.decorator_applications,
            extern_declarations: self.extern_declarations,
        }
    }

    fn check_script(&mut self, node_id: NodeId) {
        let NodeData::TypeSpecScript { statements, .. } = self.node(node_id).data.clone() else {
            return;
        };
        for statement in statements {
            self.check_node(statement);
        }
    }

    fn check_node(&mut self, node_id: NodeId) -> Option<TypeId> {
        match self.node(node_id).data.clone() {
            NodeData::NamespaceStatement { statements, .. } => {
                let namespace_id = self.check_namespace(node_id);
                match statements {
                    Some(NamespaceBody::Statements(statements)) => {
                        for statement in statements {
                            self.check_node(statement);
                        }
                    }
                    Some(NamespaceBody::Nested(node)) => {
                        self.check_node(node);
                    }
                    None => {}
                }
                Some(namespace_id)
            }
            NodeData::ModelStatement { .. } => Some(self.check_model_statement(node_id)),
            NodeData::ScalarStatement { .. } => Some(self.check_scalar(node_id)),
            NodeData::EnumStatement { .. } => Some(self.check_enum(node_id)),
            NodeData::UnionStatement { .. } => Some(self.check_union_statement(node_id)),
            NodeData::ModelDeclarationExpression { .. } => {
                Some(self.check_model_statement(node_id))
            }
            NodeData::ScalarDeclarationExpression { .. } => Some(self.check_scalar(node_id)),
            NodeData::EnumDeclarationExpression { .. } => Some(self.check_enum(node_id)),
            NodeData::UnionDeclarationExpression { .. } => {
                Some(self.check_union_statement(node_id))
            }
            NodeData::AliasStatement { .. } => Some(self.check_alias(node_id)),
            NodeData::ModelExpression { .. } => Some(self.check_model_expression(node_id)),
            NodeData::IntersectionExpression { .. } => {
                Some(self.check_intersection_expression(node_id))
            }
            NodeData::OperationStatement { .. } => Some(self.check_operation(node_id, None)),
            NodeData::OperationSignatureDeclaration {
                parameters,
                return_type,
            } => {
                self.type_for_node(parameters);
                self.type_for_node(return_type);
                None
            }
            NodeData::InterfaceStatement { .. } => Some(self.check_interface(node_id)),
            NodeData::ConstStatement {
                value,
                type_ref: Some(type_ref),
                ..
            } => {
                // TS `checkConst`: the value must be of the declared type.
                let target = self.type_for_node(type_ref);
                if !matches!(self.type_arena.get(target), Type::Error) {
                    let diagnostics = self.check_value_of_type(value, target);
                    self.diagnostics.extend(diagnostics);
                }
                None
            }
            _ => None,
        }
    }

    fn check_namespace(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let symbol = self
            .symbol_for_node(node_id)
            .expect("namespace statement should have symbol");
        let namespace_id = self.get_or_create_namespace(symbol);
        self.cache_type(node_id, namespace_id);
        namespace_id
    }

    fn get_or_create_namespace(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(type_id) = self.symbol_to_type.get(&symbol).copied() {
            return type_id;
        }

        let parent_namespace = self.parent_namespace_for_namespace_symbol(symbol);
        let node = self.symbol_table.symbols[symbol.index()]
            .declarations
            .first()
            .copied();
        let declarations = self.symbol_table.symbols[symbol.index()]
            .declarations
            .clone();
        let decorators = declarations
            .iter()
            .filter_map(|declaration| match self.node(*declaration).data.clone() {
                NodeData::NamespaceStatement { decorators, .. } => Some(decorators),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        let decorators = self.collect_decorators(&decorators);
        let name = self.symbol_table.symbols[symbol.index()].name.clone();

        let type_id = self.type_arena.reserve();
        self.symbol_to_type.insert(symbol, type_id);
        for declaration in declarations {
            self.cache_type(declaration, type_id);
        }
        self.type_arena.replace(
            type_id,
            Type::Namespace(NamespaceType {
                name: name.clone(),
                node,
                symbol: Some(symbol),
                namespace: Some(parent_namespace),
                models: IndexMap::new(),
                scalars: IndexMap::new(),
                enums: IndexMap::new(),
                unions: IndexMap::new(),
                aliases: IndexMap::new(),
                operations: IndexMap::new(),
                interfaces: IndexMap::new(),
                namespaces: IndexMap::new(),
                decorators,
            }),
        );
        self.register_namespace_member(
            parent_namespace,
            name,
            type_id,
            NamespaceMemberKind::Namespace,
        );
        type_id
    }

    fn check_model_statement(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }

        let (id, decorators, template_parameter_nodes, extends, is, properties, expression) =
            match self.node(node_id).data.clone() {
                NodeData::ModelStatement {
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    is,
                    properties,
                    ..
                } => (
                    Some(id),
                    decorators,
                    template_parameters,
                    extends,
                    is,
                    properties,
                    false,
                ),
                NodeData::ModelDeclarationExpression {
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    is,
                    properties,
                    ..
                } => (
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    is,
                    properties,
                    true,
                ),
                _ => panic!("expected model statement"),
            };

        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }
        let template_parameters = self.check_template_parameters(&template_parameter_nodes);
        if expression {
            self.report_declaration_expression_constraints(node_id, &template_parameter_nodes);
        }

        let source_model = self.check_model_is(symbol, is);
        let mut model = ModelType {
            name: self.optional_identifier_name(id),
            expression,
            node: Some(node_id),
            symbol,
            properties: IndexMap::new(),
            base_model: None,
            source_model,
            indexer: None,
            decorators: self.collect_decorators(&decorators),
            namespace,
            derived_models: Vec::new(),
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: template_parameters.clone(),
            is_template_declaration: !template_parameters.is_empty(),
        };

        if let Some(source_model) = source_model {
            self.copy_model_is_properties(type_id, source_model, &mut model);
            model.base_model = self
                .model_type(source_model)
                .and_then(|source| source.base_model);
            model.indexer = self
                .model_type(source_model)
                .and_then(|source| source.indexer);
        } else if let Some(extends) = extends {
            model.base_model = self.check_model_extends(symbol, extends);
        }

        self.type_arena.replace(type_id, Type::Model(model.clone()));
        self.check_model_members(&properties, type_id, &mut model);
        if let Some(indexer) = self.native_indexer(&decorators) {
            model.indexer = Some(indexer);
        }

        let base_model = model.base_model;
        let model_name = model.name.clone();
        self.type_arena.replace(type_id, Type::Model(model));
        if let (Some(namespace), false) = (namespace, expression) {
            self.register_namespace_member(
                namespace,
                model_name,
                type_id,
                NamespaceMemberKind::Model,
            );
        }
        if let Some(base_model) = base_model {
            if let Type::Model(base) = self.type_arena.get_mut(base_model) {
                if !base.derived_models.contains(&type_id) {
                    base.derived_models.push(type_id);
                }
            }
        }
        type_id
    }

    fn check_model_expression(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::ModelExpression { properties, .. } = self.node(node_id).data.clone() else {
            panic!("expected model expression");
        };
        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let mut model = ModelType {
            expression: true,
            name: String::new(),
            node: Some(node_id),
            symbol,
            properties: IndexMap::new(),
            base_model: None,
            source_model: None,
            indexer: None,
            decorators: Vec::new(),
            namespace,
            derived_models: Vec::new(),
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
        self.check_model_members(&properties, type_id, &mut model);
        self.type_arena.replace(type_id, Type::Model(model));
        type_id
    }

    fn check_scalar(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }

        let (id, decorators, template_parameter_nodes, extends, expression) =
            match self.node(node_id).data.clone() {
                NodeData::ScalarStatement {
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    ..
                } => (Some(id), decorators, template_parameters, extends, false),
                NodeData::ScalarDeclarationExpression {
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    ..
                } => (id, decorators, template_parameters, extends, true),
                _ => panic!("expected scalar statement"),
            };
        let symbol = self.symbol_for_node(node_id);
        if let Some(intrinsic) = symbol.and_then(|symbol| self.intrinsic_for_symbol(symbol)) {
            return self
                .check_intrinsic_scalar_declaration(node_id, intrinsic, extends, decorators);
        }
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let template_parameters = self.check_template_parameters(&template_parameter_nodes);
        if expression {
            self.report_declaration_expression_constraints(node_id, &template_parameter_nodes);
        }
        let base_scalar = extends.and_then(|extends| self.check_scalar_extends(symbol, extends));
        let decorators = self.collect_decorators(&decorators);
        let name = self.optional_identifier_name(id);
        let register_name = name.clone();
        self.type_arena.replace(
            type_id,
            Type::Scalar(ScalarType {
                name,
                node: Some(node_id),
                symbol,
                base_scalar,
                decorators,
                namespace,
                expression,
                template_node: None,
                template_arguments: Vec::new(),
                template_parameters: template_parameters.clone(),
                is_template_declaration: !template_parameters.is_empty(),
            }),
        );
        if let (Some(namespace), false) = (namespace, expression) {
            self.register_namespace_member(
                namespace,
                register_name,
                type_id,
                NamespaceMemberKind::Scalar,
            );
        }
        type_id
    }

    /// The intrinsic type a standard library scalar declaration stands for
    /// (`scalar string;` in the `TypeSpec` namespace), if any.
    fn intrinsic_for_symbol(&self, symbol: SymbolId) -> Option<TypeId> {
        let implicit = self.symbol_table.implicit_using?;
        let name = &self.symbol_table.symbols[symbol.index()].name;
        let type_id = self.intrinsics.by_name(name)?;
        (self.symbol_table.scopes[implicit.index()].symbols.get(name) == Some(&symbol))
            .then_some(type_id)
    }

    /// The declared type of a `TypeSpec` namespace member (e.g. `Array`,
    /// `Record`) when the standard library is loaded.
    fn std_template(&mut self, name: &str) -> Option<TypeId> {
        let implicit = self.symbol_table.implicit_using?;
        let symbol = self.symbol_table.scopes[implicit.index()]
            .symbols
            .get(name)
            .copied()?;
        if self.symbol_table.symbols[symbol.index()]
            .declarations
            .is_empty()
        {
            return None;
        }
        Some(self.type_for_symbol(symbol))
    }

    /// Whether the indexer is an array (integer-keyed) indexer.
    fn is_integer_indexer(&self, indexer: ModelIndexer) -> bool {
        indexer.key == self.intrinsics.integer
    }

    /// Checks a standard library declaration of an intrinsic scalar. The type
    /// stays the intrinsic unit variant (so relations and literal checks keep
    /// working); TS `Scalar` declaration details are recorded on the side.
    fn check_intrinsic_scalar_declaration(
        &mut self,
        node_id: NodeId,
        type_id: TypeId,
        extends: Option<NodeId>,
        decorators: Vec<NodeId>,
    ) -> TypeId {
        let symbol = self
            .symbol_for_node(node_id)
            .expect("intrinsic declaration has a symbol");
        self.cache_type(node_id, type_id);
        let namespace = self.namespace_for_symbol(Some(symbol));
        let base_scalar =
            extends.and_then(|extends| self.check_scalar_extends(Some(symbol), extends));
        let decorators = self.collect_decorators(&decorators);
        self.intrinsic_declarations.insert(
            type_id,
            IntrinsicDeclaration {
                node: node_id,
                symbol,
                namespace,
                base_scalar,
                decorators,
            },
        );
        if let Some(namespace) = namespace {
            let name = self.symbol_table.symbols[symbol.index()].name.clone();
            self.register_namespace_member(namespace, name, type_id, NamespaceMemberKind::Scalar);
        }
        type_id
    }

    fn check_enum(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }

        let (id, decorators, members, expression) = match self.node(node_id).data.clone() {
            NodeData::EnumStatement {
                id,
                decorators,
                members,
                ..
            } => (Some(id), decorators, members, false),
            NodeData::EnumDeclarationExpression {
                id,
                decorators,
                members,
            } => (id, decorators, members, true),
            _ => panic!("expected enum statement"),
        };
        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }
        if expression {
            self.report_declaration_expression_constraints(node_id, &[]);
        }

        let mut enum_type = EnumType {
            name: self.optional_identifier_name(id),
            node: Some(node_id),
            symbol,
            members: IndexMap::new(),
            decorators: self.collect_decorators(&decorators),
            namespace,
            expression,
        };

        for member in members {
            match self.node(member).data.clone() {
                NodeData::EnumMember { id, .. } => {
                    let member_name = self.identifier_name(id);
                    let member_id = self.check_enum_member(member, type_id);
                    if enum_type.members.contains_key(&member_name) {
                        self.report(
                            "enum-member-duplicate",
                            &format!("Enum already has a member named {member_name}"),
                            node_id,
                        );
                    } else {
                        enum_type.members.insert(member_name, member_id);
                    }
                }
                NodeData::EnumSpreadMember { target } => {
                    self.check_enum_spread_member(target, type_id, &mut enum_type);
                }
                _ => {}
            }
        }

        let enum_name = enum_type.name.clone();
        self.type_arena.replace(type_id, Type::Enum(enum_type));
        if let (Some(namespace), false) = (namespace, expression) {
            self.register_namespace_member(
                namespace,
                enum_name,
                type_id,
                NamespaceMemberKind::Enum,
            );
        }
        type_id
    }

    fn check_enum_member(&mut self, node_id: NodeId, enum_id: TypeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::EnumMember {
            id,
            value,
            decorators,
        } = self.node(node_id).data.clone()
        else {
            panic!("expected enum member");
        };
        let symbol = self.symbol_for_node(node_id);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let value = value.and_then(|node| match self.node(node).data.clone() {
            NodeData::StringLiteral { value } => {
                self.string_literal(Some(node), value.clone());
                Some(EnumMemberValue::String(value))
            }
            NodeData::NumericLiteral {
                value_as_string, ..
            } => {
                let value = Numeric::from_str(&value_as_string)
                    .expect("parser emitted invalid numeric literal");
                self.numeric_literal(Some(node), value.clone());
                Some(EnumMemberValue::Numeric(value))
            }
            _ => None,
        });
        let name = self.identifier_name(id);
        let decorators = self.collect_decorators(&decorators);
        self.type_arena.replace(
            type_id,
            Type::EnumMember(EnumMemberType {
                name,
                node: Some(node_id),
                r#enum: enum_id,
                value,
                decorators,
            }),
        );
        type_id
    }

    fn check_enum_spread_member(
        &mut self,
        target: NodeId,
        enum_id: TypeId,
        enum_type: &mut EnumType,
    ) {
        let target_type = self.type_for_node(target);
        let target_type = self.get_non_alias_type(target_type);
        self.spread_enum_members(target_type, target, enum_id, enum_type);
    }

    fn spread_enum_members(
        &mut self,
        target_type: TypeId,
        target: NodeId,
        enum_id: TypeId,
        enum_type: &mut EnumType,
    ) {
        let Type::Enum(source_enum) = self.type_arena.get(target_type).clone() else {
            if !matches!(
                self.type_arena.get(target_type),
                Type::Error | Type::Unknown
            ) {
                self.report(
                    "spread-enum",
                    "Cannot spread members of non-enum type.",
                    target,
                );
            }
            return;
        };

        for (name, member_id) in source_enum.members {
            if enum_type.members.contains_key(&name) {
                self.report(
                    "enum-member-duplicate",
                    &format!("Enum already has a member named {name}"),
                    target,
                );
                continue;
            }
            let Some(source_member) = self.enum_member_type(member_id).cloned() else {
                continue;
            };
            let cloned = self.type_arena.add(Type::EnumMember(EnumMemberType {
                name: name.clone(),
                node: source_member.node,
                r#enum: enum_id,
                value: source_member.value,
                decorators: source_member.decorators,
            }));
            enum_type.members.insert(name, cloned);
        }
    }

    fn check_union_statement(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }

        let (id, decorators, template_parameter_nodes, extends, options, expression) =
            match self.node(node_id).data.clone() {
                NodeData::UnionStatement {
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    options,
                    ..
                } => (
                    Some(id),
                    decorators,
                    template_parameters,
                    extends,
                    options,
                    false,
                ),
                NodeData::UnionDeclarationExpression {
                    id,
                    decorators,
                    template_parameters,
                    extends,
                    options,
                    ..
                } => (id, decorators, template_parameters, extends, options, true),
                _ => panic!("expected union statement"),
            };
        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        if let Some(extends) = extends.filter(|_| !self.feature_enabled("union-extends", node_id)) {
            self.report(
                "union-extends-disabled",
                "Union `extends` clauses require the 'union-extends' feature to be enabled. Add 'union-extends' to the 'features' list in your tspconfig.yaml.",
                extends,
            );
        }
        let template_parameters = self.check_template_parameters(&template_parameter_nodes);
        if expression {
            self.report_declaration_expression_constraints(node_id, &template_parameter_nodes);
        }
        let is_template_declaration = !template_parameters.is_empty();

        let mut union_type = UnionType {
            name: self.optional_identifier_name(id),
            node: Some(node_id),
            symbol,
            variants: IndexMap::new(),
            expression,
            base_type: None,
            decorators: Vec::new(),
            namespace,
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters,
            is_template_declaration,
        };
        self.type_arena
            .replace(type_id, Type::Union(union_type.clone()));
        if let Some(extends) = extends {
            union_type.base_type =
                self.check_union_base_type(symbol, type_id, &union_type.name, extends);
        }
        union_type.decorators = self.collect_decorators(&decorators);

        for (index, variant) in options.into_iter().enumerate() {
            let variant_id = self.check_union_variant(variant, type_id, index);
            let Some(variant_type) = self.union_variant_type(variant_id) else {
                continue;
            };
            let name = variant_type.name.clone();
            let variant_value = variant_type.variant_type;
            if union_type.variants.contains_key(&name) {
                self.report(
                    "union-duplicate",
                    &format!("Union already has a variant named {name}"),
                    variant,
                );
            } else {
                union_type.variants.insert(name, variant_id);
                if let (Some(base_type), false) = (union_type.base_type, is_template_declaration) {
                    let value_node = match self.node(variant).data {
                        NodeData::UnionVariant { value, .. } => value,
                        _ => variant,
                    };
                    self.check_union_variant_against_base_type(
                        variant_value,
                        base_type,
                        value_node,
                    );
                }
            }
        }

        let union_name = union_type.name.clone();
        self.type_arena.replace(type_id, Type::Union(union_type));
        if let (Some(namespace), false) = (namespace, expression) {
            self.register_namespace_member(
                namespace,
                union_name,
                type_id,
                NamespaceMemberKind::Union,
            );
        }
        type_id
    }

    fn check_union_expression(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::UnionExpression { options } = self.node(node_id).data.clone() else {
            panic!("expected union expression");
        };
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);

        let mut union_type = UnionType {
            base_type: None,
            name: String::new(),
            node: Some(node_id),
            symbol: self.symbol_for_node(node_id),
            variants: IndexMap::new(),
            expression: true,
            decorators: Vec::new(),
            namespace: None,
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };

        for option in options {
            let option_type = self.type_for_node(option);
            let non_alias = self.get_non_alias_type(option_type);
            if matches!(self.type_arena.get(non_alias), Type::Never) {
                continue;
            }
            let nested_expression = match self.type_arena.get(non_alias).clone() {
                Type::Union(nested)
                    if nested.node.is_some_and(|node| {
                        matches!(self.node(node).data, NodeData::UnionExpression { .. })
                    }) =>
                {
                    Some(nested)
                }
                _ => None,
            };
            if let Some(nested) = nested_expression {
                for variant_id in nested.variants.values() {
                    let Some(variant) = self.union_variant_type(*variant_id).cloned() else {
                        continue;
                    };
                    let name = self.synthetic_union_variant_name(union_type.variants.len());
                    let cloned = self.type_arena.add(Type::UnionVariant(UnionVariantType {
                        name: name.clone(),
                        node: variant.node,
                        union: type_id,
                        variant_type: variant.variant_type,
                        decorators: variant.decorators,
                    }));
                    union_type.variants.insert(name, cloned);
                }
            } else {
                let name = self.synthetic_union_variant_name(union_type.variants.len());
                let variant = self.type_arena.add(Type::UnionVariant(UnionVariantType {
                    name: name.clone(),
                    node: None,
                    union: type_id,
                    variant_type: non_alias,
                    decorators: Vec::new(),
                }));
                union_type.variants.insert(name, variant);
            }
        }

        self.type_arena.replace(type_id, Type::Union(union_type));
        type_id
    }

    fn check_intersection_expression(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::IntersectionExpression { options } = self.node(node_id).data.clone() else {
            panic!("expected intersection expression");
        };
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);

        let mut model = ModelType {
            expression: true,
            name: String::new(),
            node: Some(node_id),
            symbol: self.symbol_for_node(node_id),
            properties: IndexMap::new(),
            base_model: None,
            source_model: None,
            indexer: None,
            decorators: Vec::new(),
            namespace: None,
            derived_models: Vec::new(),
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
        let mut indexers = Vec::new();
        let mut model_options = Vec::new();
        for option_node in options {
            let option_type = self.type_for_node(option_node);
            let option_type = self.get_non_alias_type(option_type);
            let Some(option) = self.model_type(option_type) else {
                if !matches!(
                    self.type_arena.get(option_type),
                    Type::Error | Type::Unknown
                ) {
                    self.report(
                        "intersect-non-model",
                        "Cannot intersect non-model types (including union types).",
                        option_node,
                    );
                }
                continue;
            };
            if let Some(indexer) = option.indexer {
                if self.is_integer_indexer(indexer) {
                    self.report(
                        "intersect-invalid-index",
                        "Cannot intersect an array model.",
                        option_node,
                    );
                } else {
                    indexers.push(indexer);
                }
            }
            model_options.push(option_type);
        }

        for option_type in model_options {
            let option_indexer = self.model_type(option_type).and_then(|m| m.indexer);
            for (name, prop_id) in self.walk_model_properties_inherited(option_type) {
                let Some(prop) = self.model_property_type(prop_id).cloned() else {
                    continue;
                };
                if model.properties.contains_key(&name) {
                    self.report(
                        "intersect-duplicate-property",
                        &format!("Intersection contains duplicate property definitions for {name}"),
                        node_id,
                    );
                    continue;
                }
                let cloned = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                    name: name.clone(),
                    node: prop.node,
                    symbol: None,
                    model: type_id,
                    property_type: prop.property_type,
                    optional: prop.optional,
                    default: prop.default,
                    source_property: Some(prop_id),
                    decorators: prop.decorators,
                }));
                model.properties.insert(name, cloned);
                for indexer in indexers.clone() {
                    if Some(indexer) != option_indexer {
                        self.check_property_compatible_with_indexer(indexer, cloned, node_id);
                    }
                }
            }
        }
        if indexers.len() == 1 {
            model.indexer = indexers.first().copied();
        }
        self.type_arena.replace(type_id, Type::Model(model));
        type_id
    }

    fn check_union_variant(
        &mut self,
        node_id: NodeId,
        union_id: TypeId,
        variant_index: usize,
    ) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::UnionVariant {
            id,
            value,
            decorators,
        } = self.node(node_id).data.clone()
        else {
            panic!("expected union variant");
        };
        let symbol = self.symbol_for_node(node_id);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }
        let name = id
            .map(|id| self.identifier_name(id))
            .unwrap_or_else(|| self.synthetic_union_variant_name(variant_index));
        let variant_type = self.type_for_node(value);
        let variant_type = self.get_non_alias_type(variant_type);
        let decorators = self.collect_decorators(&decorators);
        self.type_arena.replace(
            type_id,
            Type::UnionVariant(UnionVariantType {
                name,
                node: Some(node_id),
                union: union_id,
                variant_type,
                decorators,
            }),
        );
        type_id
    }

    fn check_alias(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::AliasStatement {
            id,
            template_parameters,
            value,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected alias statement");
        };
        let symbol = self
            .symbol_for_node(node_id)
            .expect("alias statement should have symbol");
        let namespace = self.namespace_for_symbol(Some(symbol));
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        self.symbol_to_type.insert(symbol, type_id);
        let template_parameters = self.check_template_parameters(&template_parameters);

        self.alias_stack.push(symbol);
        let target = self.type_for_node(value);
        self.alias_stack.pop();

        let name = self.identifier_name(id);
        let register_name = name.clone();
        self.type_arena.replace(
            type_id,
            Type::Alias(AliasType {
                name,
                node: node_id,
                symbol,
                target,
                namespace,
                template_node: None,
                template_arguments: Vec::new(),
                template_parameters: template_parameters.clone(),
                is_template_declaration: !template_parameters.is_empty(),
            }),
        );
        if let Some(namespace) = namespace {
            self.register_namespace_member(
                namespace,
                register_name,
                type_id,
                NamespaceMemberKind::Alias,
            );
        }
        type_id
    }

    fn check_operation(&mut self, node_id: NodeId, parent_interface: Option<TypeId>) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::OperationStatement {
            id,
            decorators,
            template_parameters,
            signature,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected operation statement");
        };

        let symbol = self.symbol_for_node(node_id);
        let interface = parent_interface.or_else(|| self.interface_for_operation_symbol(symbol));
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }
        let template_parameters = self.check_template_parameters(&template_parameters);

        let name = self.identifier_name(id);
        let mut operation = OperationType {
            name: name.clone(),
            node: Some(node_id),
            symbol,
            parameters: self.intrinsics.unknown,
            return_type: self.intrinsics.void,
            decorators: Vec::new(),
            interface,
            namespace,
            source_operation: None,
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: template_parameters.clone(),
            is_template_declaration: !template_parameters.is_empty(),
        };

        match self.node(signature).data.clone() {
            NodeData::OperationSignatureDeclaration {
                parameters,
                return_type,
            } => {
                operation.parameters = self.type_for_node(parameters);
                self.set_model_namespace(operation.parameters, namespace);
                operation.return_type = self.type_for_node(return_type);
                operation.decorators = self.collect_decorators(&decorators);
            }
            NodeData::OperationSignatureReference { base_operation } => {
                if let Some(base) = self.check_operation_is(symbol, base_operation) {
                    operation.source_operation = Some(base);
                    operation.parameters = self.clone_parameter_model(base, namespace);
                    operation.return_type = self
                        .operation_type(base)
                        .map(|source| source.return_type)
                        .unwrap_or(self.intrinsics.void);
                    operation.decorators = self
                        .operation_type(base)
                        .map(|source| source.decorators.clone())
                        .unwrap_or_default();
                    operation
                        .decorators
                        .extend(self.collect_decorators(&decorators));
                } else {
                    operation.parameters = self.empty_parameter_model(namespace);
                    operation.return_type = self.intrinsics.void;
                    operation.decorators = self.collect_decorators(&decorators);
                }
            }
            _ => {}
        }

        self.type_arena.replace(type_id, Type::Operation(operation));
        if interface.is_none() {
            if let Some(namespace) = namespace {
                self.register_namespace_member(
                    namespace,
                    name,
                    type_id,
                    NamespaceMemberKind::Operation,
                );
            }
        }
        type_id
    }

    fn check_operation_is(
        &mut self,
        operation_symbol: Option<SymbolId>,
        base_operation: NodeId,
    ) -> Option<TypeId> {
        if let Some(symbol) = operation_symbol {
            self.operation_stack.push(symbol);
        }
        let target_symbol = self.resolved_symbol(base_operation);
        if let Some(target_symbol) = target_symbol {
            if self.operation_stack.contains(&target_symbol) {
                self.report_operation_cycle(target_symbol, base_operation);
                if operation_symbol.is_some() {
                    self.operation_stack.pop();
                }
                return None;
            }
        }

        let base = self.type_for_node(base_operation);
        if operation_symbol.is_some() {
            self.operation_stack.pop();
        }
        let base = self.get_non_alias_type(base);
        if matches!(self.type_arena.get(base), Type::Operation(_)) {
            Some(base)
        } else if !matches!(self.type_arena.get(base), Type::Error | Type::Unknown) {
            self.report(
                "is-operation",
                "Operation can only reuse the signature of another operation.",
                base_operation,
            );
            None
        } else {
            None
        }
    }

    fn check_interface(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::InterfaceStatement {
            id,
            decorators,
            template_parameters,
            extends,
            operations,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected interface statement");
        };
        let symbol = self
            .symbol_for_node(node_id)
            .expect("interface statement should have symbol");
        let namespace = self.namespace_for_symbol(Some(symbol));
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        self.symbol_to_type.insert(symbol, type_id);
        let template_parameters = self.check_template_parameters(&template_parameters);

        let mut own_members = IndexMap::new();
        for operation_node in operations {
            let operation_id = self.check_operation(operation_node, Some(type_id));
            let Some(operation) = self.operation_type(operation_id) else {
                continue;
            };
            let name = operation.name.clone();
            if own_members.contains_key(&name) {
                self.report(
                    "interface-duplicate",
                    &format!("Interface already has a member named {name}"),
                    operation_node,
                );
            } else {
                own_members.insert(name, operation_id);
            }
        }

        let mut interface_type = InterfaceType {
            name: self.identifier_name(id),
            node: Some(node_id),
            symbol: Some(symbol),
            operations: IndexMap::new(),
            source_interfaces: Vec::new(),
            decorators: self.collect_decorators(&decorators),
            namespace,
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: template_parameters.clone(),
            is_template_declaration: !template_parameters.is_empty(),
        };

        for extends_node in extends {
            let extends_type = self.type_for_node(extends_node);
            let extends_type = self.get_non_alias_type(extends_type);
            let Some(source_interface) = self.interface_type(extends_type).cloned() else {
                if !matches!(
                    self.type_arena.get(extends_type),
                    Type::Error | Type::Unknown
                ) {
                    self.report(
                        "extends-interface",
                        "Interfaces can only extend other interfaces",
                        extends_node,
                    );
                }
                continue;
            };
            for (name, operation_id) in source_interface.operations {
                if interface_type.operations.contains_key(&name) {
                    self.report(
                        "extends-interface-duplicate",
                        &format!(
                            "Interface extends cannot have duplicate members. The duplicate member is named {name}"
                        ),
                        extends_node,
                    );
                }
                let cloned = self.clone_operation_for_interface(operation_id, type_id, namespace);
                interface_type.operations.insert(name, cloned);
            }
            interface_type.source_interfaces.push(extends_type);
        }

        for (name, operation_id) in own_members {
            interface_type.operations.insert(name, operation_id);
        }

        let interface_name = interface_type.name.clone();
        self.type_arena
            .replace(type_id, Type::Interface(interface_type));
        if let Some(namespace) = namespace {
            self.register_namespace_member(
                namespace,
                interface_name,
                type_id,
                NamespaceMemberKind::Interface,
            );
        }
        type_id
    }

    fn check_model_property(&mut self, node_id: NodeId, model_id: TypeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::ModelProperty {
            id,
            value,
            decorators,
            optional,
            default,
        } = self.node(node_id).data.clone()
        else {
            panic!("expected model property");
        };
        let symbol = self.symbol_for_node(node_id);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        self.alias_structural_depth += 1;
        let property_type = self.type_for_node(value);
        self.alias_structural_depth -= 1;
        let default_node = default;
        let default = default_node.map(|node| self.type_for_default_node(node));
        let decorators = self.collect_decorators(&decorators);
        let name = self.identifier_name(id);
        self.type_arena.replace(
            type_id,
            Type::ModelProperty(ModelPropertyType {
                name,
                node: Some(node_id),
                symbol,
                model: model_id,
                property_type,
                optional,
                default,
                source_property: None,
                decorators,
            }),
        );
        if let Some(default_node) = default_node.filter(|node| self.is_value_node(*node)) {
            // TS `checkDefaultValue`: object / array / const / scalar constructor values.
            if !matches!(self.type_arena.get(property_type), Type::Error) {
                let diagnostics = self.check_value_of_type(default_node, property_type);
                self.diagnostics.extend(diagnostics);
            }
        } else if !optional {
            if let (Some(default_type), Some(default_node)) = (default, default_node) {
                let diagnostics = {
                    let relation = TypeRelationChecker::new(&self.type_arena);
                    relation.check_type_assignable_span(
                        default_type,
                        property_type,
                        self.node_span(default_node),
                    )
                };
                self.diagnostics.extend(diagnostics);
            }
        }
        type_id
    }

    fn type_for_node(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        match self.node(node_id).data.clone() {
            NodeData::TypeReference { .. }
            | NodeData::Identifier { .. }
            | NodeData::MemberExpression { .. } => self.type_for_reference(node_id),
            NodeData::ModelExpression { .. } => self.check_model_expression(node_id),
            NodeData::ArrayExpression { element_type } => {
                self.check_array_expression(node_id, element_type)
            }
            NodeData::UnionExpression { .. } => self.check_union_expression(node_id),
            NodeData::IntersectionExpression { .. } => self.check_intersection_expression(node_id),
            NodeData::TupleExpression { values } => self.check_tuple_expression(node_id, values),
            NodeData::StringTemplateExpression { head, spans } => {
                self.check_string_template_expression(node_id, head, spans)
            }
            NodeData::StringLiteral { value } => self.string_literal(Some(node_id), value),
            NodeData::NumericLiteral {
                value_as_string, ..
            } => self.numeric_literal(
                Some(node_id),
                Numeric::from_str(&value_as_string)
                    .expect("parser emitted invalid numeric literal"),
            ),
            NodeData::BooleanLiteral { value } => self.boolean_literal(Some(node_id), value),
            NodeData::TemplateParameterDeclaration { .. } => self.check_template_parameter(node_id),
            NodeData::TemplateArgument { argument, .. } => self.type_for_node(argument),
            NodeData::ModelStatement { .. } => self.check_model_statement(node_id),
            NodeData::ScalarStatement { .. } => self.check_scalar(node_id),
            NodeData::EnumStatement { .. } => self.check_enum(node_id),
            NodeData::UnionStatement { .. } => self.check_union_statement(node_id),
            NodeData::ModelDeclarationExpression { .. } => self.check_model_statement(node_id),
            NodeData::ScalarDeclarationExpression { .. } => self.check_scalar(node_id),
            NodeData::EnumDeclarationExpression { .. } => self.check_enum(node_id),
            NodeData::UnionDeclarationExpression { .. } => self.check_union_statement(node_id),
            NodeData::AliasStatement { .. } => self.check_alias(node_id),
            NodeData::OperationStatement { .. } => self.check_operation(node_id, None),
            NodeData::InterfaceStatement { .. } => self.check_interface(node_id),
            NodeData::NamespaceStatement { .. } => self.check_namespace(node_id),
            NodeData::Keyword => self.type_for_keyword(node_id),
            _ => self.intrinsics.unknown,
        }
    }

    fn type_for_default_node(&mut self, node_id: NodeId) -> TypeId {
        match self.node(node_id).data {
            NodeData::Keyword => self.type_for_keyword(node_id),
            _ => self.type_for_node(node_id),
        }
    }

    fn type_for_keyword(&mut self, node_id: NodeId) -> TypeId {
        let type_id = match self.node(node_id).kind {
            SyntaxKind::VoidKeyword => self.intrinsics.void,
            SyntaxKind::NeverKeyword => self.intrinsics.never,
            SyntaxKind::UnknownKeyword => self.intrinsics.unknown,
            _ => self.intrinsics.null,
        };
        self.cache_type(node_id, type_id);
        type_id
    }

    fn type_for_reference(&mut self, node_id: NodeId) -> TypeId {
        let argument_nodes = match self.node(node_id).data.clone() {
            NodeData::TypeReference { arguments, .. } => arguments,
            _ => Vec::new(),
        };
        let resolution = match self.resolutions.get(&node_id).cloned() {
            Some(resolution @ (ResolutionResult::Resolved(_) | ResolutionResult::Ambiguous(_))) => {
                Some(resolution)
            }
            _ => self
                .reference_target(node_id)
                .and_then(|target| self.resolutions.get(&target))
                .cloned(),
        };
        let symbol = match resolution {
            Some(ResolutionResult::Resolved(symbol)) => symbol,
            Some(ResolutionResult::Ambiguous(_)) => {
                // TS `checkTypeReference` yields the error type for ambiguous references.
                let error = self.type_arena.add(Type::Error);
                self.cache_type(node_id, error);
                return error;
            }
            _ => {
                let Some(symbol) = self.fallback_reference_symbol(node_id) else {
                    self.cache_type(node_id, self.intrinsics.unknown);
                    return self.intrinsics.unknown;
                };
                symbol
            }
        };
        let type_id = self.type_for_symbol(symbol);
        self.check_reference_deprecation(node_id, symbol, type_id);
        let result = if self
            .symbol_table
            .symbols
            .get(symbol.index())
            .is_some_and(|symbol| symbol.flags.contains(SymbolFlags::TemplateParameter))
        {
            self.mark_template_parameter_used(type_id);
            type_id
        } else if self.template_parameter_ids(type_id).is_empty() {
            if !argument_nodes.is_empty() {
                self.report(
                    "invalid-template-args",
                    "Can't pass template arguments to non-templated type",
                    node_id,
                );
            }
            type_id
        } else {
            let args = self.resolve_template_arguments(type_id, node_id, &argument_nodes);
            self.instantiate_template(type_id, args, node_id)
        };
        self.cache_type(node_id, result);
        result
    }

    fn type_for_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self.alias_stack.contains(&symbol)
            && self.symbol_table.symbols[symbol.index()]
                .flags
                .contains(crate::types::SymbolFlags::Alias)
        {
            if self.alias_structural_depth > 0 {
                if let Some(type_id) = self.symbol_to_type.get(&symbol).copied() {
                    return type_id;
                }
            }
            self.report_alias_cycle(symbol);
            return self.type_arena.add(Type::Error);
        }
        if let Some(source) = self.symbol_table.symbols[symbol.index()].symbol_source {
            return self.type_for_symbol(source);
        }
        if let Some(type_id) = self.symbol_to_type.get(&symbol).copied() {
            return type_id;
        }
        if self.symbol_table.symbols[symbol.index()]
            .declarations
            .is_empty()
        {
            if let Some(type_id) = self
                .intrinsics
                .by_name(&self.symbol_table.symbols[symbol.index()].name)
            {
                self.symbol_to_type.insert(symbol, type_id);
                return type_id;
            }
        }
        let Some(node) = self.symbol_table.symbols[symbol.index()]
            .declarations
            .first()
            .copied()
        else {
            return self.intrinsics.unknown;
        };
        match self.node(node).data {
            NodeData::TemplateParameterDeclaration { .. } => self.check_template_parameter(node),
            NodeData::ModelStatement { .. } => self.check_model_statement(node),
            NodeData::ScalarStatement { .. } => self.check_scalar(node),
            NodeData::EnumStatement { .. } => self.check_enum(node),
            NodeData::UnionStatement { .. } => self.check_union_statement(node),
            NodeData::AliasStatement { .. } => self.check_alias(node),
            NodeData::OperationStatement { .. } => self.check_operation(node, None),
            NodeData::InterfaceStatement { .. } => self.check_interface(node),
            NodeData::NamespaceStatement { .. } => self.check_namespace(node),
            NodeData::ModelProperty { .. } => self
                .type_cache
                .get(&node)
                .copied()
                .unwrap_or(self.intrinsics.unknown),
            _ => self.intrinsics.unknown,
        }
    }

    fn fallback_reference_symbol(&self, node_id: NodeId) -> Option<SymbolId> {
        let target = self.reference_target(node_id).unwrap_or(node_id);
        let name = self.identifier_name(target);
        self.symbol_table
            .symbols
            .iter()
            .enumerate()
            .find_map(|(index, symbol)| {
                (symbol.name == name
                    && !symbol.declarations.is_empty()
                    && symbol.flags.intersects(
                        SymbolFlags::Model
                            | SymbolFlags::Scalar
                            | SymbolFlags::Alias
                            | SymbolFlags::Interface
                            | SymbolFlags::Operation
                            | SymbolFlags::Union
                            | SymbolFlags::Enum
                            | SymbolFlags::Namespace
                            | SymbolFlags::TemplateParameter,
                    ))
                .then_some(SymbolId::new(index as u32))
            })
    }

    fn check_array_expression(&mut self, node_id: NodeId, element_type: NodeId) -> TypeId {
        self.alias_structural_depth += 1;
        let element_is_plain_reference = matches!(
            self.node(element_type).data.clone(),
            NodeData::TypeReference { arguments, .. } if arguments.is_empty()
        );
        let element = if !element_is_plain_reference {
            self.type_for_node(element_type)
        } else {
            match self.resolved_symbol(node_id) {
                Some(symbol) => {
                    let element = self.type_for_symbol(symbol);
                    self.check_reference_deprecation(element_type, symbol, element);
                    // `T[]` uses the template parameter like a plain `T` reference.
                    self.mark_template_parameter_used(element);
                    element
                }
                None => self.type_for_node(element_type),
            }
        };
        self.alias_structural_depth -= 1;
        if let Some(array_decl) = self.std_template("Array") {
            let instance = self.instantiate_template(array_decl, vec![element], node_id);
            self.cache_type(node_id, instance);
            return instance;
        }
        let model = ModelType {
            expression: false,
            name: String::new(),
            node: Some(node_id),
            symbol: self.symbol_for_node(node_id),
            properties: IndexMap::new(),
            base_model: None,
            source_model: None,
            indexer: Some(ModelIndexer {
                key: self.intrinsics.integer,
                value: element,
            }),
            decorators: Vec::new(),
            namespace: None,
            derived_models: Vec::new(),
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
        self.add_and_cache(node_id, Type::Model(model))
    }

    /// TS `checkTupleExpression`.
    fn check_tuple_expression(&mut self, node_id: NodeId, values: Vec<NodeId>) -> TypeId {
        let values = values
            .into_iter()
            .map(|value| self.type_for_node(value))
            .collect();
        self.add_and_cache(
            node_id,
            Type::Tuple(TupleType {
                node: Some(node_id),
                values,
            }),
        )
    }

    /// TS `checkStringTemplateExpresion` in the type world: literal spans plus interpolated
    /// type spans; `string_value` is set when every interpolated type serializes to a string.
    fn check_string_template_expression(
        &mut self,
        node_id: NodeId,
        head: NodeId,
        spans: Vec<NodeId>,
    ) -> TypeId {
        let mut parts = Vec::new();
        for span in spans {
            let NodeData::StringTemplateSpan {
                expression,
                literal,
            } = self.node(span).data.clone()
            else {
                continue;
            };
            let span_type = self.type_for_node(expression);
            parts.push((expression, span_type, literal));
        }
        let template = self.build_string_template(node_id, head, &parts);
        self.add_and_cache(node_id, template)
    }

    fn build_string_template(
        &mut self,
        node_id: NodeId,
        head: NodeId,
        parts: &[(NodeId, TypeId, NodeId)],
    ) -> Type {
        let mut string_value = Some(self.template_literal_value(head));
        let mut spans = vec![self.template_span_literal(head)];
        for &(expression, span_type, literal) in parts {
            spans.push(
                self.type_arena
                    .add(Type::StringTemplateSpan(StringTemplateSpanType {
                        node: Some(expression),
                        is_interpolated: true,
                        span_type,
                    })),
            );
            let serialized = self.stringify_type_for_template(span_type);
            string_value = match (string_value, serialized) {
                (Some(value), Some(part)) => Some(value + &part),
                _ => None,
            };
            spans.push(self.template_span_literal(literal));
            let literal_value = self.template_literal_value(literal);
            if let Some(value) = string_value.as_mut() {
                value.push_str(&literal_value);
            }
        }
        Type::StringTemplate(StringTemplateType {
            node: Some(node_id),
            spans,
            string_value,
        })
    }

    fn template_literal_value(&self, node: NodeId) -> String {
        match &self.node(node).data {
            NodeData::StringTemplateLiteral { value, .. } => value.clone(),
            _ => String::new(),
        }
    }

    /// TS `createTemplateSpanLiteral`.
    fn template_span_literal(&mut self, node: NodeId) -> TypeId {
        let value = self.template_literal_value(node);
        let literal = self.string_literal(None, value);
        self.type_arena
            .add(Type::StringTemplateSpan(StringTemplateSpanType {
                node: Some(node),
                is_interpolated: false,
                span_type: literal,
            }))
    }

    /// TS `stringifyTypeForTemplate`.
    fn stringify_type_for_template(&self, type_id: TypeId) -> Option<String> {
        match self.type_arena.get(self.get_non_alias_type(type_id)) {
            Type::StringLiteral(literal) => Some(literal.value.clone()),
            Type::NumericLiteral(literal) => Some(literal.value.to_string()),
            Type::BooleanLiteral(literal) => Some(literal.value.to_string()),
            Type::StringTemplate(template) => template.string_value.clone(),
            _ => None,
        }
    }

    /// Type of a template argument or default; object / array values become a
    /// [`Self::value_placeholders`] entry.
    fn template_argument_type(&mut self, node: NodeId) -> TypeId {
        if matches!(
            self.node(node).data,
            NodeData::ObjectLiteral { .. } | NodeData::ArrayLiteral { .. }
        ) {
            if let Some(placeholder) = self.type_cache.get(&node).copied() {
                return placeholder;
            }
            let placeholder = self.type_arena.add(Type::Unknown);
            self.value_placeholders.insert(placeholder, node);
            self.cache_type(node, placeholder);
            return placeholder;
        }
        self.type_for_node(node)
    }

    fn check_template_parameters(&mut self, params: &[NodeId]) -> Vec<TypeId> {
        params
            .iter()
            .map(|param| self.check_template_parameter(*param))
            .collect()
    }

    fn check_template_parameter(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }
        let NodeData::TemplateParameterDeclaration {
            id,
            constraint,
            default,
        } = self.node(node_id).data.clone()
        else {
            panic!("expected template parameter declaration");
        };
        let symbol = self
            .symbol_for_node(node_id)
            .expect("template parameter should have symbol");
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        self.symbol_to_type.insert(symbol, type_id);
        self.template_parameter_usage
            .entry(node_id)
            .or_insert(false);

        let constraint = constraint.map(|constraint| self.type_for_node(constraint));
        if let Some(default_node) = default {
            self.report_invalid_later_template_default(node_id, default_node);
        }
        let default_node = default;
        let default = default.map(|default| self.template_argument_type(default));
        if let (Some(default), Some(default_node), Some(constraint)) =
            (default, default_node, constraint)
        {
            let diagnostics = {
                let relation = TypeRelationChecker::new(&self.type_arena);
                relation.check_type_assignable_span(
                    default,
                    constraint,
                    self.node_span(default_node),
                )
            };
            self.diagnostics.extend(diagnostics);
        }

        self.type_arena.replace(
            type_id,
            Type::TemplateParameter(TemplateParameterType {
                name: self.identifier_name(id),
                node: node_id,
                symbol,
                constraint,
                default,
            }),
        );
        type_id
    }

    fn report_invalid_later_template_default(&mut self, param_node: NodeId, default_node: NodeId) {
        let Some(param_symbol) = self.symbol_for_node(param_node) else {
            return;
        };
        let parent = self.symbol_table.symbols[param_symbol.index()].parent_symbol;
        let siblings = self
            .symbol_table
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.parent_symbol == parent
                    && symbol.flags.contains(SymbolFlags::TemplateParameter)
            })
            .filter_map(|symbol| symbol.declarations.first().copied())
            .collect::<Vec<_>>();
        let Some(index) = siblings.iter().position(|node| *node == param_node) else {
            return;
        };
        let later = siblings[index..]
            .iter()
            .filter_map(|node| self.symbol_for_node(*node))
            .collect::<HashSet<_>>();
        if self.node_references_any_symbol(default_node, &later) {
            self.report(
                "invalid-template-default",
                "Template parameter defaults can only reference previously declared type parameters.",
                default_node,
            );
        }
    }

    fn node_references_any_symbol(&self, node_id: NodeId, symbols: &HashSet<SymbolId>) -> bool {
        if self
            .resolved_symbol(node_id)
            .is_some_and(|symbol| symbols.contains(&symbol))
        {
            return true;
        }
        match self.node(node_id).data.clone() {
            NodeData::TypeReference { target, arguments } => {
                self.node_references_any_symbol(target, symbols)
                    || arguments
                        .iter()
                        .any(|node| self.node_references_any_symbol(*node, symbols))
            }
            NodeData::TemplateArgument { argument, .. } => {
                self.node_references_any_symbol(argument, symbols)
            }
            NodeData::MemberExpression { base, id, .. } => {
                self.node_references_any_symbol(base, symbols)
                    || self.node_references_any_symbol(id, symbols)
            }
            NodeData::ArrayExpression { element_type } => {
                self.node_references_any_symbol(element_type, symbols)
            }
            NodeData::UnionExpression { options }
            | NodeData::IntersectionExpression { options } => options
                .iter()
                .any(|node| self.node_references_any_symbol(*node, symbols)),
            NodeData::ModelExpression { properties, .. } => properties
                .iter()
                .any(|node| self.node_references_any_symbol(*node, symbols)),
            NodeData::ModelProperty { value, default, .. } => {
                self.node_references_any_symbol(value, symbols)
                    || default.is_some_and(|node| self.node_references_any_symbol(node, symbols))
            }
            _ => false,
        }
    }

    fn mark_template_parameter_used(&mut self, type_id: TypeId) {
        if let Type::TemplateParameter(param) = self.type_arena.get(type_id) {
            self.template_parameter_usage.insert(param.node, true);
        }
    }

    /// Whether `node` is in a library or compiler (standard library) file.
    fn is_library_node(&self, node: NodeId) -> bool {
        let pos = self.node(node).pos;
        self.parse_result.library_scripts.iter().any(|script| {
            let script = self.node(*script);
            script.pos <= pos && pos <= script.end
        })
    }

    fn report_unused_template_parameters(&mut self) {
        let mut unused = self
            .template_parameter_usage
            .iter()
            .filter_map(|(node, used)| (!*used).then_some(*node))
            .collect::<Vec<_>>();
        // Source order, like the TS linter's program navigation.
        unused.sort_by_key(|node| (self.node(*node).pos, node.as_u32()));
        for node in unused {
            // A linter rule in TS: only reported in project files.
            if self.is_library_node(node) {
                continue;
            }
            let Some(symbol) = self.symbol_for_node(node) else {
                continue;
            };
            let parameter_name = self.symbol_table.symbols[symbol.index()].name.clone();
            let type_name = self.symbol_table.symbols[symbol.index()]
                .parent_symbol
                .map(|parent| self.symbol_table.symbols[parent.index()].name.clone())
                .unwrap_or_default();
            self.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                // Linter rule diagnostic (full rule code): the JS host reports it only when the
                // rule is enabled and no error occurred, like the TS linter stage (M13).
                code: "@typespec/compiler/unused-template-parameter",
                message: format!(
                    "Templates should use all specified parameters, and parameter '{parameter_name}' does not exist in type '{type_name}'. Consider removing this parameter."
                ),
                target: self.node_span(node),
            });
        }
    }

    fn template_parameter_ids(&self, type_id: TypeId) -> Vec<TypeId> {
        match self.type_arena.get(type_id) {
            Type::Model(ty) => ty.template_parameters.clone(),
            Type::Scalar(ty) => ty.template_parameters.clone(),
            Type::Alias(ty) => ty.template_parameters.clone(),
            Type::Union(ty) => ty.template_parameters.clone(),
            Type::Interface(ty) => ty.template_parameters.clone(),
            Type::Operation(ty) => ty.template_parameters.clone(),
            _ => Vec::new(),
        }
    }

    fn template_declaration_symbol(&self, type_id: TypeId) -> Option<SymbolId> {
        match self.type_arena.get(type_id) {
            Type::Model(ty) => ty.symbol,
            Type::Scalar(ty) => ty.symbol,
            Type::Alias(ty) => Some(ty.symbol),
            Type::Union(ty) => ty.symbol,
            Type::Interface(ty) => ty.symbol,
            Type::Operation(ty) => ty.symbol,
            _ => None,
        }
    }

    fn template_node(&self, type_id: TypeId) -> Option<NodeId> {
        match self.type_arena.get(type_id) {
            Type::Model(ty) => ty.template_node,
            Type::Scalar(ty) => ty.template_node,
            Type::Alias(ty) => ty.template_node,
            Type::Union(ty) => ty.template_node,
            Type::Interface(ty) => ty.template_node,
            Type::Operation(ty) => ty.template_node,
            _ => None,
        }
    }

    fn template_arguments(&self, type_id: TypeId) -> Vec<TypeId> {
        match self.type_arena.get(type_id) {
            Type::Model(ty) => ty.template_arguments.clone(),
            Type::Scalar(ty) => ty.template_arguments.clone(),
            Type::Alias(ty) => ty.template_arguments.clone(),
            Type::Union(ty) => ty.template_arguments.clone(),
            Type::Interface(ty) => ty.template_arguments.clone(),
            Type::Operation(ty) => ty.template_arguments.clone(),
            _ => Vec::new(),
        }
    }

    fn resolve_template_arguments(
        &mut self,
        template_decl: TypeId,
        node_id: NodeId,
        argument_nodes: &[NodeId],
    ) -> Vec<TypeId> {
        let params = self.template_parameter_ids(template_decl);
        let mut positional = vec![None; params.len()];
        let mut named_seen = false;
        for (index, arg_node) in argument_nodes.iter().enumerate() {
            let NodeData::TemplateArgument { name, argument } = self.node(*arg_node).data.clone()
            else {
                continue;
            };
            let arg_type = self.template_argument_type(argument);
            if let Some(name_node) = name {
                named_seen = true;
                let name = self.identifier_name(name_node);
                let param_index = params.iter().position(|param| {
                    matches!(self.type_arena.get(*param), Type::TemplateParameter(param) if param.name == name)
                });
                if let Some(param_index) = param_index {
                    if positional[param_index].is_some() {
                        self.report(
                            "invalid-template-args",
                            &format!("Cannot specify template argument '{name}' again."),
                            *arg_node,
                        );
                    } else {
                        positional[param_index] = Some((arg_type, *arg_node));
                    }
                } else {
                    self.report(
                        "invalid-template-args",
                        &format!("No parameter named '{name}' exists in the target template."),
                        *arg_node,
                    );
                }
            } else if named_seen {
                self.report(
                    "invalid-template-args",
                    "Positional template arguments cannot follow named arguments in the same argument list.",
                    *arg_node,
                );
            } else if index >= params.len() {
                self.report(
                    "invalid-template-args",
                    "Too many template arguments provided.",
                    node_id,
                );
            } else if positional[index].is_none() {
                positional[index] = Some((arg_type, *arg_node));
            }
        }

        let mut result = Vec::with_capacity(params.len());
        let mut subst = HashMap::new();
        for (index, param_id) in params.iter().enumerate() {
            let (arg, arg_node) = if let Some((arg, arg_node)) = positional[index] {
                (arg, Some(arg_node))
            } else {
                let default = match self.type_arena.get(*param_id) {
                    Type::TemplateParameter(param) => param.default,
                    _ => None,
                };
                if let Some(default) = default {
                    (self.substitute_type(default, &subst), None)
                } else {
                    let name = match self.type_arena.get(*param_id) {
                        Type::TemplateParameter(param) => param.name.clone(),
                        _ => String::new(),
                    };
                    self.report(
                        "invalid-template-args",
                        &format!("Template argument '{name}' is required and not specified."),
                        node_id,
                    );
                    let constraint = match self.type_arena.get(*param_id) {
                        Type::TemplateParameter(param) => {
                            param.constraint.unwrap_or(self.intrinsics.unknown)
                        }
                        _ => self.intrinsics.unknown,
                    };
                    (constraint, None)
                }
            };
            let constraint = match self.type_arena.get(*param_id) {
                Type::TemplateParameter(param) => param.constraint,
                _ => None,
            };
            // Mirrors TS `checkArgumentAssignable`: only explicit arguments are
            // validated here; defaults are validated at their declaration.
            let arg = match (constraint, arg_node) {
                (Some(constraint), Some(arg_node))
                    if !self.value_placeholders.contains_key(&arg) =>
                {
                    let constraint = self.substitute_type(constraint, &subst);
                    let diagnostics = {
                        let relation = TypeRelationChecker::new(&self.type_arena);
                        relation.check_type_assignable_span(
                            arg,
                            constraint,
                            self.node_span(arg_node),
                        )
                    };
                    if diagnostics.is_empty() {
                        arg
                    } else {
                        self.report(
                            "invalid-argument",
                            &format!(
                                "Argument of type '{}' is not assignable to parameter of type '{}'",
                                type_name(&self.type_arena, arg),
                                type_name(&self.type_arena, constraint)
                            ),
                            arg_node,
                        );
                        constraint
                    }
                }
                _ => arg,
            };
            subst.insert(*param_id, arg);
            result.push(arg);
        }
        result
    }

    fn instantiate_template(
        &mut self,
        template_decl: TypeId,
        template_args: Vec<TypeId>,
        target: NodeId,
    ) -> TypeId {
        let Some(symbol) = self.template_declaration_symbol(template_decl) else {
            return template_decl;
        };
        if let Some(cached) = self.template_instantiations.get(symbol, &template_args) {
            return cached;
        }
        let params = self.template_parameter_ids(template_decl);
        if params.is_empty() {
            return template_decl;
        }
        let subst = params
            .iter()
            .copied()
            .zip(template_args.iter().copied())
            .collect::<HashMap<_, _>>();
        let type_id = self.type_arena.reserve();
        self.template_instantiations
            .insert(symbol, template_args.clone(), type_id);
        self.record_instance_context(template_decl, type_id, &subst);

        let ty = self.type_arena.get(template_decl).clone();
        let instantiated = match ty {
            Type::Model(model) => Type::Model(self.instantiate_model(
                template_decl,
                type_id,
                model,
                template_args,
                &subst,
            )),
            Type::Scalar(scalar) => Type::Scalar(ScalarType {
                expression: scalar.expression,
                name: scalar.name,
                node: scalar.node,
                symbol: scalar.symbol,
                base_scalar: scalar
                    .base_scalar
                    .map(|base| self.substitute_type(base, &subst)),
                decorators: scalar.decorators,
                namespace: scalar.namespace,
                template_node: scalar.node,
                template_arguments: template_args,
                template_parameters: Vec::new(),
                is_template_declaration: false,
            }),
            Type::Alias(alias) => Type::Alias(AliasType {
                name: alias.name,
                node: alias.node,
                symbol: alias.symbol,
                target: self.substitute_type(alias.target, &subst),
                namespace: alias.namespace,
                template_node: Some(alias.node),
                template_arguments: template_args,
                template_parameters: Vec::new(),
                is_template_declaration: false,
            }),
            Type::Union(union) => {
                Type::Union(self.instantiate_union(type_id, union, template_args, &subst))
            }
            Type::Interface(interface) => Type::Interface(self.instantiate_interface(
                type_id,
                interface,
                template_args,
                &subst,
            )),
            Type::Operation(operation) => Type::Operation(OperationType {
                name: operation.name,
                node: operation.node,
                symbol: operation.symbol,
                parameters: self.substitute_type(operation.parameters, &subst),
                return_type: self.substitute_type(operation.return_type, &subst),
                decorators: operation.decorators,
                interface: operation.interface,
                namespace: operation.namespace,
                source_operation: Some(template_decl),
                template_node: operation.node,
                template_arguments: template_args,
                template_parameters: Vec::new(),
                is_template_declaration: false,
            }),
            _ => {
                self.report(
                    "invalid-template-args",
                    "Invalid template arguments.",
                    target,
                );
                Type::Error
            }
        };
        self.type_arena.replace(type_id, instantiated);
        type_id
    }

    fn instantiate_model(
        &mut self,
        source_id: TypeId,
        model_id: TypeId,
        source: ModelType,
        template_args: Vec<TypeId>,
        subst: &HashMap<TypeId, TypeId>,
    ) -> ModelType {
        let mut model = ModelType {
            expression: source.expression,
            name: source.name,
            node: source.node,
            symbol: source.symbol,
            properties: IndexMap::new(),
            base_model: source
                .base_model
                .map(|base| self.substitute_type(base, subst)),
            source_model: source
                .source_model
                .map(|source| self.substitute_type(source, subst)),
            indexer: source.indexer.map(|indexer| ModelIndexer {
                key: self.substitute_type(indexer.key, subst),
                value: self.substitute_type(indexer.value, subst),
            }),
            decorators: source.decorators,
            namespace: source.namespace,
            derived_models: Vec::new(),
            template_node: source.node.or(source.template_node),
            template_arguments: template_args,
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
        if source_id != model_id {
            self.record_instance_context(source_id, model_id, subst);
        }
        for (name, prop_id) in source.properties {
            let Some(prop) = self.model_property_type(prop_id).cloned() else {
                continue;
            };
            let property_type = self.substitute_type(prop.property_type, subst);
            let default = prop
                .default
                .map(|default| self.substitute_type(default, subst));
            let cloned = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                name: name.clone(),
                node: prop.node,
                symbol: None,
                model: model_id,
                property_type,
                optional: prop.optional,
                default,
                source_property: prop.source_property.or(Some(prop_id)),
                decorators: prop.decorators,
            }));
            self.record_instance_context(prop_id, cloned, subst);
            model.properties.insert(name, cloned);
        }
        if let Some(spreads) = self.template_parameter_spreads.get(&source_id).cloned() {
            self.apply_template_parameter_spreads(model_id, &mut model, &spreads, subst);
        }
        if let Some(base_model) = model.base_model {
            if let Type::Model(base) = self.type_arena.get_mut(base_model) {
                if source_id != model_id && !base.derived_models.contains(&model_id) {
                    base.derived_models.push(model_id);
                }
            }
        }
        model
    }

    /// Inserts the properties of models substituted for spread template
    /// parameters (`model Foo<T> { ...T }`) at their declaration positions.
    fn apply_template_parameter_spreads(
        &mut self,
        model_id: TypeId,
        model: &mut ModelType,
        spreads: &[(usize, TypeId)],
        subst: &HashMap<TypeId, TypeId>,
    ) {
        let mut existing = std::mem::take(&mut model.properties).into_iter();
        let mut consumed = 0;
        for &(index, param) in spreads {
            while consumed < index {
                if let Some((name, prop)) = existing.next() {
                    model.properties.insert(name, prop);
                }
                consumed += 1;
            }
            let source = self.substitute_type(param, subst);
            let source = self.get_non_alias_type(source);
            let Some(source_model) = self.model_type(source) else {
                continue;
            };
            if source_model
                .indexer
                .is_some_and(|indexer| self.is_integer_indexer(indexer))
            {
                continue;
            }
            for (name, prop_id) in self.walk_model_properties_inherited(source) {
                if model.properties.contains_key(&name) {
                    continue;
                }
                let Some(prop) = self.model_property_type(prop_id).cloned() else {
                    continue;
                };
                let cloned = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                    name: name.clone(),
                    node: prop.node,
                    symbol: None,
                    model: model_id,
                    property_type: prop.property_type,
                    optional: prop.optional,
                    default: prop.default,
                    source_property: Some(prop_id),
                    decorators: prop.decorators,
                }));
                model.properties.insert(name, cloned);
            }
        }
        model.properties.extend(existing);
    }

    fn instantiate_union(
        &mut self,
        union_id: TypeId,
        source: UnionType,
        template_args: Vec<TypeId>,
        subst: &HashMap<TypeId, TypeId>,
    ) -> UnionType {
        // Mirrors TS re-checking the union under a mapper: the base type is resolved against the
        // template arguments and the variants are validated against it, unless the instance is
        // still inside a template declaration (its arguments reference template parameters).
        let in_template_declaration = subst
            .values()
            .any(|arg| self.contains_template_parameter(*arg));
        let extends = source
            .node
            .and_then(|node| match self.node(node).data.clone() {
                NodeData::UnionStatement { extends, .. }
                | NodeData::UnionDeclarationExpression { extends, .. } => extends,
                _ => None,
            });
        let base_type = match (source.base_type, extends) {
            (Some(base), _) => Some(self.substitute_type(base, subst)),
            (None, Some(extends)) if !subst.is_empty() => {
                let declared = self.type_for_node(extends);
                let declared = self.get_non_alias_type(declared);
                if matches!(self.type_arena.get(declared), Type::TemplateParameter(_)) {
                    let base = self.substitute_type(declared, subst);
                    let base = self.get_non_alias_type(base);
                    if matches!(self.type_arena.get(base), Type::Error) {
                        None
                    } else {
                        self.validate_union_base_type_kind(base, extends)
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let mut union = UnionType {
            base_type,
            name: source.name,
            node: source.node,
            symbol: source.symbol,
            variants: IndexMap::new(),
            expression: source.expression,
            decorators: source.decorators,
            namespace: source.namespace,
            template_node: source.node.or(source.template_node),
            template_arguments: template_args,
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
        for (name, variant_id) in source.variants {
            let Some(variant) = self.union_variant_type(variant_id).cloned() else {
                continue;
            };
            let variant_type = self.substitute_type(variant.variant_type, subst);
            if let (Some(base_type), false, Some(variant_node)) =
                (union.base_type, in_template_declaration, variant.node)
            {
                let value_node = match self.node(variant_node).data {
                    NodeData::UnionVariant { value, .. } => value,
                    _ => variant_node,
                };
                self.check_union_variant_against_base_type(variant_type, base_type, value_node);
            }
            let cloned = self.type_arena.add(Type::UnionVariant(UnionVariantType {
                name: name.clone(),
                node: variant.node,
                union: union_id,
                variant_type,
                decorators: variant.decorators,
            }));
            self.record_instance_context(variant_id, cloned, subst);
            union.variants.insert(name, cloned);
        }
        union
    }

    fn instantiate_interface(
        &mut self,
        interface_id: TypeId,
        source: InterfaceType,
        template_args: Vec<TypeId>,
        subst: &HashMap<TypeId, TypeId>,
    ) -> InterfaceType {
        let mut interface = InterfaceType {
            name: source.name,
            node: source.node,
            symbol: source.symbol,
            operations: IndexMap::new(),
            source_interfaces: source
                .source_interfaces
                .into_iter()
                .map(|source| self.substitute_type(source, subst))
                .collect(),
            decorators: source.decorators,
            namespace: source.namespace,
            template_node: source.node.or(source.template_node),
            template_arguments: template_args,
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
        for (name, operation_id) in source.operations {
            let Some(operation) = self.operation_type(operation_id).cloned() else {
                continue;
            };
            let parameters = self.substitute_type(operation.parameters, subst);
            let return_type = self.substitute_type(operation.return_type, subst);
            let cloned = self.type_arena.add(Type::Operation(OperationType {
                name: operation.name,
                node: operation.node,
                symbol: None,
                parameters,
                return_type,
                decorators: operation.decorators,
                interface: Some(interface_id),
                namespace: operation.namespace,
                source_operation: Some(operation_id),
                template_node: operation.template_node,
                template_arguments: operation.template_arguments,
                template_parameters: Vec::new(),
                is_template_declaration: false,
            }));
            self.record_instance_context(operation_id, cloned, subst);
            interface.operations.insert(name, cloned);
        }
        interface
    }

    /// Records the template mapper `target` was instantiated with, composed with the mapper of
    /// `source` when it was itself produced by an instantiation (e.g. a property spread from
    /// `Foo<U>` into a template that is later instantiated).
    fn record_instance_context(
        &mut self,
        source: TypeId,
        target: TypeId,
        subst: &HashMap<TypeId, TypeId>,
    ) {
        let mut context = subst.clone();
        if let Some(inner) = self.instance_context(source) {
            for (param, arg) in inner {
                if let std::collections::hash_map::Entry::Vacant(entry) = context.entry(param) {
                    entry.insert(self.substitute_type(arg, subst));
                }
            }
        }
        self.instance_contexts.insert(target, context);
    }

    /// The template mapper a type was instantiated with, following copied properties and
    /// operations (`model is`, spreads, `op is`) back to the instance they were copied from.
    fn instance_context(&self, type_id: TypeId) -> Option<HashMap<TypeId, TypeId>> {
        let mut current = type_id;
        let mut visited = HashSet::new();
        while visited.insert(current) {
            if let Some(context) = self.instance_contexts.get(&current) {
                return Some(context.clone());
            }
            current = match self.type_arena.get(current) {
                Type::ModelProperty(prop) => prop.source_property?,
                Type::Operation(operation) => operation.source_operation?,
                _ => return None,
            };
        }
        None
    }

    /// Whether `type_id` references a template parameter, i.e. a template instance built from it
    /// is still part of a template declaration (TS `hasObservedTemplateParameters`).
    fn contains_template_parameter(&self, type_id: TypeId) -> bool {
        let mut visited = HashSet::new();
        self.contains_template_parameter_inner(type_id, &mut visited)
    }

    fn contains_template_parameter_inner(
        &self,
        type_id: TypeId,
        visited: &mut HashSet<TypeId>,
    ) -> bool {
        if !visited.insert(type_id) {
            return false;
        }
        if self
            .template_arguments(type_id)
            .into_iter()
            .any(|arg| self.contains_template_parameter_inner(arg, visited))
        {
            return true;
        }
        match self.type_arena.get(type_id) {
            Type::TemplateParameter(_) => true,
            Type::Model(model) if model.name.is_empty() => {
                model.indexer.is_some_and(|indexer| {
                    self.contains_template_parameter_inner(indexer.key, visited)
                        || self.contains_template_parameter_inner(indexer.value, visited)
                }) || model.properties.values().any(|prop| {
                    self.model_property_type(*prop).is_some_and(|prop| {
                        self.contains_template_parameter_inner(prop.property_type, visited)
                    })
                })
            }
            Type::Union(union) if union.expression => union.variants.values().any(|variant| {
                self.union_variant_type(*variant).is_some_and(|variant| {
                    self.contains_template_parameter_inner(variant.variant_type, visited)
                })
            }),
            _ => false,
        }
    }

    fn substitute_type(&mut self, type_id: TypeId, subst: &HashMap<TypeId, TypeId>) -> TypeId {
        if let Some(mapped) = subst.get(&type_id).copied() {
            return mapped;
        }
        if let Some(template_node) = self.template_node(type_id) {
            let args = self
                .template_arguments(type_id)
                .into_iter()
                .map(|arg| self.substitute_type(arg, subst))
                .collect::<Vec<_>>();
            if let Some(decl) = self.node_to_type.get(&template_node).copied() {
                return self.instantiate_template(decl, args, template_node);
            }
        }
        match self.type_arena.get(type_id).clone() {
            Type::TemplateParameter(_) => type_id,
            Type::Model(model) if model.name.is_empty() => {
                let model_id = self.type_arena.reserve();
                let cloned = self.instantiate_model(type_id, model_id, model, Vec::new(), subst);
                self.type_arena.replace(model_id, Type::Model(cloned));
                model_id
            }
            Type::Union(union) if union.expression => {
                let union_id = self.type_arena.reserve();
                self.record_instance_context(type_id, union_id, subst);
                let cloned = self.instantiate_union(union_id, union, Vec::new(), subst);
                self.type_arena.replace(union_id, Type::Union(cloned));
                union_id
            }
            Type::Enum(enum_type) if enum_type.expression && !subst.is_empty() => {
                self.instantiate_enum(type_id, enum_type, subst)
            }
            Type::Tuple(tuple) => {
                let values = tuple
                    .values
                    .iter()
                    .map(|value| self.substitute_type(*value, subst))
                    .collect::<Vec<_>>();
                if values == tuple.values {
                    return type_id;
                }
                self.type_arena.add(Type::Tuple(TupleType {
                    node: tuple.node,
                    values,
                }))
            }
            Type::StringTemplate(template) => {
                self.substitute_string_template(type_id, template, subst)
            }
            _ => type_id,
        }
    }

    /// TS re-checks a string template expression under the template mapper.
    fn substitute_string_template(
        &mut self,
        type_id: TypeId,
        template: StringTemplateType,
        subst: &HashMap<TypeId, TypeId>,
    ) -> TypeId {
        let Some(node_id) = template.node else {
            return type_id;
        };
        let NodeData::StringTemplateExpression { head, spans } = self.node(node_id).data.clone()
        else {
            return type_id;
        };
        let interpolated = template
            .spans
            .iter()
            .filter_map(|span| match self.type_arena.get(*span) {
                Type::StringTemplateSpan(span) if span.is_interpolated => Some(span.span_type),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        let mut parts = Vec::new();
        for (span, original) in spans.into_iter().zip(interpolated) {
            let NodeData::StringTemplateSpan {
                expression,
                literal,
            } = self.node(span).data.clone()
            else {
                continue;
            };
            let mapped = self.substitute_type(original, subst);
            changed |= mapped != original;
            parts.push((expression, mapped, literal));
        }
        if !changed {
            return type_id;
        }
        let instance = self.build_string_template(node_id, head, &parts);
        self.type_arena.add(instance)
    }

    /// Mirrors TS re-checking an enum declaration expression under a template mapper: each
    /// instance gets its own enum and spread members are resolved against the template arguments.
    fn instantiate_enum(
        &mut self,
        source_id: TypeId,
        source: EnumType,
        subst: &HashMap<TypeId, TypeId>,
    ) -> TypeId {
        let member_nodes = match source.node.map(|node| self.node(node).data.clone()) {
            Some(NodeData::EnumDeclarationExpression { members, .. }) => members,
            _ => Vec::new(),
        };
        let source_node = source.node;
        let enum_id = self.type_arena.reserve();
        self.record_instance_context(source_id, enum_id, subst);
        let mut enum_type = EnumType {
            members: IndexMap::new(),
            ..source
        };
        for member in member_nodes {
            match self.node(member).data.clone() {
                NodeData::EnumMember { .. } => {
                    let Some(declared_id) = self.type_cache.get(&member).copied() else {
                        continue;
                    };
                    let Some(declared) = self.enum_member_type(declared_id).cloned() else {
                        continue;
                    };
                    if enum_type.members.contains_key(&declared.name) {
                        self.report(
                            "enum-member-duplicate",
                            &format!("Enum already has a member named {}", declared.name),
                            source_node.unwrap_or(member),
                        );
                        continue;
                    }
                    let name = declared.name.clone();
                    let cloned = self.type_arena.add(Type::EnumMember(EnumMemberType {
                        r#enum: enum_id,
                        ..declared
                    }));
                    self.record_instance_context(declared_id, cloned, subst);
                    enum_type.members.insert(name, cloned);
                }
                NodeData::EnumSpreadMember { target } => {
                    let target_type = self.type_for_node(target);
                    let target_type = self.substitute_type(target_type, subst);
                    let target_type = self.get_non_alias_type(target_type);
                    self.spread_enum_members(target_type, target, enum_id, &mut enum_type);
                }
                _ => {}
            }
        }
        self.type_arena.replace(enum_id, Type::Enum(enum_type));
        enum_id
    }

    fn check_model_extends(
        &mut self,
        model_symbol: Option<SymbolId>,
        extends: NodeId,
    ) -> Option<TypeId> {
        let target_symbol = self.resolved_symbol(extends);
        if let Some(target_symbol) = target_symbol {
            if Some(target_symbol) == model_symbol || self.base_stack.contains(&target_symbol) {
                self.report_base_cycle(target_symbol, extends);
                return None;
            }
        }

        if let Some(model_symbol) = model_symbol {
            self.base_stack.push(model_symbol);
        }
        let extends_type = self.type_for_node(extends);
        if model_symbol.is_some() {
            self.base_stack.pop();
        }
        let extends_type = self.get_non_alias_type(extends_type);
        if matches!(self.type_arena.get(extends_type), Type::Model(_)) {
            Some(extends_type)
        } else if !matches!(
            self.type_arena.get(extends_type),
            Type::Error | Type::Unknown
        ) {
            self.report("extend-model", "Models must extend other models.", extends);
            None
        } else {
            None
        }
    }

    fn check_model_is(
        &mut self,
        model_symbol: Option<SymbolId>,
        is_node: Option<NodeId>,
    ) -> Option<TypeId> {
        let is_node = is_node?;
        let target_symbol = self.resolved_symbol(is_node);
        if let Some(target_symbol) = target_symbol {
            if Some(target_symbol) == model_symbol || self.base_stack.contains(&target_symbol) {
                self.report_base_cycle(target_symbol, is_node);
                return None;
            }
        }
        if let Some(model_symbol) = model_symbol {
            self.base_stack.push(model_symbol);
        }
        let source = self.type_for_node(is_node);
        if model_symbol.is_some() {
            self.base_stack.pop();
        }
        let source = self.get_non_alias_type(source);
        let Type::Model(model) = self.type_arena.get(source) else {
            self.report(
                "is-model",
                "Model `is` must specify another model.",
                is_node,
            );
            return None;
        };
        // Array expressions are anonymous models with an indexer in the Rust port, but TS resolves
        // them to the named `Array<T>` model, so they are valid `is` targets.
        if model.name.is_empty() && model.indexer.is_none() {
            self.report(
                "is-model",
                "Model `is` cannot specify a model expression.",
                is_node,
            );
            return None;
        }
        Some(source)
    }

    fn optional_identifier_name(&self, id: Option<NodeId>) -> String {
        id.map(|id| self.identifier_name(id)).unwrap_or_default()
    }

    /// Diagnostics for declarations used in expression position. Like TS,
    /// `declaration-expression-disabled` is skipped when the `declaration-expressions` compiler
    /// feature is enabled for the node's location (M13).
    fn report_declaration_expression_constraints(
        &mut self,
        node_id: NodeId,
        template_parameters: &[NodeId],
    ) {
        if let Some(first) = template_parameters.first() {
            self.report(
                "templated-declaration-in-expression",
                "A declaration used as an expression cannot have template parameters as it cannot be referenced or instantiated.",
                *first,
            );
        }
        if !self.feature_enabled("declaration-expressions", node_id) {
            self.report(
                "declaration-expression-disabled",
                "Declaration expressions require the 'declaration-expressions' feature to be enabled. Add 'declaration-expressions' to the 'features' list in your tspconfig.yaml.",
                node_id,
            );
        }
    }

    /// TS `isCompilerFeatureEnabled(program, feature, node)`: project files use the project
    /// features, library files their library's features, standard library files none (M13).
    pub(crate) fn feature_enabled(&self, feature: &str, node: NodeId) -> bool {
        let pos = self.node(node).pos;
        let contains = |script: &&NodeId| {
            let script = self.node(**script);
            script.pos <= pos && pos <= script.end
        };
        if self
            .parse_result
            .compiler_scripts
            .iter()
            .any(|s| contains(&s))
        {
            return false;
        }
        if let Some(script) = self.parse_result.library_scripts.iter().find(contains) {
            return self
                .options
                .library_features
                .get(script)
                .is_some_and(|features| features.iter().any(|f| f == feature));
        }
        self.options.project_features.iter().any(|f| f == feature)
    }

    fn check_union_base_type(
        &mut self,
        union_symbol: Option<SymbolId>,
        union_id: TypeId,
        union_name: &str,
        extends: NodeId,
    ) -> Option<TypeId> {
        let target_symbol = self.resolved_symbol(extends);
        if let Some(target_symbol) = target_symbol {
            if Some(target_symbol) == union_symbol || self.base_stack.contains(&target_symbol) {
                self.report_base_cycle(target_symbol, extends);
                return None;
            }
        }

        if let Some(union_symbol) = union_symbol {
            self.base_stack.push(union_symbol);
        }
        let base_type = self.type_for_node(extends);
        if union_symbol.is_some() {
            self.base_stack.pop();
        }
        let base_type = self.get_non_alias_type(base_type);
        let unresolved =
            |node| matches!(self.resolutions.get(&node), Some(ResolutionResult::Unknown));
        let is_reference = self.node(extends).kind == SyntaxKind::TypeReference;
        if matches!(self.type_arena.get(base_type), Type::Error)
            || (is_reference
                && (unresolved(extends) || self.reference_target(extends).is_some_and(unresolved)))
        {
            return None;
        }
        if self.union_expression_references(base_type, union_id) {
            let name = if union_name.is_empty() {
                "(anonymous union)"
            } else {
                union_name
            };
            self.report(
                "circular-base-type",
                &format!("Type '{name}' recursively references itself as a base type."),
                extends,
            );
            return None;
        }
        self.validate_union_base_type_kind(base_type, extends)
    }

    fn validate_union_base_type_kind(
        &mut self,
        base_type: TypeId,
        extends: NodeId,
    ) -> Option<TypeId> {
        match self.type_arena.get(base_type) {
            Type::TemplateParameter(_) => None,
            Type::Model(model)
                if model.node.is_some_and(|node| {
                    matches!(self.node(node).data, NodeData::ModelExpression { .. })
                }) =>
            {
                self.report(
                    "extend-union",
                    "Unions cannot extend model expressions.",
                    extends,
                );
                None
            }
            Type::Model(_) | Type::Enum(_) | Type::Union(_) => Some(base_type),
            _ if self.is_scalar_like(base_type) => Some(base_type),
            _ => {
                self.report(
                    "extend-union",
                    "Union `extends` must specify a model, scalar, enum, or union.",
                    extends,
                );
                None
            }
        }
    }

    /// Whether `target` is reachable from `type_id` through union expressions only.
    fn union_expression_references(&self, type_id: TypeId, target: TypeId) -> bool {
        if type_id == target {
            return true;
        }
        let Type::Union(union) = self.type_arena.get(type_id) else {
            return false;
        };
        if !union.expression {
            return false;
        }
        union.variants.values().any(|variant_id| {
            self.union_variant_type(*variant_id).is_some_and(|variant| {
                self.union_expression_references(variant.variant_type, target)
            })
        })
    }

    fn check_union_variant_against_base_type(
        &mut self,
        variant_type: TypeId,
        base_type: TypeId,
        target: NodeId,
    ) {
        if matches!(self.type_arena.get(variant_type), Type::Error) {
            return;
        }
        let diagnostics = {
            let relation = TypeRelationChecker::new(&self.type_arena);
            relation.check_type_assignable_span(variant_type, base_type, self.node_span(target))
        };
        self.diagnostics.extend(diagnostics);
    }

    fn check_scalar_extends(
        &mut self,
        scalar_symbol: Option<SymbolId>,
        extends: NodeId,
    ) -> Option<TypeId> {
        let target_symbol = self.resolved_symbol(extends);
        if let Some(target_symbol) = target_symbol {
            if Some(target_symbol) == scalar_symbol || self.base_stack.contains(&target_symbol) {
                self.report_base_cycle(target_symbol, extends);
                return None;
            }
        }

        if let Some(scalar_symbol) = scalar_symbol {
            self.base_stack.push(scalar_symbol);
        }
        let extends_type = self.type_for_node(extends);
        if scalar_symbol.is_some() {
            self.base_stack.pop();
        }
        let extends_type = self.get_non_alias_type(extends_type);
        if self.is_scalar_like(extends_type) {
            Some(extends_type)
        } else if !matches!(
            self.type_arena.get(extends_type),
            Type::Error | Type::Unknown
        ) {
            self.report("extend-model", "Models must extend other models.", extends);
            None
        } else {
            None
        }
    }

    fn copy_model_is_properties(
        &mut self,
        model_id: TypeId,
        source_model: TypeId,
        model: &mut ModelType,
    ) {
        if !matches!(self.type_arena.get(source_model), Type::Model(_)) {
            return;
        }
        for (name, prop_id) in self.walk_model_properties_inherited(source_model) {
            let Some(prop) = self.model_property_type(prop_id).cloned() else {
                continue;
            };
            let cloned = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                name: name.clone(),
                node: prop.node,
                symbol: None,
                model: model_id,
                property_type: prop.property_type,
                optional: prop.optional,
                default: prop.default,
                source_property: Some(prop_id),
                decorators: prop.decorators,
            }));
            model.properties.insert(name, cloned);
        }
    }

    /// Finds the indexer that applies to a model, either its own or inherited from a base model.
    fn find_indexer(&self, model: &ModelType) -> Option<ModelIndexer> {
        let mut indexer = model.indexer;
        let mut base = model.base_model;
        let mut seen = HashSet::new();
        while indexer.is_none() {
            let base_id = base?;
            if !seen.insert(base_id) {
                return None;
            }
            let base_model = self.model_type(base_id)?;
            indexer = base_model.indexer;
            base = base_model.base_model;
        }
        indexer
    }

    fn report_no_array_properties(&mut self, node: NodeId) {
        self.report(
            "no-array-properties",
            "Array models cannot have any properties.",
            node,
        );
    }

    /// Mirrors TS `checkModelProperties`: checks declared and spread members,
    /// validating them against the model indexer and accumulating spread indexers.
    fn check_model_members(&mut self, members: &[NodeId], model_id: TypeId, model: &mut ModelType) {
        let mut spread_indexers: Vec<ModelIndexer> = Vec::new();
        for &member in members {
            match self.node(member).data {
                NodeData::ModelProperty { id, .. } => {
                    let prop_id = self.check_model_property(member, model_id);
                    if let Some(indexer) = self.find_indexer(model) {
                        self.check_property_compatible_with_indexer(indexer, prop_id, member);
                    }
                    let name = self.identifier_name(id);
                    if model.properties.contains_key(&name) {
                        self.report_duplicate_property(&model.name, &name, member);
                    } else {
                        model.properties.insert(name, prop_id);
                    }
                }
                NodeData::ModelSpreadProperty { target } => {
                    let spread_type = self.type_for_node(target);
                    let spread_type = self.get_non_alias_type(spread_type);
                    if matches!(self.type_arena.get(spread_type), Type::TemplateParameter(_)) {
                        self.template_parameter_spreads
                            .entry(model_id)
                            .or_default()
                            .push((model.properties.len(), spread_type));
                        continue;
                    }
                    let Some(source) = self.check_spread_source(target) else {
                        continue;
                    };
                    let source_props = self.walk_model_properties_inherited(source);
                    let additional_indexer = self.model_type(source).and_then(|m| m.indexer);
                    if self
                        .find_indexer(model)
                        .is_some_and(|indexer| self.is_integer_indexer(indexer))
                        && (!source_props.is_empty() || additional_indexer.is_some())
                    {
                        self.report_no_array_properties(member);
                        continue;
                    }
                    spread_indexers.extend(additional_indexer);
                    for (name, prop_id) in source_props {
                        let Some(prop) = self.model_property_type(prop_id).cloned() else {
                            continue;
                        };
                        let cloned = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                            name: name.clone(),
                            node: prop.node,
                            symbol: None,
                            model: model_id,
                            property_type: prop.property_type,
                            optional: prop.optional,
                            default: prop.default,
                            source_property: Some(prop_id),
                            decorators: prop.decorators,
                        }));
                        if let Some(indexer) = self.find_indexer(model) {
                            self.check_property_compatible_with_indexer(indexer, cloned, member);
                        }
                        if model.properties.contains_key(&name) {
                            self.report_duplicate_property(&model.name, &name, target);
                            continue;
                        }
                        model.properties.insert(name, cloned);
                    }
                }
                _ => {}
            }
        }
        if let Some(first) = spread_indexers.first().copied() {
            let value = if spread_indexers.len() == 1 {
                first.value
            } else {
                let values = spread_indexers.iter().map(|i| i.value).collect::<Vec<_>>();
                self.create_anonymous_union(&values)
            };
            model.indexer = Some(ModelIndexer {
                key: first.key,
                value,
            });
        }
    }

    /// Mirrors TS `checkSpreadProperty` validation; returns the model to spread.
    fn check_spread_source(&mut self, target: NodeId) -> Option<TypeId> {
        let source = self.type_for_node(target);
        let source = self.get_non_alias_type(source);
        match self.type_arena.get(source) {
            Type::TemplateParameter(_) | Type::Error => None,
            Type::Model(model)
                if !model
                    .indexer
                    .is_some_and(|indexer| self.is_integer_indexer(indexer)) =>
            {
                Some(source)
            }
            _ => {
                self.report(
                    "spread-model",
                    "Cannot spread properties of non-model type.",
                    target,
                );
                None
            }
        }
    }

    /// Mirrors TS `checkPropertyCompatibleWithIndexer`.
    fn check_property_compatible_with_indexer(
        &mut self,
        indexer: ModelIndexer,
        prop_id: TypeId,
        target: NodeId,
    ) {
        if self.is_integer_indexer(indexer) {
            self.report_no_array_properties(target);
            return;
        }
        let Some(property_type) = self.model_property_type(prop_id).map(|p| p.property_type) else {
            return;
        };
        let target = match self.node(target).data {
            NodeData::ModelProperty { value, .. } => value,
            _ => target,
        };
        let diagnostics = {
            let relation = TypeRelationChecker::new(&self.type_arena);
            relation.check_type_assignable_span(
                property_type,
                indexer.value,
                self.node_span(target),
            )
        };
        if !diagnostics.is_empty() {
            let message = diagnostics
                .iter()
                .map(|d| format!("  {}", d.message))
                .collect::<Vec<_>>()
                .join("\n");
            self.report(
                "incompatible-indexer",
                &format!("Property is incompatible with indexer:\n{message}"),
                target,
            );
        }
    }

    /// Recognizes the JS-only `TypeSpec.@indexer(key, value)` decorator natively.
    /// Mirrors the arity part of TS `checkDecoratorArguments`. TS checks decorators once per
    /// checked type (template declarations and each instance), so this walks every type.
    fn native_indexer(&mut self, decorators: &[NodeId]) -> Option<ModelIndexer> {
        let implicit = self.symbol_table.implicit_using?;
        let indexer_symbol = self.symbol_table.scopes[implicit.index()]
            .symbols
            .get("@indexer")
            .copied()?;
        for &decorator in decorators {
            let NodeData::DecoratorExpression { target, arguments } =
                self.node(decorator).data.clone()
            else {
                continue;
            };
            if self.resolved_symbol(target) != Some(indexer_symbol) || arguments.len() != 2 {
                continue;
            }
            let key = self.type_for_node(arguments[0]);
            let value = self.type_for_node(arguments[1]);
            return Some(ModelIndexer { key, value });
        }
        None
    }

    fn collect_decorators(&mut self, decorators: &[NodeId]) -> Vec<DecoratorRef> {
        decorators
            .iter()
            .filter_map(|decorator| match self.node(*decorator).data.clone() {
                NodeData::DecoratorExpression { target, arguments } => Some(DecoratorRef {
                    node: *decorator,
                    name: Self::normalize_decorator_name(&self.decorator_name(target)),
                    args: arguments,
                }),
                _ => None,
            })
            .collect()
    }

    fn apply_decorators(&mut self, runner: &mut dyn DecoratorRunner) {
        let template_declaration_nodes = self.template_declaration_nodes();
        let mut index = 0;
        // Decorator argument substitution can instantiate new types; decorate those too.
        while index < self.type_arena.types().len() {
            let type_id = TypeId::new(index as u32);
            index += 1;
            let Some(context) = self.decorator_context(type_id, &template_declaration_nodes) else {
                continue;
            };
            let mut applications = Vec::new();
            if let Some(doc) = self.doc_comment_for_type(type_id) {
                let node = self.type_node(type_id).unwrap_or(self.parse_result.root);
                applications.push(DecoratorApplication {
                    node,
                    name: "@doc".to_string(),
                    args: vec![DecoratorArgument::String(doc)],
                });
            }
            for decorator in self.decorators_for_type(type_id) {
                let application = self.decorator_application(decorator, context.as_ref());
                applications.push(application);
            }
            applications.extend(self.augment_decorator_applications_for_type(type_id));
            for application in applications {
                runner.call_decorator(self, type_id, &application);
            }
        }
    }

    /// Mirrors TS `skipDecorators` for types checked in a template declaration: returns `None`
    /// when decorators must not run on `type_id`, otherwise the template mapper (if any) used to
    /// resolve its decorator arguments.
    fn decorator_context(
        &self,
        type_id: TypeId,
        template_declaration_nodes: &HashSet<NodeId>,
    ) -> Option<Option<HashMap<TypeId, TypeId>>> {
        let context = self.instance_context(type_id);
        if let Some(context) = &context {
            if context
                .values()
                .any(|arg| self.contains_template_parameter(*arg))
            {
                return None;
            }
        }
        if let Some(container) = self.member_container(type_id) {
            self.decorator_context(container, template_declaration_nodes)?;
        }
        if context.is_none()
            && self
                .type_node(type_id)
                .is_some_and(|node| template_declaration_nodes.contains(&node))
        {
            return None;
        }
        Some(context)
    }

    fn member_container(&self, type_id: TypeId) -> Option<TypeId> {
        match self.type_arena.get(type_id) {
            Type::ModelProperty(prop) => Some(prop.model),
            Type::UnionVariant(variant) => Some(variant.union),
            Type::EnumMember(member) => Some(member.r#enum),
            Type::Operation(operation) => operation.interface,
            _ => None,
        }
    }

    /// Nodes lexically inside a templated declaration (including the declaration itself).
    fn template_declaration_nodes(&self) -> HashSet<NodeId> {
        let mut result = HashSet::new();
        let mut stack = self
            .parse_result
            .scripts
            .iter()
            .map(|script| (*script, false))
            .collect::<Vec<_>>();
        while let Some((node_id, inside)) = stack.pop() {
            let inside = inside || Self::is_templated_declaration(&self.node(node_id).data);
            if inside {
                result.insert(node_id);
            }
            stack.extend(
                self.node(node_id)
                    .children()
                    .into_iter()
                    .map(|child| (child, inside)),
            );
        }
        result
    }

    fn is_templated_declaration(data: &NodeData) -> bool {
        match data {
            NodeData::ModelStatement {
                template_parameters,
                ..
            }
            | NodeData::ScalarStatement {
                template_parameters,
                ..
            }
            | NodeData::InterfaceStatement {
                template_parameters,
                ..
            }
            | NodeData::UnionStatement {
                template_parameters,
                ..
            }
            | NodeData::OperationStatement {
                template_parameters,
                ..
            }
            | NodeData::AliasStatement {
                template_parameters,
                ..
            }
            | NodeData::ModelDeclarationExpression {
                template_parameters,
                ..
            }
            | NodeData::ScalarDeclarationExpression {
                template_parameters,
                ..
            }
            | NodeData::UnionDeclarationExpression {
                template_parameters,
                ..
            } => !template_parameters.is_empty(),
            _ => false,
        }
    }

    fn decorators_for_type(&self, type_id: TypeId) -> Vec<DecoratorRef> {
        match self.type_arena.get(type_id) {
            Type::Scalar(ty) => ty.decorators.clone(),
            Type::Model(ty) => ty.decorators.clone(),
            Type::ModelProperty(ty) => ty.decorators.clone(),
            Type::Enum(ty) => ty.decorators.clone(),
            Type::EnumMember(ty) => ty.decorators.clone(),
            Type::Union(ty) => ty.decorators.clone(),
            Type::UnionVariant(ty) => ty.decorators.clone(),
            Type::Operation(ty) => ty.decorators.clone(),
            Type::Interface(ty) => ty.decorators.clone(),
            Type::Namespace(ty) => ty.decorators.clone(),
            _ => self
                .intrinsic_declarations
                .get(&type_id)
                .map(|declaration| declaration.decorators.clone())
                .unwrap_or_default(),
        }
    }

    fn decorator_application(
        &mut self,
        decorator: DecoratorRef,
        context: Option<&HashMap<TypeId, TypeId>>,
    ) -> DecoratorApplication {
        let args = decorator
            .args
            .iter()
            .copied()
            .map(|arg| self.decorator_argument(arg, context))
            .collect();
        DecoratorApplication {
            node: decorator.node,
            name: decorator.name,
            args,
        }
    }

    fn augment_decorator_applications_for_type(
        &mut self,
        target_type_id: TypeId,
    ) -> Vec<DecoratorApplication> {
        let augment_nodes = self.symbol_table.augment_decorators.clone();
        let mut result = Vec::new();
        for augment_node in augment_nodes {
            let NodeData::AugmentDecoratorStatement {
                target,
                target_type,
                arguments,
            } = self.node(augment_node).data.clone()
            else {
                continue;
            };
            if self.type_for_node(target_type) != target_type_id {
                continue;
            }
            result.push(self.decorator_application(
                DecoratorRef {
                    node: augment_node,
                    name: Self::normalize_decorator_name(&self.decorator_name(target)),
                    args: arguments,
                },
                None,
            ));
        }
        result
    }

    fn decorator_argument(
        &mut self,
        node_id: NodeId,
        context: Option<&HashMap<TypeId, TypeId>>,
    ) -> DecoratorArgument {
        match self.node(node_id).data.clone() {
            NodeData::StringLiteral { value } => DecoratorArgument::String(value),
            NodeData::NumericLiteral { value, .. } => DecoratorArgument::Number(value),
            NodeData::BooleanLiteral { value } => DecoratorArgument::Boolean(value),
            NodeData::Identifier { sv } if sv == "null" => DecoratorArgument::None,
            _ => {
                let mut type_id = self.type_for_node(node_id);
                if let Some(context) = context {
                    type_id = self.substitute_type(type_id, context);
                }
                match self.type_arena.get(type_id).clone() {
                    Type::StringLiteral(value) => DecoratorArgument::String(value.value),
                    Type::NumericLiteral(value) => DecoratorArgument::Number(value.value.as_f64()),
                    Type::BooleanLiteral(value) => DecoratorArgument::Boolean(value.value),
                    Type::Null => DecoratorArgument::None,
                    _ => DecoratorArgument::TypeRef(type_id),
                }
            }
        }
    }

    fn doc_comment_for_type(&self, type_id: TypeId) -> Option<String> {
        let node_id = self.type_node(type_id)?;
        let mut text = String::new();
        for doc in &self.node(node_id).docs {
            let NodeData::Doc { content, .. } = self.node(*doc).data.clone() else {
                continue;
            };
            for content_node in content {
                if let NodeData::DocText { text: chunk } = self.node(content_node).data.clone() {
                    text.push_str(&chunk);
                }
            }
        }
        let trimmed = text.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    }

    fn type_node(&self, type_id: TypeId) -> Option<NodeId> {
        match self.type_arena.get(type_id) {
            Type::Scalar(ty) => ty.node,
            Type::Model(ty) => ty.node,
            Type::ModelProperty(ty) => ty.node,
            Type::Enum(ty) => ty.node,
            Type::EnumMember(ty) => ty.node,
            Type::Union(ty) => ty.node,
            Type::UnionVariant(ty) => ty.node,
            Type::Alias(ty) => Some(ty.node),
            Type::Operation(ty) => ty.node,
            Type::Interface(ty) => ty.node,
            Type::Namespace(ty) => ty.node,
            Type::TemplateParameter(ty) => Some(ty.node),
            _ => self
                .intrinsic_declarations
                .get(&type_id)
                .map(|declaration| declaration.node),
        }
    }

    fn decorator_name(&self, node_id: NodeId) -> String {
        match &self.node(node_id).data {
            NodeData::Identifier { sv } => sv.clone(),
            NodeData::TypeReference { target, .. } => self.decorator_name(*target),
            NodeData::MemberExpression { base, id, selector } => {
                format!(
                    "{}{}{}",
                    self.decorator_name(*base),
                    selector,
                    self.decorator_name(*id)
                )
            }
            _ => "<unknown>".to_string(),
        }
    }

    fn normalize_decorator_name(name: &str) -> String {
        if name.contains('@') {
            return name.to_string();
        }
        if let Some((namespace, decorator)) = name.rsplit_once('.') {
            format!("{namespace}.@{decorator}")
        } else {
            format!("@{name}")
        }
    }

    fn is_scalar_like(&self, type_id: TypeId) -> bool {
        matches!(
            self.type_arena.get(type_id),
            Type::Scalar(_)
                | Type::String
                | Type::Bytes
                | Type::Int8
                | Type::Int16
                | Type::Int32
                | Type::Int64
                | Type::Uint8
                | Type::Uint16
                | Type::Uint32
                | Type::Uint64
                | Type::Float
                | Type::Float32
                | Type::Float64
                | Type::Decimal
                | Type::Decimal128
                | Type::Integer
                | Type::Numeric
                | Type::SafeInt
                | Type::Boolean
                | Type::Url
                | Type::PlainDate
                | Type::PlainTime
                | Type::UtcDateTime
                | Type::OffsetDateTime
                | Type::Duration
        )
    }

    fn resolved_symbol(&self, node_id: NodeId) -> Option<SymbolId> {
        let resolution = match self.resolutions.get(&node_id) {
            Some(ResolutionResult::Resolved(symbol)) => Some(ResolutionResult::Resolved(*symbol)),
            _ => self
                .reference_target(node_id)
                .and_then(|target| self.resolutions.get(&target))
                .cloned(),
        };
        match resolution {
            Some(ResolutionResult::Resolved(symbol)) => Some(symbol),
            _ => None,
        }
    }

    fn reference_target(&self, node_id: NodeId) -> Option<NodeId> {
        match self.node(node_id).data {
            NodeData::TypeReference { target, .. } => Some(target),
            _ => None,
        }
    }

    fn model_type(&self, type_id: TypeId) -> Option<&ModelType> {
        match self.type_arena.get(type_id) {
            Type::Model(model) => Some(model),
            _ => None,
        }
    }

    fn walk_model_properties_inherited(&self, model_id: TypeId) -> Vec<(String, TypeId)> {
        let mut result = Vec::new();
        let Some(model) = self.model_type(model_id) else {
            return result;
        };
        for (name, prop_id) in &model.properties {
            result.push((name.clone(), *prop_id));
        }
        if let Some(base) = model.base_model {
            result.extend(self.walk_model_properties_inherited(base));
        }
        result
    }

    fn model_property_type(&self, type_id: TypeId) -> Option<&ModelPropertyType> {
        match self.type_arena.get(type_id) {
            Type::ModelProperty(prop) => Some(prop),
            _ => None,
        }
    }

    fn enum_member_type(&self, type_id: TypeId) -> Option<&EnumMemberType> {
        match self.type_arena.get(type_id) {
            Type::EnumMember(member) => Some(member),
            _ => None,
        }
    }

    fn union_variant_type(&self, type_id: TypeId) -> Option<&UnionVariantType> {
        match self.type_arena.get(type_id) {
            Type::UnionVariant(variant) => Some(variant),
            _ => None,
        }
    }

    fn operation_type(&self, type_id: TypeId) -> Option<&OperationType> {
        match self.type_arena.get(type_id) {
            Type::Operation(operation) => Some(operation),
            _ => None,
        }
    }

    fn interface_type(&self, type_id: TypeId) -> Option<&InterfaceType> {
        match self.type_arena.get(type_id) {
            Type::Interface(interface) => Some(interface),
            _ => None,
        }
    }

    fn empty_parameter_model(&mut self, namespace: Option<TypeId>) -> TypeId {
        self.type_arena.add(Type::Model(ModelType {
            expression: true,
            name: String::new(),
            node: None,
            symbol: None,
            properties: IndexMap::new(),
            base_model: None,
            source_model: None,
            indexer: None,
            decorators: Vec::new(),
            namespace,
            derived_models: Vec::new(),
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: Vec::new(),
            is_template_declaration: false,
        }))
    }

    fn clone_parameter_model(&mut self, operation_id: TypeId, namespace: Option<TypeId>) -> TypeId {
        let Some(operation) = self.operation_type(operation_id).cloned() else {
            return self.empty_parameter_model(namespace);
        };
        let Some(source_model) = self.model_type(operation.parameters).cloned() else {
            return self.empty_parameter_model(namespace);
        };
        let model_id = self.type_arena.reserve();
        let mut model = ModelType {
            expression: source_model.expression,
            name: String::new(),
            node: source_model.node,
            symbol: None,
            properties: IndexMap::new(),
            base_model: source_model.base_model,
            source_model: source_model.source_model,
            indexer: source_model.indexer,
            decorators: source_model.decorators,
            namespace,
            derived_models: Vec::new(),
            template_node: source_model.template_node,
            template_arguments: source_model.template_arguments,
            template_parameters: source_model.template_parameters,
            is_template_declaration: source_model.is_template_declaration,
        };
        for (name, prop_id) in source_model.properties {
            let Some(prop) = self.model_property_type(prop_id).cloned() else {
                continue;
            };
            let cloned = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                name: name.clone(),
                node: prop.node,
                symbol: None,
                model: model_id,
                property_type: prop.property_type,
                optional: prop.optional,
                default: prop.default,
                source_property: prop.source_property.or(Some(prop_id)),
                decorators: prop.decorators,
            }));
            model.properties.insert(name, cloned);
        }
        self.type_arena.replace(model_id, Type::Model(model));
        model_id
    }

    fn clone_operation_for_interface(
        &mut self,
        operation_id: TypeId,
        interface_id: TypeId,
        namespace: Option<TypeId>,
    ) -> TypeId {
        let Some(source) = self.operation_type(operation_id).cloned() else {
            return operation_id;
        };
        let parameters = self.clone_parameter_model(operation_id, namespace);
        self.type_arena.add(Type::Operation(OperationType {
            name: source.name,
            node: source.node,
            symbol: None,
            parameters,
            return_type: source.return_type,
            decorators: source.decorators,
            interface: Some(interface_id),
            namespace,
            source_operation: Some(operation_id),
            template_node: source.template_node,
            template_arguments: source.template_arguments,
            template_parameters: source.template_parameters,
            is_template_declaration: source.is_template_declaration,
        }))
    }

    fn set_model_namespace(&mut self, model_id: TypeId, namespace: Option<TypeId>) {
        if let Type::Model(model) = self.type_arena.get_mut(model_id) {
            model.namespace = namespace;
        }
    }

    fn namespace_for_symbol(&mut self, symbol: Option<SymbolId>) -> Option<TypeId> {
        let mut current =
            symbol.and_then(|symbol| self.symbol_table.symbols[symbol.index()].parent_symbol);
        while let Some(symbol_id) = current {
            let symbol = &self.symbol_table.symbols[symbol_id.index()];
            if symbol.flags.contains(SymbolFlags::Namespace) {
                return Some(self.get_or_create_namespace(symbol_id));
            }
            current = symbol.parent_symbol;
        }
        symbol
            .map(|_| self.global_namespace)
            .or(Some(self.global_namespace))
            .filter(|_| symbol.is_some())
    }

    fn parent_namespace_for_namespace_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let mut current = self.symbol_table.symbols[symbol.index()].parent_symbol;
        while let Some(symbol_id) = current {
            let parent = &self.symbol_table.symbols[symbol_id.index()];
            if parent.flags.contains(SymbolFlags::Namespace) {
                return self.get_or_create_namespace(symbol_id);
            }
            current = parent.parent_symbol;
        }
        self.global_namespace
    }

    fn interface_for_operation_symbol(&mut self, symbol: Option<SymbolId>) -> Option<TypeId> {
        let parent =
            symbol.and_then(|symbol| self.symbol_table.symbols[symbol.index()].parent_symbol)?;
        if !self.symbol_table.symbols[parent.index()]
            .flags
            .contains(SymbolFlags::Interface)
        {
            return None;
        }
        if let Some(type_id) = self.symbol_to_type.get(&parent).copied() {
            return Some(type_id);
        }
        Some(self.type_for_symbol(parent))
    }

    fn register_namespace_member(
        &mut self,
        namespace_id: TypeId,
        name: String,
        type_id: TypeId,
        kind: NamespaceMemberKind,
    ) {
        let Type::Namespace(namespace) = self.type_arena.get_mut(namespace_id) else {
            return;
        };
        match kind {
            NamespaceMemberKind::Model => {
                namespace.models.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Scalar => {
                namespace.scalars.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Enum => {
                namespace.enums.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Union => {
                namespace.unions.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Alias => {
                namespace.aliases.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Operation => {
                namespace.operations.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Interface => {
                namespace.interfaces.entry(name).or_insert(type_id);
            }
            NamespaceMemberKind::Namespace => {
                namespace.namespaces.entry(name).or_insert(type_id);
            }
        };
    }

    fn add_and_cache(&mut self, node_id: NodeId, ty: Type) -> TypeId {
        let type_id = self.type_arena.add(ty);
        self.cache_type(node_id, type_id);
        type_id
    }

    fn string_literal(&mut self, node_id: Option<NodeId>, value: String) -> TypeId {
        let key = LiteralKey::String(value.clone());
        self.literal_type(
            node_id,
            key,
            Type::StringLiteral(StringLiteralType { value }),
        )
    }

    fn numeric_literal(&mut self, node_id: Option<NodeId>, value: Numeric) -> TypeId {
        let key = LiteralKey::Numeric(value.clone());
        self.literal_type(
            node_id,
            key,
            Type::NumericLiteral(NumericLiteralType { value }),
        )
    }

    fn boolean_literal(&mut self, node_id: Option<NodeId>, value: bool) -> TypeId {
        let key = LiteralKey::Boolean(value);
        self.literal_type(
            node_id,
            key,
            Type::BooleanLiteral(BooleanLiteralType { value }),
        )
    }

    fn literal_type(&mut self, node_id: Option<NodeId>, key: LiteralKey, ty: Type) -> TypeId {
        let type_id = if let Some(type_id) = self.literal_cache.get(&key).copied() {
            type_id
        } else {
            let type_id = self.type_arena.add(ty);
            self.literal_cache.insert(key, type_id);
            type_id
        };
        if let Some(node_id) = node_id {
            self.cache_type(node_id, type_id);
        }
        type_id
    }

    fn get_non_alias_type(&self, type_id: TypeId) -> TypeId {
        let mut current = type_id;
        let mut seen = HashSet::new();
        while seen.insert(current) {
            match self.type_arena.get(current) {
                Type::Alias(alias) => current = alias.target,
                _ => return current,
            }
        }
        current
    }

    fn synthetic_union_variant_name(&self, index: usize) -> String {
        format!("Symbol(name)#{index}")
    }

    /// Mirrors TS `createUnion`: a node-less expression union of `options`.
    fn create_anonymous_union(&mut self, options: &[TypeId]) -> TypeId {
        let type_id = self.type_arena.reserve();
        let mut variants = IndexMap::new();
        for (index, option) in options.iter().enumerate() {
            let name = self.synthetic_union_variant_name(index);
            let variant = self.type_arena.add(Type::UnionVariant(UnionVariantType {
                name: name.clone(),
                node: None,
                union: type_id,
                variant_type: *option,
                decorators: Vec::new(),
            }));
            variants.insert(name, variant);
        }
        self.type_arena.replace(
            type_id,
            Type::Union(UnionType {
                base_type: None,
                name: String::new(),
                node: None,
                symbol: None,
                variants,
                expression: true,
                decorators: Vec::new(),
                namespace: None,
                template_node: None,
                template_arguments: Vec::new(),
                template_parameters: Vec::new(),
                is_template_declaration: false,
            }),
        );
        type_id
    }

    fn cache_type(&mut self, node_id: NodeId, type_id: TypeId) {
        self.type_cache.insert(node_id, type_id);
        self.node_to_type.insert(node_id, type_id);
    }

    fn symbol_for_node(&self, node_id: NodeId) -> Option<SymbolId> {
        self.symbol_table.node_symbols.get(&node_id).copied()
    }

    fn identifier_name(&self, node_id: NodeId) -> String {
        match &self.node(node_id).data {
            NodeData::Identifier { sv } => sv.clone(),
            NodeData::TypeReference { target, .. } => self.identifier_name(*target),
            NodeData::MemberExpression { base, id, selector } => {
                format!(
                    "{}{}{}",
                    self.identifier_name(*base),
                    selector,
                    self.identifier_name(*id)
                )
            }
            _ => "<unknown>".to_string(),
        }
    }

    fn report_duplicate_property(&mut self, model_name: &str, prop_name: &str, target: NodeId) {
        let message = if model_name.is_empty() {
            format!("Model already has a property named {prop_name}")
        } else {
            format!("Model {model_name} already has a property named {prop_name}")
        };
        self.report("duplicate-property", &message, target);
    }

    fn report_base_cycle(&mut self, symbol: SymbolId, target: NodeId) {
        if !self.reported_base_cycles.insert(symbol) {
            return;
        }
        let name = self.symbol_table.symbols[symbol.index()].name.clone();
        self.report(
            "circular-base-type",
            &format!("Type '{name}' recursively references itself as a base type."),
            target,
        );
    }

    fn report_alias_cycle(&mut self, symbol: SymbolId) {
        if !self.reported_alias_cycles.insert(symbol) {
            return;
        }
        let name = self.symbol_table.symbols[symbol.index()].name.clone();
        let target = self.symbol_table.symbols[symbol.index()]
            .declarations
            .first()
            .copied()
            .unwrap_or(self.parse_result.root);
        self.report(
            "circular-alias-type",
            &format!("Alias type '{name}' recursively references itself."),
            target,
        );
    }

    fn report_operation_cycle(&mut self, symbol: SymbolId, target: NodeId) {
        if !self.reported_operation_cycles.insert(symbol) {
            return;
        }
        let name = self.symbol_table.symbols[symbol.index()].name.clone();
        self.report(
            "circular-op-signature",
            &format!("Operation '{name}' recursively references itself."),
            target,
        );
    }

    fn report(&mut self, code: &'static str, message: &str, target: NodeId) {
        self.diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code,
            message: message.to_string(),
            target: self.node_span(target),
        });
    }

    fn is_member_duplicate_diagnostic(&self, span: Span) -> bool {
        self.symbol_table
            .symbols
            .iter()
            .filter(|symbol| symbol.flags.contains(crate::types::SymbolFlags::Member))
            .flat_map(|symbol| symbol.declarations.iter())
            .any(|node| self.node_span(*node) == span)
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

impl DecoratorContext for Checker<'_> {
    fn set_decorator_property(
        &mut self,
        decorator_name: &str,
        target: TypeId,
        property: &str,
        value: DecoratorValue,
    ) {
        let key = if property.contains(':') {
            property.to_string()
        } else {
            decorator_state_key(decorator_name, property)
        };
        self.decorator_state
            .entry(target)
            .or_default()
            .insert(key, value);
    }

    fn get_decorator_property(
        &self,
        decorator_name: &str,
        target: TypeId,
        property: &str,
    ) -> Option<DecoratorValue> {
        let state = self.decorator_state.get(&target)?;
        if property.contains(':') {
            state.get(property).cloned()
        } else {
            state
                .get(&decorator_state_key(decorator_name, property))
                .or_else(|| state.get(property))
                .cloned()
        }
    }

    fn get_type_name(&self, target: TypeId) -> String {
        type_name_only(&self.type_arena, target)
    }

    fn get_type_kind(&self, target: TypeId) -> String {
        self.type_arena.get(target).kind_name().to_string()
    }

    fn report_decorator_diagnostic(
        &mut self,
        code: String,
        message: String,
        target: Option<TypeId>,
        decorator_node: NodeId,
    ) {
        let target = target
            .and_then(|type_id| self.type_node(type_id))
            .unwrap_or(decorator_node);
        self.diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: Box::leak(code.into_boxed_str()),
            message,
            target: self.node_span(target),
        });
    }
}

#[cfg(test)]
mod tests {
    use typespec_parser::parse;
    use typespec_source::SourceFile;

    use super::*;
    use crate::{bind, resolve_names};

    fn check_source(source: &str) -> CheckResult {
        let source = SourceFile::new(source.to_string());
        let parse_result = parse(&source);
        let mut table = bind(&parse_result);
        let resolution = resolve_names(&parse_result, &mut table);
        let mut result = check(&parse_result, &table, &resolution.resolutions);
        result.diagnostics.extend(resolution.diagnostics);
        result
    }

    fn check_source_with_runner(source: &str, runner: &mut dyn DecoratorRunner) -> CheckResult {
        let source = SourceFile::new(source.to_string());
        let parse_result = parse(&source);
        let mut table = bind(&parse_result);
        let resolution = resolve_names(&parse_result, &mut table);
        let mut result =
            check_with_decorator_runner(&parse_result, &table, &resolution.resolutions, runner);
        result.diagnostics.extend(resolution.diagnostics);
        result
    }

    struct RecordingRunner;

    impl DecoratorRunner for RecordingRunner {
        fn is_enabled(&self) -> bool {
            true
        }

        fn call_decorator(
            &mut self,
            context: &mut dyn DecoratorContext,
            target: TypeId,
            application: &DecoratorApplication,
        ) {
            match application.name.as_str() {
                "@doc" => {
                    if let Some(DecoratorArgument::String(value)) = application.args.first() {
                        context.set_decorator_property(
                            "@doc",
                            target,
                            "doc",
                            DecoratorValue::String(value.clone()),
                        );
                    } else {
                        context.report_decorator_diagnostic(
                            "invalid-argument-count".to_string(),
                            "Missing doc argument.".to_string(),
                            Some(target),
                            application.node,
                        );
                    }
                }
                "@key" if context.get_type_kind(target) != "ModelProperty" => {
                    context.report_decorator_diagnostic(
                        "decorator-wrong-target".to_string(),
                        "@key can only target model properties.".to_string(),
                        Some(target),
                        application.node,
                    );
                }
                _ => {}
            }
        }
    }

    fn find_named_model(result: &CheckResult, name: &str) -> TypeId {
        result
            .type_arena
            .types()
            .iter()
            .enumerate()
            .find_map(|(id, ty)| match ty {
                Type::Model(model) if model.name == name => Some(TypeId::new(id as u32)),
                _ => None,
            })
            .expect("model should exist")
    }

    fn find_named_type(result: &CheckResult, name: &str, kind: &str) -> TypeId {
        result
            .type_arena
            .types()
            .iter()
            .enumerate()
            .find_map(|(id, ty)| match (kind, ty) {
                ("Enum", Type::Enum(en)) if en.name == name => Some(TypeId::new(id as u32)),
                ("Union", Type::Union(union)) if union.name == name => Some(TypeId::new(id as u32)),
                ("Alias", Type::Alias(alias)) if alias.name == name => Some(TypeId::new(id as u32)),
                ("Operation", Type::Operation(operation)) if operation.name == name => {
                    Some(TypeId::new(id as u32))
                }
                ("Interface", Type::Interface(interface)) if interface.name == name => {
                    Some(TypeId::new(id as u32))
                }
                ("Namespace", Type::Namespace(namespace)) if namespace.name == name => {
                    Some(TypeId::new(id as u32))
                }
                _ => None,
            })
            .expect("type should exist")
    }

    fn property_type(result: &CheckResult, model_id: TypeId, name: &str) -> TypeId {
        let Type::Model(model) = result.type_arena.get(model_id) else {
            panic!("expected model");
        };
        let Type::ModelProperty(prop) = result.type_arena.get(model.properties[name]) else {
            panic!("expected property");
        };
        prop.property_type
    }

    #[test]
    fn registers_stable_intrinsics() {
        let result = check_source("model Foo { x: string; y: int32; }");
        assert!(matches!(
            result.type_arena.get(TypeId::new(0)),
            Type::Numeric
        ));
        assert!(matches!(
            result.type_arena.get(TypeId::new(16)),
            Type::String
        ));
        assert!(matches!(result.type_arena.get(TypeId::new(3)), Type::Int32));
    }

    #[test]
    fn creates_model_with_property() {
        let result = check_source("model Foo { x: string; }");
        let foo = find_named_model(&result, "Foo");
        let Type::Model(model) = result.type_arena.get(foo) else {
            panic!("expected model");
        };
        let prop_id = model.properties["x"];
        let Type::ModelProperty(prop) = result.type_arena.get(prop_id) else {
            panic!("expected property");
        };
        assert_eq!(prop.model, foo);
        assert!(matches!(
            result.type_arena.get(prop.property_type),
            Type::String
        ));
    }

    #[test]
    fn handles_circular_model_references() {
        let result = check_source("model A { b: B; } model B { a: A; }");
        let a = find_named_model(&result, "A");
        let b = find_named_model(&result, "B");
        let Type::Model(model_a) = result.type_arena.get(a) else {
            panic!("expected model A");
        };
        let Type::ModelProperty(prop_b) = result.type_arena.get(model_a.properties["b"]) else {
            panic!("expected property b");
        };
        assert_eq!(prop_b.property_type, b);
        let Type::Model(model_b) = result.type_arena.get(b) else {
            panic!("expected model B");
        };
        let Type::ModelProperty(prop_a) = result.type_arena.get(model_b.properties["a"]) else {
            panic!("expected property a");
        };
        assert_eq!(prop_a.property_type, a);
    }

    #[test]
    fn reports_duplicate_property() {
        let result = check_source("model Foo { x: string; x: int32; }");
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "duplicate-property"));
    }

    #[test]
    fn creates_enum_with_mixed_value_members() {
        let result = check_source(r#"enum Status { Active, Pending: "pending", Code: 42 }"#);
        let status = find_named_type(&result, "Status", "Enum");
        let Type::Enum(en) = result.type_arena.get(status) else {
            panic!("expected enum");
        };
        assert_eq!(
            en.members.keys().cloned().collect::<Vec<_>>(),
            vec!["Active", "Pending", "Code"]
        );
        let Type::EnumMember(active) = result.type_arena.get(en.members["Active"]) else {
            panic!("expected enum member");
        };
        assert!(active.value.is_none());
        let Type::EnumMember(pending) = result.type_arena.get(en.members["Pending"]) else {
            panic!("expected enum member");
        };
        assert!(matches!(
            pending.value,
            Some(EnumMemberValue::String(ref value)) if value == "pending"
        ));
        let Type::EnumMember(code) = result.type_arena.get(en.members["Code"]) else {
            panic!("expected enum member");
        };
        assert!(matches!(
            code.value,
            Some(EnumMemberValue::Numeric(ref value)) if value == &Numeric::from_str("42").unwrap()
        ));
    }

    #[test]
    fn distinguishes_declared_and_expression_unions() {
        let result =
            check_source("union Result { ok: string } model Foo { value: string | int32; }");
        let result_union = find_named_type(&result, "Result", "Union");
        let Type::Union(declared) = result.type_arena.get(result_union) else {
            panic!("expected declared union");
        };
        assert!(!declared.expression);
        let expression_union = result
            .type_arena
            .types()
            .iter()
            .find(|ty| matches!(ty, Type::Union(union) if union.expression))
            .expect("expression union should exist");
        let Type::Union(expression) = expression_union else {
            panic!("expected union");
        };
        assert_eq!(expression.variants.len(), 2);
    }

    #[test]
    fn reports_circular_alias() {
        let result = check_source("alias A = B; alias B = A;");
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "circular-alias-type"));
    }

    #[test]
    fn canonicalizes_literal_types() {
        let result = check_source(r#"alias A = 42; alias B = 42; alias C = "x"; alias D = "x";"#);
        let a = find_named_type(&result, "A", "Alias");
        let b = find_named_type(&result, "B", "Alias");
        let c = find_named_type(&result, "C", "Alias");
        let d = find_named_type(&result, "D", "Alias");
        let Type::Alias(alias_a) = result.type_arena.get(a) else {
            panic!("expected alias");
        };
        let Type::Alias(alias_b) = result.type_arena.get(b) else {
            panic!("expected alias");
        };
        assert_eq!(alias_a.target, alias_b.target);
        let Type::Alias(alias_c) = result.type_arena.get(c) else {
            panic!("expected alias");
        };
        let Type::Alias(alias_d) = result.type_arena.get(d) else {
            panic!("expected alias");
        };
        assert_eq!(alias_c.target, alias_d.target);
    }

    #[test]
    fn allows_recursive_model_expression_alias() {
        let result = check_source("alias A = { a: A };");
        assert!(!result
            .diagnostics
            .iter()
            .any(|d| d.code == "circular-alias-type"));
        let alias = find_named_type(&result, "A", "Alias");
        let Type::Alias(alias_type) = result.type_arena.get(alias) else {
            panic!("expected alias");
        };
        let Type::Model(model) = result.type_arena.get(alias_type.target) else {
            panic!("expected model expression target");
        };
        let Type::ModelProperty(prop) = result.type_arena.get(model.properties["a"]) else {
            panic!("expected property");
        };
        assert_eq!(prop.property_type, alias);
    }

    #[test]
    fn spreads_model_properties_through_aliases() {
        let result = check_source("model Foo { x: string; } alias A = Foo; model Bar { ...A }");
        let bar = find_named_model(&result, "Bar");
        let Type::Model(model) = result.type_arena.get(bar) else {
            panic!("expected model");
        };
        assert!(model.properties.contains_key("x"));
    }

    #[test]
    fn reports_spread_properties_in_array_models_once_and_skips_them() {
        let result = check_source(
            "model S { name: string; } model B is string[]; model A extends B { ...S; ...S; }",
        );
        let count = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "no-array-properties")
            .count();
        assert_eq!(count, 2);
        assert!(!result
            .diagnostics
            .iter()
            .any(|d| d.code == "duplicate-property"));
        let Type::Model(model) = result.type_arena.get(find_named_model(&result, "A")) else {
            panic!("expected model");
        };
        assert!(model.properties.is_empty());
    }

    #[test]
    fn allows_empty_spreads_in_array_models() {
        let result = check_source("model S {} model A is string[] { ...S; }");
        assert!(!result
            .diagnostics
            .iter()
            .any(|d| d.code == "no-array-properties"));
    }

    #[test]
    fn creates_global_namespace() {
        let result = check_source("model Foo {}");
        let Type::Namespace(global) = result.type_arena.get(TypeId::new(29)) else {
            panic!("expected global namespace after intrinsics");
        };
        assert_eq!(global.name, "");
        assert!(global.namespace.is_none());
        assert!(global.models.contains_key("Foo"));
    }

    #[test]
    fn creates_nested_namespace_parent_chain() {
        let result = check_source("namespace A.B.C { model Foo {} }");
        let a = find_named_type(&result, "A", "Namespace");
        let b = find_named_type(&result, "B", "Namespace");
        let c = find_named_type(&result, "C", "Namespace");
        let Type::Namespace(ns_b) = result.type_arena.get(b) else {
            panic!("expected namespace B");
        };
        let Type::Namespace(ns_c) = result.type_arena.get(c) else {
            panic!("expected namespace C");
        };
        assert_eq!(ns_b.namespace, Some(a));
        assert_eq!(ns_c.namespace, Some(b));
    }

    #[test]
    fn operation_parameters_are_anonymous_model() {
        let result = check_source("op foo(name: string, age: int32): void;");
        let op = find_named_type(&result, "foo", "Operation");
        let Type::Operation(operation) = result.type_arena.get(op) else {
            panic!("expected operation");
        };
        let Type::Model(parameters) = result.type_arena.get(operation.parameters) else {
            panic!("expected parameter model");
        };
        assert_eq!(parameters.name, "");
        assert_eq!(
            parameters.properties.keys().cloned().collect::<Vec<_>>(),
            vec!["name", "age"]
        );
    }

    #[test]
    fn interface_inheritance_copies_operations() {
        let result =
            check_source("interface Base { list(): string; } interface Child extends Base {}");
        let child = find_named_type(&result, "Child", "Interface");
        let Type::Interface(interface) = result.type_arena.get(child) else {
            panic!("expected interface");
        };
        assert!(interface.operations.contains_key("list"));
        assert_eq!(interface.source_interfaces.len(), 1);
    }

    #[test]
    fn namespace_reopening_merges_into_single_type() {
        let result = check_source("namespace A { model X {} } namespace A { model Y {} }");
        let namespaces = result
            .type_arena
            .types()
            .iter()
            .filter(|ty| matches!(ty, Type::Namespace(ns) if ns.name == "A"))
            .count();
        assert_eq!(namespaces, 1);
        let a = find_named_type(&result, "A", "Namespace");
        let Type::Namespace(namespace) = result.type_arena.get(a) else {
            panic!("expected namespace");
        };
        assert!(namespace.models.contains_key("X"));
        assert!(namespace.models.contains_key("Y"));
    }

    #[test]
    fn creates_template_parameter_types() {
        let result = check_source("model Foo<T> { value: T; }");
        let foo = find_named_model(&result, "Foo");
        let Type::Model(model) = result.type_arena.get(foo) else {
            panic!("expected model");
        };
        assert!(model.is_template_declaration);
        assert_eq!(model.template_parameters.len(), 1);
        let value_type = property_type(&result, foo, "value");
        assert_eq!(value_type, model.template_parameters[0]);
        assert!(matches!(
            result.type_arena.get(value_type),
            Type::TemplateParameter(param) if param.name == "T"
        ));
    }

    #[test]
    fn memoizes_same_template_instantiation() {
        let result =
            check_source("model Foo<T> { value: T; } model X { a: Foo<string>; b: Foo<string>; }");
        let x = find_named_model(&result, "X");
        let a = property_type(&result, x, "a");
        let b = property_type(&result, x, "b");
        assert_eq!(a, b);
        let Type::Model(foo_string) = result.type_arena.get(a) else {
            panic!("expected instantiated model");
        };
        assert!(!foo_string.is_template_declaration);
        assert_eq!(foo_string.template_arguments.len(), 1);
        assert!(matches!(
            result.type_arena.get(foo_string.template_arguments[0]),
            Type::String
        ));
        assert!(matches!(
            result.type_arena.get(property_type(&result, a, "value")),
            Type::String
        ));
    }

    #[test]
    fn distinguishes_different_template_arguments() {
        let result =
            check_source("model Foo<T> { value: T; } model X { a: Foo<string>; b: Foo<int32>; }");
        let x = find_named_model(&result, "X");
        let a = property_type(&result, x, "a");
        let b = property_type(&result, x, "b");
        assert_ne!(a, b);
    }

    #[test]
    fn applies_template_parameter_defaults() {
        let result = check_source("model Foo<T = string> { value: T; } model X { a: Foo; }");
        let x = find_named_model(&result, "X");
        let a = property_type(&result, x, "a");
        assert!(matches!(
            result.type_arena.get(property_type(&result, a, "value")),
            Type::String
        ));
    }

    #[test]
    fn validates_template_parameter_constraints() {
        let result =
            check_source("model Foo<T extends string> { value: T; } model X { a: Foo<boolean>; }");
        let diagnostics = result
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str(), d.target.pos))
            .collect::<Vec<_>>();
        assert_eq!(
            diagnostics,
            [(
                "invalid-argument",
                "Argument of type 'boolean' is not assignable to parameter of type 'string'",
                59
            )]
        );
    }

    #[test]
    fn invalid_template_parameter_default_is_reported_once_at_declaration() {
        let result =
            check_source("model Foo<T extends string = int32> { value: T; } model X { a: Foo; }");
        let diagnostics = result
            .diagnostics
            .iter()
            .map(|d| (d.code, d.target.pos))
            .collect::<Vec<_>>();
        assert_eq!(diagnostics, [("unassignable", 29)]);
    }

    #[test]
    fn reports_missing_template_argument() {
        let result = check_source("model Foo<T> { value: T; } model X { a: Foo; }");
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "invalid-template-args"
                && d.message == "Template argument 'T' is required and not specified."));
    }

    #[test]
    fn validates_union_template_instance_variants_against_base_type() {
        let unassignable = |source: &str| {
            check_source(source)
                .diagnostics
                .into_iter()
                .filter(|d| d.code == "unassignable")
                .map(|d| d.message)
                .collect::<Vec<_>>()
        };
        assert!(unassignable("union U<T> extends string { a: T }").is_empty());
        assert_eq!(
            unassignable("union U<T> extends string { a: T } alias X = U<int32>;"),
            vec!["Type 'int32' is not assignable to type 'string'"]
        );
        assert_eq!(
            unassignable("union U<T> extends T { a: string } alias X = U<int32>;"),
            vec!["Type 'string' is not assignable to type 'int32'"]
        );
        assert!(unassignable(
            "union U<T> extends string { a: T } model M<X> { u: U<X> } alias X = U<\"a\">;"
        )
        .is_empty());
    }

    struct TrackRunner(Vec<String>);

    impl DecoratorRunner for TrackRunner {
        fn is_enabled(&self) -> bool {
            true
        }

        fn call_decorator(
            &mut self,
            context: &mut dyn DecoratorContext,
            target: TypeId,
            application: &DecoratorApplication,
        ) {
            if application.name != "@track" {
                return;
            }
            let arg = match application.args.first() {
                Some(DecoratorArgument::TypeRef(arg)) => context.get_type_kind(*arg),
                _ => "-".to_string(),
            };
            self.0
                .push(format!("{} <- {arg}", context.get_type_kind(target)));
        }
    }

    fn track_calls(source: &str) -> Vec<String> {
        let mut runner = TrackRunner(Vec::new());
        let source = format!("extern dec track(target: unknown, arg?: unknown);\n{source}");
        check_source_with_runner(&source, &mut runner);
        runner.0.sort();
        runner.0
    }

    #[test]
    fn skips_decorators_in_template_declarations_and_resolves_instance_arguments() {
        assert!(track_calls("@track(T) model Foo<T> {} model Bar<U> { foo: Foo<U> }").is_empty());
        assert!(track_calls("alias Foo<T> = { @track(T) prop: string; };").is_empty());
        assert_eq!(
            track_calls(
                "model Foo<T> { @track(T) prop: string } model Bar<U> { ...Foo<U> } enum A { a } model X { bar: Bar<A> }"
            ),
            vec!["ModelProperty <- Enum"]
        );
        // Upstream 16caf10f0: template parameter defaults are checked inside the declaration.
        assert_eq!(
            track_calls(
                "@track(R) model Tags<R extends {}> {} op patch<R extends {}, P extends {} = Tags<R>>(body: P): void; model Foo {} op fooPatch is patch<Foo>;"
            ),
            vec!["Model <- Model"]
        );
        // Upstream 9c9055062: concrete declarations first reached from a template are decorated.
        assert_eq!(
            track_calls("model Box<T> { item: Target.member; value: T } @track enum Target { member } model S is Box<string>;"),
            vec!["Enum <- -"]
        );
    }

    #[test]
    fn interpolated_function_call_in_template_declaration_does_not_panic() {
        // Upstream ad69727db: the Rust port does not support `extern fn`, function calls or string
        // template values yet (M10-M12); checking must still complete without panicking.
        let mut runner = RecordingRunner;
        let result = check_source_with_runner(
            "extern fn fn(type: unknown): valueof string;\n@doc(\"${fn(T)}\") model Crud<T extends Reflection.Model> {}\nmodel Foo {}\nalias X = Crud<Foo>;",
            &mut runner,
        );
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn instantiates_enum_declaration_expression_per_template_instance() {
        let result = check_source(
            "model M<T> { x: enum E { a, ...T } } enum A { x } enum B { y } model X is M<A>; model Y is M<B>;",
        );
        let enum_of = |name: &str| {
            let model = find_named_model(&result, name);
            let Type::Model(model) = result.type_arena.get(model) else {
                panic!("expected model");
            };
            let Type::ModelProperty(prop) = result.type_arena.get(model.properties["x"]) else {
                panic!("expected property");
            };
            prop.property_type
        };
        let (x, y) = (enum_of("X"), enum_of("Y"));
        assert_ne!(x, y);
        for (enum_id, expected) in [(x, ["a", "x"]), (y, ["a", "y"])] {
            let Type::Enum(enum_type) = result.type_arena.get(enum_id) else {
                panic!("expected enum");
            };
            assert_eq!(enum_type.members.keys().collect::<Vec<_>>(), expected);
            for member in enum_type.members.values() {
                let Type::EnumMember(member) = result.type_arena.get(*member) else {
                    panic!("expected enum member");
                };
                assert_eq!(member.r#enum, enum_id);
            }
        }
    }

    #[test]
    fn decorator_state_side_table_round_trips() {
        let mut runner = RecordingRunner;
        let result = check_source_with_runner(r#"@doc("hello") model Foo {}"#, &mut runner);
        let foo = find_named_model(&result, "Foo");
        assert_eq!(
            result.decorator_state[&foo]["@doc:doc"],
            DecoratorValue::String("hello".to_string())
        );
    }

    #[test]
    fn doc_comment_synthesizes_doc_decorator() {
        let mut runner = RecordingRunner;
        let result = check_source_with_runner("/** hello */\nmodel Foo {}", &mut runner);
        let foo = find_named_model(&result, "Foo");
        assert_eq!(
            result.decorator_state[&foo]["@doc:doc"],
            DecoratorValue::String("hello".to_string())
        );
    }

    #[test]
    fn decorator_runner_can_report_diagnostics() {
        let mut runner = RecordingRunner;
        let result = check_source_with_runner("@key model Foo {}", &mut runner);
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "decorator-wrong-target"));
    }

    #[test]
    fn array_of_template_parameter_uses_the_parameter() {
        let result = check_source("model Page<T> { items: T[]; }");
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.code == "@typespec/compiler/unused-template-parameter"),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn unused_template_parameters_are_reported_in_source_order() {
        let result = check_source("model A<T> {} model B<U> {} model C<V, W> {}");
        let positions = result
            .diagnostics
            .iter()
            .filter(|d| d.code == "@typespec/compiler/unused-template-parameter")
            .map(|d| d.target.pos)
            .collect::<Vec<_>>();
        assert_eq!(positions.len(), 4);
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    fn check_source_with_options(source: &str, options: CheckOptions) -> CheckResult {
        let source = SourceFile::new(source.to_string());
        let parse_result = parse(&source);
        let mut table = bind(&parse_result);
        let resolution = resolve_names(&parse_result, &mut table);
        check_with_options(&parse_result, &table, &resolution.resolutions, options)
    }

    #[test]
    fn project_features_disable_feature_diagnostics() {
        let source = "union A { a: string } union B extends A { b: int32 } model M { p: model { x: string } }";
        let codes = |options| {
            check_source_with_options(source, options)
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        };
        let disabled = codes(CheckOptions::default());
        assert!(disabled.contains(&"union-extends-disabled"), "{disabled:?}");
        assert!(
            disabled.contains(&"declaration-expression-disabled"),
            "{disabled:?}"
        );
        let enabled = codes(CheckOptions {
            project_features: vec!["union-extends".into(), "declaration-expressions".into()],
            ..CheckOptions::default()
        });
        assert!(!enabled.contains(&"union-extends-disabled"), "{enabled:?}");
        assert!(
            !enabled.contains(&"declaration-expression-disabled"),
            "{enabled:?}"
        );
    }
}
