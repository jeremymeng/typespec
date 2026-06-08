use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use typespec_parser::ast::{NamespaceBody, NodeData, NodeId};
use typespec_parser::syntax_kind::SyntaxKind;
use typespec_parser::ParseResult;
use typespec_source::{Diagnostic, DiagnosticSeverity, Numeric, Span};

use crate::decorator::{
    decorator_state_key, DecoratorApplication, DecoratorArgument, DecoratorContext,
    DecoratorRunner, DecoratorState, DecoratorValue,
};
use crate::instantiation::InstantiationMap;
use crate::intrinsics::{register_intrinsics, Intrinsics};
use crate::name_resolver::ResolutionResult;
use crate::type_relation::{type_name, TypeRelationChecker};
use crate::types::{
    AliasType, BooleanLiteralType, DecoratorRef, EnumMemberType, EnumMemberValue, EnumType,
    InterfaceType, ModelPropertyType, ModelType, NamespaceType, NumericLiteralType, OperationType,
    ScalarType, StringLiteralType, SymbolFlags, SymbolId, SymbolTable, TemplateParameterType, Type,
    TypeArena, TypeId, UnionType, UnionVariantType,
};

pub struct Checker<'p> {
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
    decorator_state: DecoratorState,
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
}

pub fn check(
    parse_result: &ParseResult,
    symbol_table: &SymbolTable,
    resolutions: &HashMap<NodeId, ResolutionResult>,
) -> CheckResult {
    let mut checker = Checker::new(parse_result, symbol_table, resolutions);
    checker.check_script(parse_result.root);
    checker.finish()
}

