//! M12 value type-checking: object (`#{}`) and array (`#[]`) values get the
//! precise synthetic types TS builds (`createTypeForObjectValue` /
//! `createTypeForArrayValue`) and are checked against their target with the
//! diagnostics TS reports after flattening relation errors
//! (`convertErrorsToDiagnostics`).

use indexmap::IndexMap;
use typespec_parser::ast::{NodeData, NodeId};
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use super::Checker;
use crate::type_relation::{type_name, TypeRelationChecker};
use crate::types::{ModelPropertyType, ModelType, SymbolFlags, TupleType, Type, TypeId};

impl Checker<'_> {
    /// Whether the node is an expression TS only evaluates as a value.
    pub(super) fn is_value_node(&self, node: NodeId) -> bool {
        match &self.node(node).data {
            NodeData::ObjectLiteral { .. }
            | NodeData::ArrayLiteral { .. }
            | NodeData::CallExpression { .. } => true,
            _ => self.const_declaration(node).is_some(),
        }
    }

    fn const_declaration(&self, node: NodeId) -> Option<NodeId> {
        if !matches!(
            self.node(node).data,
            NodeData::TypeReference { .. }
                | NodeData::Identifier { .. }
                | NodeData::MemberExpression { .. }
        ) {
            return None;
        }
        let symbol = &self.symbol_table.symbols[self.resolved_symbol(node)?.index()];
        if !symbol.flags.contains(SymbolFlags::Const) {
            return None;
        }
        symbol.declarations.first().copied()
    }

    /// Type of the value written at `node`; `None` when it can't be determined
    /// (the value is then not checked).
    pub(super) fn value_type(&mut self, node: NodeId) -> Option<TypeId> {
        if let Some(cached) = self.value_types.get(&node).copied() {
            return Some(cached);
        }
        let type_id = match self.node(node).data.clone() {
            NodeData::ObjectLiteral { properties, .. } => {
                self.object_value_type(node, properties)?
            }
            NodeData::ArrayLiteral { values } => {
                let mut items = Vec::new();
                for value in values {
                    items.push(self.value_type(value)?);
                }
                self.type_arena.add(Type::Tuple(TupleType {
                    node: Some(node),
                    values: items,
                }))
            }
            NodeData::CallExpression { target, .. } => {
                let scalar_node = match self.node(target).data.clone() {
                    NodeData::MemberExpression { base, .. } => base,
                    _ => target,
                };
                let scalar = self.type_for_node(scalar_node);
                if !self.is_scalar(scalar) {
                    return None;
                }
                scalar
            }
            _ => match self.const_declaration(node) {
                Some(declaration) => {
                    let NodeData::ConstStatement {
                        value, type_ref, ..
                    } = self.node(declaration).data.clone()
                    else {
                        return None;
                    };
                    match type_ref {
                        Some(type_ref) => self.type_for_node(type_ref),
                        None => self.value_type(value)?,
                    }
                }
                None => {
                    let type_id = self.type_for_node(node);
                    if matches!(self.type_arena.get(type_id), Type::Error) {
                        return None;
                    }
                    type_id
                }
            },
        };
        self.value_types.insert(node, type_id);
        Some(type_id)
    }

    fn is_scalar(&self, type_id: TypeId) -> bool {
        let type_id = self.get_non_alias_type(type_id);
        matches!(self.type_arena.get(type_id), Type::Scalar(_))
            || self.intrinsic_declarations.contains_key(&type_id)
    }

    /// TS `createTypeForObjectValue`.
    fn object_value_type(&mut self, node: NodeId, properties: Vec<NodeId>) -> Option<TypeId> {
        let model_id = self.type_arena.reserve();
        let mut members = IndexMap::new();
        for property in properties {
            match self.node(property).data.clone() {
                NodeData::ObjectLiteralProperty { id, value } => {
                    let property_type = self.value_type(value)?;
                    let name = self.identifier_name(id);
                    let prop_id = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                        name: name.clone(),
                        node: Some(property),
                        symbol: None,
                        model: model_id,
                        property_type,
                        optional: false,
                        default: None,
                        source_property: None,
                        decorators: Vec::new(),
                    }));
                    self.object_value_properties.insert(prop_id);
                    members.insert(name, prop_id);
                }
                NodeData::ObjectLiteralSpreadProperty { target } => {
                    let spread = self.value_type(target)?;
                    let Type::Model(spread) = self.type_arena.get(spread).clone() else {
                        return None;
                    };
                    for (name, prop_id) in spread.properties {
                        let Type::ModelProperty(prop) = self.type_arena.get(prop_id).clone() else {
                            continue;
                        };
                        let copy = self.type_arena.add(Type::ModelProperty(ModelPropertyType {
                            model: model_id,
                            ..prop
                        }));
                        members.insert(name, copy);
                    }
                }
                _ => {}
            }
        }
        self.type_arena.replace(
            model_id,
            Type::Model(ModelType {
                name: String::new(),
                node: Some(node),
                symbol: None,
                properties: members,
                base_model: None,
                source_model: None,
                indexer: None,
                decorators: Vec::new(),
                namespace: None,
                derived_models: Vec::new(),
                expression: true,
                template_node: None,
                template_arguments: Vec::new(),
                template_parameters: Vec::new(),
                is_template_declaration: false,
            }),
        );
        Some(model_id)
    }

    pub(super) fn is_value_assignable(&self, source: TypeId, target: TypeId) -> bool {
        TypeRelationChecker::new(&self.type_arena)
            .with_object_value_properties(&self.object_value_properties)
            .is_type_assignable_to_span(source, target, Span { pos: 0, end: 0 })
            .is_related()
    }

    /// TS `relation.isValueOfType(value, target, node)` with its flattened diagnostics.
    pub(super) fn check_value_of_type(&mut self, node: NodeId, target: TypeId) -> Vec<Diagnostic> {
        let Some(source) = self.value_type(node) else {
            return Vec::new();
        };
        let mut diagnostics = Vec::new();
        self.check_value_structure(node, source, target, node, &mut diagnostics);
        diagnostics
    }

    /// Walks object / array literals like TS flattens relation errors: errors
    /// on literal members land on the innermost literal property, everything
    /// else on `report_at` with the combined relation message.
    fn check_value_structure(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        report_at: NodeId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if self.is_value_assignable(source, target) {
            return;
        }
        let target = self.get_non_alias_type(target);
        match (
            self.node(node).data.clone(),
            self.type_arena.get(target).clone(),
        ) {
            (NodeData::ObjectLiteral { properties, .. }, Type::Model(target_model))
                if !self.is_array_model(&target_model) && !self.is_reflection_model(target) =>
            {
                let Type::Model(source_model) = self.type_arena.get(source).clone() else {
                    return;
                };
                let literal_members = properties
                    .iter()
                    .filter_map(|property| match self.node(*property).data.clone() {
                        NodeData::ObjectLiteralProperty { id, value } => {
                            Some((self.identifier_name(id), (*property, value)))
                        }
                        _ => None,
                    })
                    .collect::<IndexMap<_, _>>();
                let mut remaining = source_model.properties.clone();
                for (name, target_prop) in self.properties_inherited(target) {
                    let Type::ModelProperty(target_prop) = self.type_arena.get(target_prop).clone()
                    else {
                        continue;
                    };
                    let Some(source_prop) = remaining.shift_remove(&name) else {
                        if !target_prop.optional {
                            diagnostics.push(Diagnostic {
                                severity: DiagnosticSeverity::Error,
                                code: "missing-property",
                                message: format!(
                                    "Property '{}' is missing on type '{}' but required in '{}'",
                                    name,
                                    type_name(&self.type_arena, source),
                                    type_name(&self.type_arena, target)
                                ),
                                target: self.node_span(node),
                            });
                        }
                        continue;
                    };
                    let Type::ModelProperty(source_prop) = self.type_arena.get(source_prop).clone()
                    else {
                        continue;
                    };
                    match literal_members.get(&name).copied() {
                        Some((property_node, value_node)) => self.check_value_structure(
                            value_node,
                            source_prop.property_type,
                            target_prop.property_type,
                            property_node,
                            diagnostics,
                        ),
                        None => self.report_value_leaf(
                            source_prop.property_type,
                            target_prop.property_type,
                            source_prop.node.unwrap_or(node),
                            diagnostics,
                        ),
                    }
                }
                if let Some(indexer) = target_model.indexer {
                    // TS `arePropertiesAssignableToIndexer`: the first failing property.
                    for prop_id in remaining.values() {
                        let Type::ModelProperty(prop) = self.type_arena.get(*prop_id).clone()
                        else {
                            continue;
                        };
                        if !self.is_value_assignable(prop.property_type, indexer.value) {
                            self.report_value_leaf(
                                prop.property_type,
                                indexer.value,
                                node,
                                diagnostics,
                            );
                            break;
                        }
                    }
                } else {
                    for (name, prop_id) in &remaining {
                        if !self.object_value_properties.contains(prop_id) {
                            continue;
                        }
                        let Some((property_node, _)) = literal_members.get(name).copied() else {
                            continue;
                        };
                        diagnostics.push(Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            code: "unexpected-property",
                            message: format!(
                                "Object value may only specify known properties, and '{}' does not exist in type '{}'.",
                                name,
                                type_name(&self.type_arena, target)
                            ),
                            target: self.node_span(property_node),
                        });
                    }
                }
            }
            (NodeData::ArrayLiteral { values }, Type::Model(target_model))
                if self.is_array_model(&target_model) =>
            {
                let item_target = target_model.indexer.unwrap().value;
                self.check_first_failing_item(&values, |_| item_target, report_at, diagnostics);
            }
            (NodeData::ArrayLiteral { values }, Type::Tuple(target_tuple))
                if values.len() == target_tuple.values.len() =>
            {
                let targets = target_tuple.values.clone();
                self.check_first_failing_item(
                    &values,
                    |index| targets[index],
                    report_at,
                    diagnostics,
                );
            }
            _ => self.report_value_leaf(source, target, report_at, diagnostics),
        }
    }

    fn check_first_failing_item(
        &mut self,
        values: &[NodeId],
        target_of: impl Fn(usize) -> TypeId,
        report_at: NodeId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        for (index, value) in values.iter().enumerate() {
            let Some(item) = self.value_type(*value) else {
                continue;
            };
            let item_target = target_of(index);
            if !self.is_value_assignable(item, item_target) {
                self.check_value_structure(*value, item, item_target, report_at, diagnostics);
                return;
            }
        }
    }

    /// A relation error retargeted to `report_at`: TS combines the error with
    /// its first nested error chain into one diagnostic.
    fn report_value_leaf(
        &mut self,
        source: TypeId,
        target: TypeId,
        report_at: NodeId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let span = self.node_span(report_at);
        let errors = TypeRelationChecker::new(&self.type_arena)
            .with_object_value_properties(&self.object_value_properties)
            .check_type_assignable_span(source, target, span);
        let Some(first) = errors.first() else {
            return;
        };
        let unassignable = format!(
            "Type '{}' is not assignable to type '{}'",
            type_name(&self.type_arena, source),
            type_name(&self.type_arena, target)
        );
        let message = if errors.len() == 1 && first.code == "unassignable" {
            first.message.clone()
        } else {
            let nested = first
                .message
                .split('\n')
                .map(|line| format!("\n  {line}"))
                .collect::<String>();
            format!("{unassignable}{nested}")
        };
        diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "unassignable",
            message,
            target: span,
        });
    }

    fn is_array_model(&self, model: &ModelType) -> bool {
        model.indexer.is_some_and(|indexer| {
            matches!(
                self.type_arena.get(self.get_non_alias_type(indexer.key)),
                Type::Integer
            )
        })
    }

    fn is_reflection_model(&self, type_id: TypeId) -> bool {
        let Type::Model(model) = self.type_arena.get(type_id) else {
            return false;
        };
        model.namespace.is_some_and(|namespace| {
            matches!(self.type_arena.get(namespace), Type::Namespace(ns) if ns.name == "Reflection")
        })
    }

    fn properties_inherited(&self, model: TypeId) -> Vec<(String, TypeId)> {
        let mut result = Vec::new();
        let mut current = Some(model);
        let mut seen = std::collections::HashSet::new();
        while let Some(model_id) = current.filter(|id| seen.insert(*id)) {
            let Type::Model(model) = self.type_arena.get(model_id) else {
                break;
            };
            for (name, prop) in &model.properties {
                result.push((name.clone(), *prop));
            }
            current = model.base_model;
        }
        result
    }
}
