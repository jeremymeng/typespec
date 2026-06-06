use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use typespec_parser::ast::{NamespaceBody, NodeData, NodeId};
use typespec_parser::syntax_kind::SyntaxKind;
use typespec_parser::ParseResult;
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use crate::intrinsics::{register_intrinsics, Intrinsics};
use crate::name_resolver::ResolutionResult;
use crate::types::{
    AliasType, BooleanLiteralType, DecoratorRef, EnumMemberType, EnumMemberValue, EnumType,
    InterfaceType, ModelPropertyType, ModelType, NamespaceType, NumericLiteralType, OperationType,
    ScalarType, StringLiteralType, SymbolFlags, SymbolId, SymbolTable, Type, TypeArena, TypeId,
    UnionType, UnionVariantType,
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
    Numeric(u64),
    Boolean(bool),
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub type_arena: TypeArena,
    pub symbol_to_type: HashMap<SymbolId, TypeId>,
    pub node_to_type: HashMap<NodeId, TypeId>,
    pub diagnostics: Vec<Diagnostic>,
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
        }
    }

    fn finish(mut self) -> CheckResult {
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
            NodeData::NumericLiteral { value, .. } => {
                self.numeric_literal(Some(node), value);
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
        let NodeData::AliasStatement { id, value, .. } = self.node(node_id).data.clone() else {
            panic!("expected alias statement");
        };
        let symbol = self
            .symbol_for_node(node_id)
            .expect("alias statement should have symbol");
        let namespace = self.namespace_for_symbol(Some(symbol));
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        self.symbol_to_type.insert(symbol, type_id);

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
        let default = default.map(|node| self.type_for_default_node(node));
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
                decorators,
            }),
        );
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
            NodeData::StringLiteral { value } => self.string_literal(Some(node_id), value),
            NodeData::NumericLiteral { value, .. } => self.numeric_literal(Some(node_id), value),
            NodeData::BooleanLiteral { value } => self.boolean_literal(Some(node_id), value),
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
        let resolution = self
            .resolutions
            .get(&node_id)
            .or_else(|| {
                self.reference_target(node_id)
                    .and_then(|target| self.resolutions.get(&target))
            })
            .cloned();
        let Some(ResolutionResult::Resolved(symbol)) = resolution else {
            self.cache_type(node_id, self.intrinsics.unknown);
            return self.intrinsics.unknown;
        };
        let type_id = self.type_for_symbol(symbol);
        self.cache_type(node_id, type_id);
        type_id
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

    fn check_array_expression(&mut self, node_id: NodeId, element_type: NodeId) -> TypeId {
        self.alias_structural_depth += 1;
        let element = self
            .resolved_symbol(node_id)
            .map(|symbol| self.type_for_symbol(symbol))
            .unwrap_or_else(|| self.type_for_node(element_type));
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
        };
        self.add_and_cache(node_id, Type::Model(model))
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
        let Some(source) = self.model_type(source_model).cloned() else {
            return;
        };
        for (name, prop_id) in source.properties {
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
                decorators: prop.decorators,
            }));
            model.properties.insert(name, cloned);
        }
    }

    fn check_spread_property(&mut self, target: NodeId, model_id: TypeId, model: &mut ModelType) {
        let source = self.type_for_node(target);
        let source = self.get_non_alias_type(source);
        let Some(source_model) = self.model_type(source).cloned() else {
            self.report(
                "spread-model",
                "Cannot spread properties of non-model type.",
                target,
            );
            return;
        };
        for (name, prop_id) in source_model.properties {
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
                    name: self.decorator_name(target),
                    args: arguments,
                }),
                _ => None,
            })
            .collect()
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
        match self.resolutions.get(&node_id).or_else(|| {
            self.reference_target(node_id)
                .and_then(|target| self.resolutions.get(&target))
        }) {
            Some(ResolutionResult::Resolved(symbol)) => Some(*symbol),
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

    fn numeric_literal(&mut self, node_id: Option<NodeId>, value: f64) -> TypeId {
        let key = LiteralKey::Numeric(value.to_bits());
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
        assert!(matches!(code.value, Some(EnumMemberValue::Numeric(42.0))));
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
}
