use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use typespec_parser::ast::{NamespaceBody, NodeData, NodeId};
use typespec_parser::ParseResult;
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use crate::intrinsics::{register_intrinsics, Intrinsics};
use crate::name_resolver::ResolutionResult;
use crate::types::{
    DecoratorRef, ModelPropertyType, ModelType, ScalarType, SymbolId, SymbolTable, Type, TypeArena,
    TypeId,
};

pub struct Checker<'p> {
    parse_result: &'p ParseResult,
    symbol_table: &'p SymbolTable,
    resolutions: &'p HashMap<NodeId, ResolutionResult>,
    type_arena: TypeArena,
    intrinsics: Intrinsics,
    diagnostics: Vec<Diagnostic>,
    type_cache: HashMap<NodeId, TypeId>,
    symbol_to_type: HashMap<SymbolId, TypeId>,
    node_to_type: HashMap<NodeId, TypeId>,
    base_stack: Vec<SymbolId>,
    reported_base_cycles: HashSet<SymbolId>,
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
        Self {
            parse_result,
            symbol_table,
            resolutions,
            type_arena,
            intrinsics,
            diagnostics: Vec::new(),
            type_cache: HashMap::new(),
            symbol_to_type,
            node_to_type: HashMap::new(),
            base_stack: Vec::new(),
            reported_base_cycles: HashSet::new(),
        }
    }

    fn finish(mut self) -> CheckResult {
        self.diagnostics
            .extend(self.parse_result.diagnostics.iter().cloned());
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
                None
            }
            NodeData::ModelStatement { .. } => Some(self.check_model_statement(node_id)),
            NodeData::ScalarStatement { .. } => Some(self.check_scalar(node_id)),
            NodeData::ModelExpression { .. } => Some(self.check_model_expression(node_id)),
            NodeData::OperationStatement { signature, .. } => {
                self.check_node(signature);
                None
            }
            NodeData::OperationSignatureDeclaration {
                parameters,
                return_type,
            } => {
                self.type_for_node(parameters);
                self.type_for_node(return_type);
                None
            }
            NodeData::InterfaceStatement { operations, .. } => {
                for operation in operations {
                    self.check_node(operation);
                }
                None
            }
            _ => None,
        }
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
            namespace: None,
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
        self.type_arena.replace(type_id, Type::Model(model));
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
            namespace: None,
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
        let type_id = self.type_arena.reserve();
        self.cache_type(node_id, type_id);
        if let Some(symbol) = symbol {
            self.symbol_to_type.insert(symbol, type_id);
        }

        let base_scalar = extends.and_then(|extends| self.check_scalar_extends(symbol, extends));
        let decorators = self.collect_decorators(&decorators);
        let name = self.identifier_name(id);
        self.type_arena.replace(
            type_id,
            Type::Scalar(ScalarType {
                name,
                node: Some(node_id),
                symbol,
                base_scalar,
                decorators,
                namespace: None,
            }),
        );
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

        let property_type = self.type_for_node(value);
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
            NodeData::StringLiteral { value } => {
                self.add_and_cache(node_id, Type::StringLiteral(value))
            }
            NodeData::NumericLiteral {
                value_as_string, ..
            } => self.add_and_cache(node_id, Type::NumericLiteral(value_as_string)),
            NodeData::BooleanLiteral { value } => {
                self.add_and_cache(node_id, Type::BooleanLiteral(value))
            }
            NodeData::ModelStatement { .. } => self.check_model_statement(node_id),
            NodeData::ScalarStatement { .. } => self.check_scalar(node_id),
            _ => self.intrinsics.unknown,
        }
    }

    fn type_for_default_node(&mut self, node_id: NodeId) -> TypeId {
        match self.node(node_id).data {
            NodeData::Keyword => self.add_and_cache(node_id, Type::Null),
            _ => self.type_for_node(node_id),
        }
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
            NodeData::ModelProperty { .. } => self
                .type_cache
                .get(&node)
                .copied()
                .unwrap_or(self.intrinsics.unknown),
            _ => self.intrinsics.unknown,
        }
    }

    fn check_array_expression(&mut self, node_id: NodeId, element_type: NodeId) -> TypeId {
        let element = self
            .resolved_symbol(node_id)
            .map(|symbol| self.type_for_symbol(symbol))
            .unwrap_or_else(|| self.type_for_node(element_type));
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

    fn add_and_cache(&mut self, node_id: NodeId, ty: Type) -> TypeId {
        let type_id = self.type_arena.add(ty);
        self.cache_type(node_id, type_id);
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
}