pub fn check_with_decorator_runner(
    parse_result: &ParseResult,
    symbol_table: &SymbolTable,
    resolutions: &HashMap<NodeId, ResolutionResult>,
    runner: &mut dyn DecoratorRunner,
) -> CheckResult {
    let mut checker = Checker::new(parse_result, symbol_table, resolutions);
    checker.check_script(parse_result.root);
    if runner.is_enabled() {
        checker.apply_decorators(runner);
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
            template_parameter_usage: HashMap::new(),
            decorator_state: HashMap::new(),
        }
    }

    fn finish(mut self) -> CheckResult {
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
        CheckResult {
            type_arena: self.type_arena,
            symbol_to_type: self.symbol_to_type,
            node_to_type: self.node_to_type,
            diagnostics: self.diagnostics,
            decorator_state: self.decorator_state,
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

        let NodeData::ModelStatement {
            id,
            decorators,
            template_parameters,
            extends,
            is,
            properties,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected model statement");
        };

        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }
        let template_parameters = self.check_template_parameters(&template_parameters);

        let source_model = self.check_model_is(symbol, is);
        let mut model = ModelType {
            name: self.identifier_name(id),
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
        for property in properties {
            if let NodeData::ModelProperty { id, .. } = self.node(property).data {
                let prop_id = self.check_model_property(property, type_id);
                let name = self.identifier_name(id);
                if model.properties.contains_key(&name) {
                    self.report_duplicate_property(&model.name, &name, property);
                } else {
                    model.properties.insert(name, prop_id);
                }
            } else if let NodeData::ModelSpreadProperty { target } = self.node(property).data {
                self.check_spread_property(target, type_id, &mut model);
            }
        }

        let base_model = model.base_model;
        let model_name = model.name.clone();
        self.type_arena.replace(type_id, Type::Model(model));
        if let Some(namespace) = namespace {
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
        for property in properties {
            if let NodeData::ModelProperty { id, .. } = self.node(property).data {
                let prop_id = self.check_model_property(property, type_id);
                let name = self.identifier_name(id);
                if model.properties.contains_key(&name) {
                    self.report_duplicate_property("", &name, property);
                } else {
                    model.properties.insert(name, prop_id);
                }
            } else if let NodeData::ModelSpreadProperty { target } = self.node(property).data {
                self.check_spread_property(target, type_id, &mut model);
            }
        }
        self.type_arena.replace(type_id, Type::Model(model));
        type_id
    }

    fn check_scalar(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }

        let NodeData::ScalarStatement {
            id,
            decorators,
            template_parameters,
            extends,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected scalar statement");
        };
        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let template_parameters = self.check_template_parameters(&template_parameters);
        let base_scalar = extends.and_then(|extends| self.check_scalar_extends(symbol, extends));
        let decorators = self.collect_decorators(&decorators);
        let name = self.identifier_name(id);
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
                NamespaceMemberKind::Scalar,
            );
        }
        type_id
    }

    fn check_enum(&mut self, node_id: NodeId) -> TypeId {
        if let Some(cached) = self.type_cache.get(&node_id).copied() {
            return cached;
        }

        let NodeData::EnumStatement {
            id,
            decorators,
            members,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected enum statement");
        };
        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let mut enum_type = EnumType {
            name: self.identifier_name(id),
            node: Some(node_id),
            symbol,
            members: IndexMap::new(),
            decorators: self.collect_decorators(&decorators),
            namespace,
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
        if let Some(namespace) = namespace {
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

        let NodeData::UnionStatement {
            id,
            decorators,
            template_parameters,
            options,
            ..
        } = self.node(node_id).data.clone()
        else {
            panic!("expected union statement");
        };
        let symbol = self.symbol_for_node(node_id);
        let namespace = self.namespace_for_symbol(symbol);
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let mut union_type = UnionType {
            name: self.identifier_name(id),
            node: Some(node_id),
            symbol,
            variants: IndexMap::new(),
            expression: false,
            decorators: self.collect_decorators(&decorators),
            namespace,
            template_node: None,
            template_arguments: Vec::new(),
            template_parameters: self.check_template_parameters(&template_parameters),
            is_template_declaration: !template_parameters.is_empty(),
        };

        for (index, variant) in options.into_iter().enumerate() {
            let variant_id = self.check_union_variant(variant, type_id, index);
            let Some(variant_type) = self.union_variant_type(variant_id) else {
                continue;
            };
            let name = variant_type.name.clone();
            if union_type.variants.contains_key(&name) {
                self.report(
                    "union-duplicate",
                    &format!("Union already has a variant named {name}"),
                    variant,
                );
            } else {
                union_type.variants.insert(name, variant_id);
            }
        }

        let union_name = union_type.name.clone();
        self.type_arena.replace(type_id, Type::Union(union_type));
        if let Some(namespace) = namespace {
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
                Type::Union(nested) if nested.expression => Some(nested),
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
                self.report(
                    "intersect-invalid-index",
                    "Cannot intersect an array model.",
                    option_node,
                );
                indexers.push(indexer);
            }
            model_options.push(option_type);
        }

        for option_type in model_options {
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
        if !optional {
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
            Some(ResolutionResult::Resolved(symbol)) => Some(ResolutionResult::Resolved(symbol)),
            _ => self
                .reference_target(node_id)
                .and_then(|target| self.resolutions.get(&target))
                .cloned(),
        };
        let symbol = match resolution {
            Some(ResolutionResult::Resolved(symbol)) => symbol,
            _ => {
                let Some(symbol) = self.fallback_reference_symbol(node_id) else {
                    self.cache_type(node_id, self.intrinsics.unknown);
                    return self.intrinsics.unknown;
                };
                symbol
            }
        };
        let type_id = self.type_for_symbol(symbol);
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
        if let Some(type_id) = self
            .intrinsics
            .by_name(&self.symbol_table.symbols[symbol.index()].name)
        {
            self.symbol_to_type.insert(symbol, type_id);
            return type_id;
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
        let element_has_template_args = matches!(
            self.node(element_type).data.clone(),
            NodeData::TypeReference { arguments, .. } if !arguments.is_empty()
        );
        let element = if element_has_template_args {
            self.type_for_node(element_type)
        } else {
            self.resolved_symbol(node_id)
                .map(|symbol| self.type_for_symbol(symbol))
                .unwrap_or_else(|| self.type_for_node(element_type))
        };
        self.alias_structural_depth -= 1;
        let model = ModelType {
            name: String::new(),
            node: Some(node_id),
            symbol: self.symbol_for_node(node_id),
            properties: IndexMap::new(),
            base_model: None,
            source_model: None,
            indexer: Some(element),
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
        let default = default.map(|default| self.type_for_node(default));
        if let (Some(default), Some(constraint)) = (default, constraint) {
            let diagnostics = {
                let relation = TypeRelationChecker::new(&self.type_arena);
                relation.check_type_assignable_span(default, constraint, self.node_span(node_id))
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

    fn report_unused_template_parameters(&mut self) {
        let unused = self
            .template_parameter_usage
            .iter()
            .filter_map(|(node, used)| (!*used).then_some(*node))
            .collect::<Vec<_>>();
        for node in unused {
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
                code: "unused-template-parameter",
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
            let arg_type = self.type_for_node(argument);
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
                        positional[param_index] = Some(arg_type);
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
                positional[index] = Some(arg_type);
            }
        }

        let mut result = Vec::with_capacity(params.len());
        let mut subst = HashMap::new();
        for (index, param_id) in params.iter().enumerate() {
            let arg = if let Some(arg) = positional[index] {
                arg
            } else {
                let default = match self.type_arena.get(*param_id) {
                    Type::TemplateParameter(param) => param.default,
                    _ => None,
                };
                if let Some(default) = default {
                    self.substitute_type(default, &subst)
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
                    match self.type_arena.get(*param_id) {
                        Type::TemplateParameter(param) => {
                            param.constraint.unwrap_or(self.intrinsics.unknown)
                        }
                        _ => self.intrinsics.unknown,
                    }
                }
            };
            let constraint = match self.type_arena.get(*param_id) {
                Type::TemplateParameter(param) => param.constraint,
                _ => None,
            };
            let arg = if let Some(constraint) = constraint {
                let constraint = self.substitute_type(constraint, &subst);
                let diagnostics = {
                    let relation = TypeRelationChecker::new(&self.type_arena);
                    relation.check_type_assignable_span(arg, constraint, self.node_span(node_id))
                };
                if diagnostics.is_empty() {
                    arg
                } else {
                    let param_name = match self.type_arena.get(*param_id) {
                        Type::TemplateParameter(param) => param.name.clone(),
                        _ => String::new(),
                    };
                    self.diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: "unassignable",
                        message: format!(
                            "Type '{}' is not assignable to template parameter '{}' which extends '{}'.",
                            type_name(&self.type_arena, arg),
                            param_name,
                            type_name(&self.type_arena, constraint)
                        ),
                        target: self.node_span(node_id),
                    });
                    constraint
                }
            } else {
                arg
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
            indexer: source
                .indexer
                .map(|indexer| self.substitute_type(indexer, subst)),
            decorators: source.decorators,
            namespace: source.namespace,
            derived_models: Vec::new(),
            template_node: source.node.or(source.template_node),
            template_arguments: template_args,
            template_parameters: Vec::new(),
            is_template_declaration: false,
        };
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
            model.properties.insert(name, cloned);
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

    fn instantiate_union(
        &mut self,
        union_id: TypeId,
        source: UnionType,
        template_args: Vec<TypeId>,
        subst: &HashMap<TypeId, TypeId>,
    ) -> UnionType {
        let mut union = UnionType {
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
            let cloned = self.type_arena.add(Type::UnionVariant(UnionVariantType {
                name: name.clone(),
                node: variant.node,
                union: union_id,
                variant_type,
                decorators: variant.decorators,
            }));
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
            interface.operations.insert(name, cloned);
        }
        interface
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
                let cloned = self.instantiate_union(union_id, union, Vec::new(), subst);
                self.type_arena.replace(union_id, Type::Union(cloned));
                union_id
            }
            _ => type_id,
        }
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
        if model.name.is_empty() {
            self.report(
                "is-model",
                "Model `is` cannot specify a model expression.",
                is_node,
            );
            return None;
        }
        Some(source)
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

    fn check_spread_property(&mut self, target: NodeId, model_id: TypeId, model: &mut ModelType) {
        let source = self.type_for_node(target);
        let source = self.get_non_alias_type(source);
        if !matches!(self.type_arena.get(source), Type::Model(_)) {
            self.report(
                "spread-model",
                "Cannot spread properties of non-model type.",
                target,
            );
            return;
        }
        for (name, prop_id) in self.walk_model_properties_inherited(source) {
            let Some(prop) = self.model_property_type(prop_id).cloned() else {
                continue;
            };
            if model.properties.contains_key(&name) {
                self.report_duplicate_property(&model.name, &name, target);
                continue;
            }
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
        let type_ids = (0..self.type_arena.types().len())
            .map(|id| TypeId::new(id as u32))
            .collect::<Vec<_>>();
        for type_id in type_ids {
            let mut applications = Vec::new();
            if let Some(doc) = self.doc_comment_for_type(type_id) {
                let node = self.type_node(type_id).unwrap_or(self.parse_result.root);
                applications.push(DecoratorApplication {
                    node,
                    name: "@doc".to_string(),
                    args: vec![DecoratorArgument::String(doc)],
                });
            }
            applications.extend(
                self.decorators_for_type(type_id)
                    .into_iter()
                    .map(|decorator| self.decorator_application(decorator)),
            );
            applications.extend(self.augment_decorator_applications_for_type(type_id));
            for application in applications {
                runner.call_decorator(self, type_id, &application);
            }
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
            _ => Vec::new(),
        }
    }

    fn decorator_application(&mut self, decorator: DecoratorRef) -> DecoratorApplication {
        let args = decorator
            .args
            .iter()
            .copied()
            .map(|arg| self.decorator_argument(arg))
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
            result.push(self.decorator_application(DecoratorRef {
                node: augment_node,
                name: Self::normalize_decorator_name(&self.decorator_name(target)),
                args: arguments,
            }));
        }
        result
    }

    fn decorator_argument(&mut self, node_id: NodeId) -> DecoratorArgument {
        match self.node(node_id).data.clone() {
            NodeData::StringLiteral { value } => DecoratorArgument::String(value),
            NodeData::NumericLiteral { value, .. } => DecoratorArgument::Number(value),
            NodeData::BooleanLiteral { value } => DecoratorArgument::Boolean(value),
            NodeData::Identifier { sv } if sv == "null" => DecoratorArgument::None,
            _ => {
                let type_id = self.type_for_node(node_id);
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
            _ => None,
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
        type_name(&self.type_arena, target)
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
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "unassignable" && d.message.contains("template parameter 'T'")));
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
}
