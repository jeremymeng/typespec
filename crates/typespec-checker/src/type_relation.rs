use std::collections::HashSet;
use std::mem::discriminant;

use typespec_parser::ast::NodeId;
use typespec_source::{Diagnostic, DiagnosticSeverity, Numeric, Span};

use crate::numeric_ranges::{is_in_range, numeric_scalar_info, range_subsumes};
use crate::types::{
    ModelIndexer, ModelPropertyType, ModelType, ScalarType, Type, TypeArena, TypeId, UnionType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationResult {
    Related,
    Error(Vec<Diagnostic>),
}

impl RelationResult {
    pub fn is_related(&self) -> bool {
        matches!(self, RelationResult::Related)
    }
}

pub struct TypeRelationChecker<'a> {
    arena: &'a TypeArena,
    /// Properties written in an object value literal (`#{ a: 1 }`): TS reports them as
    /// `unexpected-property` when the target model doesn't declare them.
    object_value_properties: Option<&'a HashSet<TypeId>>,
}

/// Relation pairs currently being checked. Revisiting a pair means the types are recursive; each
/// relation kind resolves the cycle instead of recursing forever.
struct RelationContext {
    visiting: HashSet<(TypeId, TypeId)>,
}

impl<'a> TypeRelationChecker<'a> {
    pub fn new(arena: &'a TypeArena) -> Self {
        Self {
            arena,
            object_value_properties: None,
        }
    }

    pub fn with_object_value_properties(mut self, properties: &'a HashSet<TypeId>) -> Self {
        self.object_value_properties = Some(properties);
        self
    }

    pub fn is_type_identical_to(&self, source: TypeId, target: TypeId) -> bool {
        let source = self.get_non_alias_type(source);
        let target = self.get_non_alias_type(target);
        if source == target {
            return true;
        }
        match (self.arena.get(source), self.arena.get(target)) {
            (Type::StringLiteral(a), Type::StringLiteral(b)) => a.value == b.value,
            (Type::NumericLiteral(a), Type::NumericLiteral(b)) => a.value == b.value,
            (Type::BooleanLiteral(a), Type::BooleanLiteral(b)) => a.value == b.value,
            (a, b) if is_intrinsic_scalar(a) && is_intrinsic_scalar(b) => {
                discriminant(a) == discriminant(b)
            }
            _ => false,
        }
    }

    pub fn is_subtype_of(&self, source: TypeId, target: TypeId) -> RelationResult {
        if self.is_subtype_internal(source, target) {
            RelationResult::Related
        } else {
            RelationResult::Error(Vec::new())
        }
    }

    pub fn is_type_assignable_to(
        &self,
        source: TypeId,
        target: TypeId,
        target_node: NodeId,
    ) -> RelationResult {
        let span = Span {
            pos: target_node.as_u32(),
            end: target_node.as_u32(),
        };
        self.is_type_assignable_to_span(source, target, span)
    }

    pub fn is_type_assignable_to_span(
        &self,
        source: TypeId,
        target: TypeId,
        target_span: Span,
    ) -> RelationResult {
        let mut ctx = RelationContext {
            visiting: HashSet::new(),
        };
        match self.assignable_internal(source, target, target_span, &mut ctx) {
            Ok(()) => RelationResult::Related,
            Err(diagnostics) => RelationResult::Error(diagnostics),
        }
    }

    pub fn check_type_assignable(
        &self,
        source: TypeId,
        target: TypeId,
        target_node: NodeId,
    ) -> Vec<Diagnostic> {
        match self.is_type_assignable_to(source, target, target_node) {
            RelationResult::Related => Vec::new(),
            RelationResult::Error(diagnostics) => diagnostics,
        }
    }

    pub fn check_type_assignable_span(
        &self,
        source: TypeId,
        target: TypeId,
        target_span: Span,
    ) -> Vec<Diagnostic> {
        match self.is_type_assignable_to_span(source, target, target_span) {
            RelationResult::Related => Vec::new(),
            RelationResult::Error(diagnostics) => diagnostics,
        }
    }

    pub fn get_non_alias_type(&self, type_id: TypeId) -> TypeId {
        get_non_alias_type(self.arena, type_id)
    }

    fn is_subtype_internal(&self, source: TypeId, target: TypeId) -> bool {
        let source = self.get_non_alias_type(source);
        let target = self.get_non_alias_type(target);
        if self.is_type_identical_to(source, target) {
            return true;
        }
        if matches!(self.arena.get(source), Type::Never)
            || matches!(self.arena.get(target), Type::Unknown)
        {
            return true;
        }
        if self.scalar_related(source, target) {
            return true;
        }
        if let (Some(source_info), Some(target_info)) = (
            numeric_scalar_info(self.arena.get(source)),
            numeric_scalar_info(self.arena.get(target)),
        ) {
            if source_info.range.unbounded && target_info.range.unbounded {
                return false;
            }
            return (!target_info.integer || source_info.integer)
                && range_subsumes(&target_info.range, &source_info.range);
        }
        false
    }

    fn assignable_internal(
        &self,
        source: TypeId,
        target: TypeId,
        target_span: Span,
        ctx: &mut RelationContext,
    ) -> Result<(), Vec<Diagnostic>> {
        let source = self.get_non_alias_type(source);
        let target = self.get_non_alias_type(target);

        if self.is_type_identical_to(source, target) || self.is_subtype_internal(source, target) {
            return Ok(());
        }

        // Matches TS: a template parameter is assignable wherever its constraint is.
        if let Type::TemplateParameter(param) = self.arena.get(source) {
            if let Some(constraint) = param.constraint {
                return self
                    .assignable_internal(constraint, target, target_span, ctx)
                    .map_err(|_| vec![self.unassignable(source, target, target_span)]);
            }
        }

        if matches!(self.arena.get(source), Type::Error)
            || matches!(self.arena.get(target), Type::Error | Type::Unknown)
        {
            return Ok(());
        }
        if matches!(self.arena.get(target), Type::Void) {
            return if matches!(self.arena.get(source), Type::Void) {
                Ok(())
            } else {
                Err(vec![self.unassignable(source, target, target_span)])
            };
        }

        // Matches TS: a reflection model target (`Reflection.Model`, ...) accepts exactly the
        // types of its kind; it is never related structurally.
        if self.reflection_kind(target).is_some() {
            return if self.is_reflection_assignment(source, target) {
                Ok(())
            } else {
                Err(vec![self.unassignable(source, target, target_span)])
            };
        }

        if self.literal_assignable_to(source, target) {
            return Ok(());
        }

        match (self.arena.get(source), self.arena.get(target)) {
            (Type::Union(source_union), _) => {
                // A union can reach itself (`union Foo { self: Foo }`). Coming back to the same
                // pair resolves optimistically: a purely cyclic union has no values, so like
                // `never` it is vacuously assignable.
                if !ctx.visiting.insert((source, target)) {
                    return Ok(());
                }
                let mut result = Ok(());
                for variant in source_union.variants.values() {
                    let Some(variant_type) = self.union_variant_target(*variant) else {
                        continue;
                    };
                    if self
                        .assignable_internal(variant_type, target, target_span, ctx)
                        .is_err()
                    {
                        result = Err(vec![self.unassignable(source, target, target_span)]);
                        break;
                    }
                }
                ctx.visiting.remove(&(source, target));
                result
            }
            (_, Type::Union(target_union)) => {
                // Being assignable to a union only requires one variant to match, so coming back
                // to the same pair brings no new information and is resolved as unrelated.
                if !ctx.visiting.insert((source, target)) {
                    return Err(vec![self.unassignable(source, target, target_span)]);
                }
                let mut related = false;
                for variant in target_union.variants.values() {
                    let Some(variant_type) = self.union_variant_target(*variant) else {
                        continue;
                    };
                    if self
                        .assignable_internal(source, variant_type, target_span, ctx)
                        .is_ok()
                    {
                        related = true;
                        break;
                    }
                }
                ctx.visiting.remove(&(source, target));
                if related {
                    Ok(())
                } else {
                    Err(vec![self.unassignable(source, target, target_span)])
                }
            }
            (Type::Model(source_model), Type::Model(target_model)) => {
                // TS `isTypeAssignableToWorker` model/indexer rules.
                let source_is_array = self.is_integer_indexer(source_model.indexer);
                if target_model.name != "object"
                    && target_model.indexer.is_none()
                    && source_is_array
                {
                    let key = source_model.indexer.map(|indexer| indexer.key).unwrap();
                    return Err(vec![self.missing_index(key, target, target_span)]);
                }
                if self.is_integer_indexer(target_model.indexer) {
                    return if source_is_array {
                        self.has_index_and_is_assignable_to(source, target, target_span, ctx)
                    } else {
                        Err(vec![self.unassignable(source, target, target_span)])
                    };
                }
                self.models_assignable(source, target, target_span, ctx)
            }
            // TS `isTupleAssignableToArray` (min/max items are not checked yet).
            (Type::Tuple(tuple), Type::Model(target_model))
                if self.is_integer_indexer(target_model.indexer) =>
            {
                let item_target = target_model.indexer.unwrap().value;
                for item in &tuple.values {
                    self.assignable_internal(*item, item_target, target_span, ctx)?;
                }
                Ok(())
            }
            // TS `isTupleAssignableToTuple`.
            (Type::Tuple(source_tuple), Type::Tuple(target_tuple)) => {
                if source_tuple.values.len() != target_tuple.values.len() {
                    let mut diagnostic = self.unassignable(source, target, target_span);
                    diagnostic.message = format!(
                        "{}\n  Source has {} element(s) but target requires {}.",
                        diagnostic.message,
                        source_tuple.values.len(),
                        target_tuple.values.len()
                    );
                    return Err(vec![diagnostic]);
                }
                for (source_item, target_item) in
                    source_tuple.values.iter().zip(target_tuple.values.iter())
                {
                    self.assignable_internal(*source_item, *target_item, target_span, ctx)?;
                }
                Ok(())
            }
            (Type::Enum(_), Type::Enum(_)) => {
                Err(vec![self.unassignable(source, target, target_span)])
            }
            (Type::EnumMember(member), Type::Enum(_)) if member.r#enum == target => Ok(()),
            _ => Err(vec![self.unassignable(source, target, target_span)]),
        }
    }

    fn models_assignable(
        &self,
        source_id: TypeId,
        target_id: TypeId,
        target_span: Span,
        ctx: &mut RelationContext,
    ) -> Result<(), Vec<Diagnostic>> {
        if !ctx.visiting.insert((source_id, target_id)) {
            return Ok(());
        }

        let mut errors = Vec::new();
        // TS `areModelsRelated`: the source's own properties not matched by a target property
        // must satisfy the target indexer.
        let mut remaining = self
            .model(source_id)
            .map(|model| model.properties.clone())
            .unwrap_or_default();
        for (target_name, target_prop_id) in self.walk_model_properties_inherited(target_id) {
            remaining.shift_remove(&target_name);
            let Some(target_prop) = self.model_property(target_prop_id) else {
                continue;
            };
            let source_prop_id = self.get_property_inherited(source_id, &target_name);
            match source_prop_id.and_then(|id| self.model_property(id).map(|prop| (id, prop))) {
                None => {
                    if !target_prop.optional {
                        errors.push(self.missing_property(
                            &target_name,
                            source_id,
                            target_id,
                            target_span,
                        ));
                    }
                }
                Some((_source_prop_id, source_prop)) => {
                    if source_prop.optional && !target_prop.optional {
                        errors.push(self.property_required(&target_name, target_id, target_span));
                    }
                    if let Err(mut diagnostics) = self.assignable_internal(
                        source_prop.property_type,
                        target_prop.property_type,
                        target_span,
                        ctx,
                    ) {
                        if let Some(first) = diagnostics.first_mut() {
                            first.message = format!(
                                "Types of property '{}' are incompatible\n  {}",
                                source_prop.name, first.message
                            );
                            first.code = "property-unassignable";
                        }
                        errors.extend(diagnostics);
                    }
                }
            }
        }
        if let Some(indexer) = self.model(target_id).and_then(|model| model.indexer) {
            for prop_id in remaining.values() {
                let Some(prop) = self.model_property(*prop_id) else {
                    continue;
                };
                if let Err(diagnostics) =
                    self.assignable_internal(prop.property_type, indexer.value, target_span, ctx)
                {
                    errors.extend(diagnostics);
                    break;
                }
            }
            // Named models must declare a compatible indexer; anonymous models need not.
            let source_named = self.model(source_id).is_some_and(|m| !m.name.is_empty());
            if source_named && !self.is_integer_indexer(Some(indexer)) {
                if let Err(diagnostics) =
                    self.has_index_and_is_assignable_to(source_id, target_id, target_span, ctx)
                {
                    errors.extend(diagnostics);
                }
            }
        } else if let Some(object_value_properties) = self.object_value_properties {
            // TS `shouldCheckExcessProperties`: only object values are checked.
            for (name, prop_id) in &remaining {
                if object_value_properties.contains(prop_id) {
                    errors.push(Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: "unexpected-property",
                        message: format!(
                            "Object value may only specify known properties, and '{}' does not exist in type '{}'.",
                            name,
                            self.type_name(target_id)
                        ),
                        target: target_span,
                    });
                }
            }
        }
        ctx.visiting.remove(&(source_id, target_id));
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn literal_assignable_to(&self, source: TypeId, target: TypeId) -> bool {
        match (self.arena.get(source), self.arena.get(target)) {
            (Type::StringLiteral(source), Type::StringLiteral(target)) => {
                source.value == target.value
            }
            (Type::NumericLiteral(source), Type::NumericLiteral(target)) => {
                source.value == target.value
            }
            (Type::BooleanLiteral(source), Type::BooleanLiteral(target)) => {
                source.value == target.value
            }
            // TS `isSimpleTypeAssignableTo` string template rules.
            (Type::StringTemplate(source), Type::StringLiteral(target)) => {
                source.string_value.as_deref() == Some(target.value.as_str())
            }
            (Type::StringLiteral(source), Type::StringTemplate(target)) => {
                target.string_value.as_deref() == Some(source.value.as_str())
            }
            (Type::StringTemplate(source), Type::StringTemplate(target)) => {
                target.string_value.is_some() && source.string_value == target.string_value
            }
            (Type::StringLiteral(_) | Type::StringTemplate(_), Type::String) => true,
            (Type::BooleanLiteral(_), Type::Boolean) => true,
            (Type::StringLiteral(_) | Type::StringTemplate(_), _) => {
                self.scalar_related(target, intrinsic_target_id(self.arena, "string"))
            }
            (Type::BooleanLiteral(_), _) => {
                self.scalar_related(target, intrinsic_target_id(self.arena, "boolean"))
            }
            (Type::NumericLiteral(source), _) => {
                self.numeric_literal_assignable_to_scalar(&source.value, target)
            }
            _ => false,
        }
    }

    fn numeric_literal_assignable_to_scalar(&self, value: &Numeric, target: TypeId) -> bool {
        let target = self.nearest_numeric_intrinsic(target);
        let Some(target) = target else {
            return false;
        };
        match self.arena.get(target) {
            Type::Numeric | Type::Decimal | Type::Decimal128 | Type::Float => true,
            Type::Integer => value.is_integer(),
            ty => {
                let Some(info) = numeric_scalar_info(ty) else {
                    return false;
                };
                (!info.integer || value.is_integer()) && is_in_range(value, &info.range)
            }
        }
    }

    fn nearest_numeric_intrinsic(&self, type_id: TypeId) -> Option<TypeId> {
        let mut current = self.get_non_alias_type(type_id);
        let mut seen = HashSet::new();
        while seen.insert(current) {
            if numeric_scalar_info(self.arena.get(current)).is_some() {
                return Some(current);
            }
            match self.arena.get(current) {
                Type::Scalar(scalar) => current = scalar.base_scalar?,
                _ => return None,
            }
        }
        None
    }

    fn scalar_related(&self, source: TypeId, target: TypeId) -> bool {
        let source = self.get_non_alias_type(source);
        let target = self.get_non_alias_type(target);
        if source == target {
            return true;
        }
        let target_name = intrinsic_scalar_name(self.arena.get(target));
        let mut current = Some(source);
        let mut seen = HashSet::new();
        while let Some(id) = current {
            if !seen.insert(id) {
                return false;
            }
            if id == target {
                return true;
            }
            let ty = self.arena.get(id);
            if let (Some(current_name), Some(target_name)) =
                (intrinsic_scalar_name(ty), target_name)
            {
                if current_name == target_name {
                    return true;
                }
                current = intrinsic_base_name(current_name)
                    .and_then(|base| find_intrinsic_scalar(self.arena, base));
            } else {
                current = match ty {
                    Type::Scalar(ScalarType { base_scalar, .. }) => *base_scalar,
                    _ => None,
                };
            }
        }
        false
    }

    fn is_reflection_assignment(&self, source: TypeId, target: TypeId) -> bool {
        let Some(reflection_kind) = self.reflection_kind(target) else {
            return false;
        };
        let source = self.get_non_alias_type(source);
        matches!(
            (reflection_kind.as_str(), self.arena.get(source)),
            ("Enum", Type::Enum(_))
                | ("EnumMember", Type::EnumMember(_))
                | ("Interface", Type::Interface(_))
                | ("Model", Type::Model(_))
                | ("ModelProperty", Type::ModelProperty(_))
                | ("Namespace", Type::Namespace(_))
                | ("Operation", Type::Operation(_))
                | ("Scalar", Type::Scalar(_))
                | ("Scalar", Type::String)
                | ("Scalar", Type::Bytes)
                | ("Scalar", Type::Int8)
                | ("Scalar", Type::Int16)
                | ("Scalar", Type::Int32)
                | ("Scalar", Type::Int64)
                | ("Scalar", Type::Uint8)
                | ("Scalar", Type::Uint16)
                | ("Scalar", Type::Uint32)
                | ("Scalar", Type::Uint64)
                | ("Scalar", Type::Float)
                | ("Scalar", Type::Float32)
                | ("Scalar", Type::Float64)
                | ("Scalar", Type::Decimal)
                | ("Scalar", Type::Decimal128)
                | ("Scalar", Type::Integer)
                | ("Scalar", Type::Numeric)
                | ("Scalar", Type::SafeInt)
                | ("Scalar", Type::Boolean)
                | ("Union", Type::Union(_))
                | ("UnionVariant", Type::UnionVariant(_))
        )
    }

    fn reflection_kind(&self, target: TypeId) -> Option<String> {
        let Type::Model(model) = self.arena.get(target) else {
            return None;
        };
        if !matches!(
            model.name.as_str(),
            "Enum"
                | "EnumMember"
                | "Interface"
                | "Model"
                | "ModelProperty"
                | "Namespace"
                | "Operation"
                | "Scalar"
                | "Union"
                | "UnionVariant"
        ) {
            return None;
        }
        let namespace = model.namespace?;
        let Type::Namespace(ns) = self.arena.get(namespace) else {
            return None;
        };
        if ns.name != "Reflection" {
            return None;
        }
        let parent = ns.namespace?;
        let Type::Namespace(parent_ns) = self.arena.get(parent) else {
            return None;
        };
        if parent_ns.name == "TypeSpec" {
            Some(model.name.clone())
        } else {
            None
        }
    }

    fn union_variant_target(&self, variant_id: TypeId) -> Option<TypeId> {
        match self.arena.get(variant_id) {
            Type::UnionVariant(variant) => Some(self.get_non_alias_type(variant.variant_type)),
            _ => None,
        }
    }

    fn walk_model_properties_inherited(&self, model_id: TypeId) -> Vec<(String, TypeId)> {
        let mut result = Vec::new();
        let Some(model) = self.model(model_id) else {
            return result;
        };
        for (name, prop) in &model.properties {
            result.push((name.clone(), *prop));
        }
        if let Some(base) = model.base_model {
            result.extend(self.walk_model_properties_inherited(base));
        }
        result
    }

    fn get_property_inherited(&self, model_id: TypeId, name: &str) -> Option<TypeId> {
        let model = self.model(model_id)?;
        model.properties.get(name).copied().or_else(|| {
            model
                .base_model
                .and_then(|base| self.get_property_inherited(base, name))
        })
    }

    fn model(&self, type_id: TypeId) -> Option<&ModelType> {
        match self.arena.get(type_id) {
            Type::Model(model) => Some(model),
            _ => None,
        }
    }

    /// TS `isArrayModelType`: the indexer key is `integer`.
    fn is_integer_indexer(&self, indexer: Option<ModelIndexer>) -> bool {
        indexer.is_some_and(|indexer| {
            matches!(
                self.arena.get(self.get_non_alias_type(indexer.key)),
                Type::Integer
            )
        })
    }

    /// TS `hasIndexAndIsAssignableTo`.
    fn has_index_and_is_assignable_to(
        &self,
        source: TypeId,
        target: TypeId,
        span: Span,
        ctx: &mut RelationContext,
    ) -> Result<(), Vec<Diagnostic>> {
        let Some(target_indexer) = self.model(target).and_then(|model| model.indexer) else {
            return Ok(());
        };
        match self.model(source).and_then(|model| model.indexer) {
            Some(source_indexer)
                if self.get_non_alias_type(source_indexer.key)
                    == self.get_non_alias_type(target_indexer.key) =>
            {
                self.assignable_internal(source_indexer.value, target_indexer.value, span, ctx)
            }
            _ => Err(vec![self.missing_index(target_indexer.key, source, span)]),
        }
    }

    fn missing_index(&self, key: TypeId, source: TypeId, span: Span) -> Diagnostic {
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "missing-index",
            message: format!(
                "Index signature for type '{}' is missing in type '{}'.",
                self.type_name(key),
                self.type_name(source)
            ),
            target: span,
        }
    }

    fn model_property(&self, type_id: TypeId) -> Option<&ModelPropertyType> {
        match self.arena.get(type_id) {
            Type::ModelProperty(prop) => Some(prop),
            _ => None,
        }
    }

    fn unassignable(&self, source: TypeId, target: TypeId, span: Span) -> Diagnostic {
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "unassignable",
            message: format!(
                "Type '{}' is not assignable to type '{}'",
                self.type_name(source),
                self.type_name(target)
            ),
            target: span,
        }
    }

    fn missing_property(
        &self,
        property: &str,
        source: TypeId,
        target: TypeId,
        span: Span,
    ) -> Diagnostic {
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "missing-property",
            message: format!(
                "Property '{}' is missing on type '{}' but required in '{}'",
                property,
                self.type_name(source),
                self.type_name(target)
            ),
            target: span,
        }
    }

    fn property_required(&self, property: &str, target: TypeId, span: Span) -> Diagnostic {
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: "property-required",
            message: format!(
                "Property '{}' is required in type '{}' but here is optional.",
                property,
                self.type_name(target)
            ),
            target: span,
        }
    }

    fn type_name(&self, type_id: TypeId) -> String {
        type_name(self.arena, type_id)
    }
}

pub fn get_non_alias_type(arena: &TypeArena, type_id: TypeId) -> TypeId {
    let mut current = type_id;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        match arena.get(current) {
            Type::Alias(alias) => current = alias.target,
            _ => return current,
        }
    }
    current
}

pub fn get_effective_model_type(arena: &TypeArena, model_id: TypeId) -> TypeId {
    let model_id = get_non_alias_type(arena, model_id);
    let Type::Model(model) = arena.get(model_id) else {
        return model_id;
    };
    if !model.name.is_empty() || model.properties.is_empty() {
        return model_id;
    }

    let mut candidates: Option<HashSet<TypeId>> = None;
    for prop_id in model.properties.values() {
        let mut sources = named_source_models_for_property(arena, *prop_id);
        if sources.is_empty() {
            return model_id;
        }
        let derived = sources
            .iter()
            .copied()
            .flat_map(|source| derived_models_recursive(arena, source))
            .collect::<Vec<_>>();
        sources.extend(derived);
        candidates = Some(match candidates {
            None => sources,
            Some(existing) => existing.intersection(&sources).copied().collect(),
        });
    }

    let mut candidates = candidates
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|id| id.as_u32());
    candidates
        .into_iter()
        .find(|candidate| count_properties_inherited(arena, *candidate) == model.properties.len())
        .unwrap_or(model_id)
}

fn named_source_models_for_property(arena: &TypeArena, prop_id: TypeId) -> HashSet<TypeId> {
    let mut result = HashSet::new();
    let Type::ModelProperty(prop) = arena.get(prop_id) else {
        return result;
    };
    if let Some(source) = prop.source_property {
        return named_source_models_for_property(arena, source);
    }
    let Type::Model(model) = arena.get(prop.model) else {
        return result;
    };
    if !model.name.is_empty() {
        result.insert(prop.model);
    }
    result
}

fn derived_models_recursive(arena: &TypeArena, model_id: TypeId) -> Vec<TypeId> {
    let Type::Model(model) = arena.get(model_id) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for derived in &model.derived_models {
        result.push(*derived);
        result.extend(derived_models_recursive(arena, *derived));
    }
    result
}

fn count_properties_inherited(arena: &TypeArena, model_id: TypeId) -> usize {
    let Type::Model(model) = arena.get(model_id) else {
        return 0;
    };
    model.properties.len()
        + model
            .base_model
            .map(|base| count_properties_inherited(arena, base))
            .unwrap_or(0)
}

pub fn type_name(arena: &TypeArena, type_id: TypeId) -> String {
    let type_id = get_non_alias_type(arena, type_id);
    match arena.get(type_id) {
        Type::Scalar(s) => format!(
            "{}{}",
            namespace_prefix(arena, s.namespace.filter(|_| !s.expression)),
            s.name
        ),
        Type::Model(model) if model.name.is_empty() && model.properties.is_empty() => {
            "{}".to_string()
        }
        Type::Model(model) if model.name == "Array" && model.indexer.is_some() => {
            format!("{}[]", type_name(arena, model.indexer.unwrap().value))
        }
        Type::Model(model) if model.name.is_empty() => format!(
            "{{ {} }}",
            model
                .properties
                .iter()
                .map(|(name, prop)| match arena.get(*prop) {
                    Type::ModelProperty(prop) =>
                        format!("{name}: {}", type_name(arena, prop.property_type)),
                    _ => name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Model(model) => {
            let name = format!(
                "{}{}",
                namespace_prefix(arena, model.namespace.filter(|_| !model.expression)),
                model.name
            );
            if model.template_arguments.is_empty() {
                name
            } else {
                format_template_name(arena, &name, &model.template_arguments)
            }
        }
        Type::ModelProperty(prop) => format!("{}.{}", type_name(arena, prop.model), prop.name),
        Type::Tuple(tuple) => format!(
            "[{}]",
            tuple
                .values
                .iter()
                .map(|value| type_name(arena, *value))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::StringTemplate(template) => match &template.string_value {
            Some(value) if !value.is_empty() => format!("\"{value}\""),
            _ => "string".to_string(),
        },
        Type::StringTemplateSpan(span) => type_name(arena, span.span_type),
        Type::Enum(en) => format!(
            "{}{}",
            namespace_prefix(arena, en.namespace.filter(|_| !en.expression)),
            en.name
        ),
        Type::EnumMember(member) => {
            format!("{}.{}", type_name(arena, member.r#enum), member.name)
        }
        Type::Union(union) if union.name.is_empty() => union_type_name(arena, union),
        Type::Union(union) => {
            let name = format!(
                "{}{}",
                namespace_prefix(arena, union.namespace.filter(|_| !union.expression)),
                union.name
            );
            if union.template_arguments.is_empty() {
                name
            } else {
                format_template_name(arena, &name, &union.template_arguments)
            }
        }
        Type::UnionVariant(variant) => type_name(arena, variant.variant_type),
        Type::Alias(alias) if !alias.template_arguments.is_empty() => {
            format_template_name(arena, &alias.name, &alias.template_arguments)
        }
        Type::Alias(alias) => alias.name.clone(),
        Type::Operation(operation) => {
            let prefix = match operation.interface {
                Some(interface) => format!("{}.", type_name(arena, interface)),
                None => namespace_prefix(arena, operation.namespace),
            };
            let name = format!("{prefix}{}", operation.name);
            if operation.template_arguments.is_empty() {
                name
            } else {
                format_template_name(arena, &name, &operation.template_arguments)
            }
        }
        Type::Interface(interface) => {
            let name = format!(
                "{}{}",
                namespace_prefix(arena, interface.namespace),
                interface.name
            );
            if interface.template_arguments.is_empty() {
                name
            } else {
                format_template_name(arena, &name, &interface.template_arguments)
            }
        }
        Type::Namespace(_) => namespace_full_name(arena, type_id),
        Type::TemplateParameter(param) => param.name.clone(),
        Type::String => "string".to_string(),
        Type::Bytes => "bytes".to_string(),
        Type::Int8 => "int8".to_string(),
        Type::Int16 => "int16".to_string(),
        Type::Int32 => "int32".to_string(),
        Type::Int64 => "int64".to_string(),
        Type::Uint8 => "uint8".to_string(),
        Type::Uint16 => "uint16".to_string(),
        Type::Uint32 => "uint32".to_string(),
        Type::Uint64 => "uint64".to_string(),
        Type::Float => "float".to_string(),
        Type::Float32 => "float32".to_string(),
        Type::Float64 => "float64".to_string(),
        Type::Decimal => "decimal".to_string(),
        Type::Decimal128 => "decimal128".to_string(),
        Type::Integer => "integer".to_string(),
        Type::Numeric => "numeric".to_string(),
        Type::SafeInt => "safeint".to_string(),
        Type::Boolean => "boolean".to_string(),
        Type::Url => "url".to_string(),
        Type::PlainDate => "plainDate".to_string(),
        Type::PlainTime => "plainTime".to_string(),
        Type::UtcDateTime => "utcDateTime".to_string(),
        Type::OffsetDateTime => "offsetDateTime".to_string(),
        Type::Duration => "duration".to_string(),
        Type::Void => "void".to_string(),
        Type::Never => "never".to_string(),
        Type::Unknown => "unknown".to_string(),
        Type::Null => "null".to_string(),
        Type::StringLiteral(value) => format!("\"{}\"", value.value),
        Type::NumericLiteral(value) => value.value.to_string(),
        Type::BooleanLiteral(value) => value.value.to_string(),
        Type::Error => "<error>".to_string(),
    }
}

/// TS `getTypeName(type, { nameOnly: true })`: no namespace or container qualification.
pub fn type_name_only(arena: &TypeArena, type_id: TypeId) -> String {
    let type_id = get_non_alias_type(arena, type_id);
    let (name, namespace) = match arena.get(type_id) {
        Type::ModelProperty(prop) => return prop.name.clone(),
        Type::EnumMember(member) => return member.name.clone(),
        Type::Namespace(namespace) => return namespace.name.clone(),
        Type::Operation(operation) if operation.interface.is_some() => {
            let full = type_name(arena, type_id);
            let prefix = format!("{}.", type_name(arena, operation.interface.unwrap()));
            return full.strip_prefix(&prefix).unwrap_or(&full).to_string();
        }
        Type::Model(model) => (type_name(arena, type_id), model.namespace),
        Type::Scalar(scalar) => (type_name(arena, type_id), scalar.namespace),
        Type::Enum(en) => (type_name(arena, type_id), en.namespace),
        Type::Union(union) => (type_name(arena, type_id), union.namespace),
        Type::Interface(interface) => (type_name(arena, type_id), interface.namespace),
        Type::Operation(operation) => (type_name(arena, type_id), operation.namespace),
        _ => return type_name(arena, type_id),
    };
    let prefix = namespace_prefix(arena, namespace);
    match name.strip_prefix(&prefix) {
        Some(stripped) => stripped.to_string(),
        None => name,
    }
}

/// TS `getNamespaceFullName`.
fn namespace_full_name(arena: &TypeArena, namespace: TypeId) -> String {
    let mut segments = Vec::new();
    let mut current = Some(namespace);
    while let Some(Type::Namespace(ns)) = current.map(|id| arena.get(id)) {
        if ns.name.is_empty() {
            break;
        }
        segments.push(ns.name.as_str());
        current = ns.namespace;
    }
    segments.reverse();
    segments.join(".")
}

/// TS `getNamespacePrefix`: empty for the global and standard library namespaces.
fn namespace_prefix(arena: &TypeArena, namespace: Option<TypeId>) -> String {
    let Some(namespace) = namespace else {
        return String::new();
    };
    let Type::Namespace(ns) = arena.get(namespace) else {
        return String::new();
    };
    // A missing parent is the global namespace.
    let parent_name = |id: Option<TypeId>| match id.map(|id| arena.get(id)) {
        Some(Type::Namespace(parent)) => Some((parent.name.as_str(), parent.namespace)),
        None => Some(("", None)),
        _ => None,
    };
    let is_std = match parent_name(ns.namespace) {
        Some(("", _)) => ns.name == "TypeSpec",
        Some(("TypeSpec", grandparent)) => {
            ns.name == "Reflection" && matches!(parent_name(grandparent), Some(("", _)))
        }
        _ => false,
    };
    if is_std {
        return String::new();
    }
    let full_name = namespace_full_name(arena, namespace);
    if full_name.is_empty() {
        full_name
    } else {
        format!("{full_name}.")
    }
}

fn format_template_name(arena: &TypeArena, name: &str, args: &[TypeId]) -> String {
    format!(
        "{}<{}>",
        name,
        args.iter()
            .map(|arg| type_name(arena, *arg))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn union_type_name(arena: &TypeArena, union: &UnionType) -> String {
    union
        .variants
        .values()
        .filter_map(|variant_id| match arena.get(*variant_id) {
            Type::UnionVariant(variant) => Some(type_name(arena, variant.variant_type)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn is_intrinsic_scalar(ty: &Type) -> bool {
    intrinsic_scalar_name(ty).is_some()
}

fn intrinsic_scalar_name(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::String => Some("string"),
        Type::Bytes => Some("bytes"),
        Type::Int8 => Some("int8"),
        Type::Int16 => Some("int16"),
        Type::Int32 => Some("int32"),
        Type::Int64 => Some("int64"),
        Type::Uint8 => Some("uint8"),
        Type::Uint16 => Some("uint16"),
        Type::Uint32 => Some("uint32"),
        Type::Uint64 => Some("uint64"),
        Type::Float => Some("float"),
        Type::Float32 => Some("float32"),
        Type::Float64 => Some("float64"),
        Type::Decimal => Some("decimal"),
        Type::Decimal128 => Some("decimal128"),
        Type::Integer => Some("integer"),
        Type::Numeric => Some("numeric"),
        Type::SafeInt => Some("safeint"),
        Type::Boolean => Some("boolean"),
        Type::Url => Some("url"),
        Type::PlainDate => Some("plainDate"),
        Type::PlainTime => Some("plainTime"),
        Type::UtcDateTime => Some("utcDateTime"),
        Type::OffsetDateTime => Some("offsetDateTime"),
        Type::Duration => Some("duration"),
        Type::Void => Some("void"),
        Type::Never => Some("never"),
        Type::Unknown => Some("unknown"),
        Type::Null => Some("null"),
        _ => None,
    }
}

fn intrinsic_base_name(name: &str) -> Option<&'static str> {
    match name {
        "integer" | "float" | "decimal" => Some("numeric"),
        "int64" | "uint64" => Some("integer"),
        "int32" => Some("int64"),
        "int16" => Some("int32"),
        "int8" => Some("int16"),
        "uint32" => Some("uint64"),
        "uint16" => Some("uint32"),
        "uint8" => Some("uint16"),
        "safeint" => Some("int64"),
        "float64" => Some("float"),
        "float32" => Some("float64"),
        "decimal128" => Some("decimal"),
        "url" => Some("string"),
        _ => None,
    }
}

fn find_intrinsic_scalar(arena: &TypeArena, name: &str) -> Option<TypeId> {
    arena.types().iter().enumerate().find_map(|(index, ty)| {
        (intrinsic_scalar_name(ty) == Some(name)).then_some(TypeId::new(index as u32))
    })
}

fn intrinsic_target_id(arena: &TypeArena, name: &str) -> TypeId {
    find_intrinsic_scalar(arena, name).unwrap_or(TypeId::new(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::check;
    use crate::{bind, resolve_names};
    use typespec_parser::parse;
    use typespec_source::SourceFile;

    fn check_source(source: &str) -> crate::checker::CheckResult {
        let source = SourceFile::new(source.to_string());
        let parse_result = parse(&source);
        let mut table = bind(&parse_result);
        let resolution = resolve_names(&parse_result, &mut table);
        check(&parse_result, &table, &resolution.resolutions)
    }

    fn named(result: &crate::checker::CheckResult, name: &str) -> TypeId {
        result
            .type_arena
            .types()
            .iter()
            .enumerate()
            .find_map(|(index, ty)| match ty {
                Type::Model(model) if model.name == name => Some(TypeId::new(index as u32)),
                Type::Alias(alias) if alias.name == name => Some(TypeId::new(index as u32)),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn identity_looks_through_alias_chains() {
        let result = check_source("alias A = string; alias B = A;");
        let relation = TypeRelationChecker::new(&result.type_arena);
        assert!(relation.is_type_identical_to(named(&result, "B"), TypeId::new(16)));
    }

    #[test]
    fn numeric_subtypes_follow_intrinsic_chain() {
        let result = check_source("");
        let relation = TypeRelationChecker::new(&result.type_arena);
        assert!(relation
            .is_subtype_of(TypeId::new(3), TypeId::new(2))
            .is_related());
        assert!(relation
            .is_subtype_of(TypeId::new(5), TypeId::new(3))
            .is_related());
    }

    #[test]
    fn numeric_literals_check_ranges() {
        let result = check_source("alias In = 42; alias Out = 9999;");
        let relation = TypeRelationChecker::new(&result.type_arena);
        let span = Span { pos: 0, end: 0 };
        let Type::Alias(alias_in) = result.type_arena.get(named(&result, "In")) else {
            panic!("alias");
        };
        let Type::Alias(alias_out) = result.type_arena.get(named(&result, "Out")) else {
            panic!("alias");
        };
        assert!(relation
            .is_type_assignable_to_span(alias_in.target, TypeId::new(5), span)
            .is_related());
        assert!(!relation
            .is_type_assignable_to_span(alias_out.target, TypeId::new(5), span)
            .is_related());
    }

    #[test]
    fn union_all_and_any_assignability() {
        let result = check_source(r#"alias A = "a" | "b"; alias B = string;"#);
        let relation = TypeRelationChecker::new(&result.type_arena);
        let span = Span { pos: 0, end: 0 };
        let Type::Alias(a) = result.type_arena.get(named(&result, "A")) else {
            panic!("alias");
        };
        let Type::Alias(b) = result.type_arena.get(named(&result, "B")) else {
            panic!("alias");
        };
        assert!(relation
            .is_type_assignable_to_span(a.target, b.target, span)
            .is_related());
    }

    #[test]
    fn missing_required_model_property_is_an_error() {
        let result = check_source("model Source {} model Target { x: string; }");
        let relation = TypeRelationChecker::new(&result.type_arena);
        let span = Span { pos: 0, end: 0 };
        let diagnostics = relation.check_type_assignable_span(
            named(&result, "Source"),
            named(&result, "Target"),
            span,
        );
        assert!(diagnostics.iter().any(|d| d.code == "missing-property"));
    }

    fn named_union(result: &crate::checker::CheckResult, name: &str) -> TypeId {
        result
            .type_arena
            .types()
            .iter()
            .enumerate()
            .find_map(|(index, ty)| match ty {
                Type::Union(union) if union.name == name => Some(TypeId::new(index as u32)),
                _ => None,
            })
            .unwrap()
    }

    // Variants are named `recur` rather than `self`: the Rust parser rejects reserved keywords as
    // union variant names (see docs/rust-port/upstream-sync.md, Known behavioral differences).
    #[test]
    fn self_referencing_union_source_terminates_and_is_vacuously_assignable() {
        let result = check_source("union Loop { recur: Loop } model Target { name: string }");
        let relation = TypeRelationChecker::new(&result.type_arena);
        let span = Span { pos: 0, end: 0 };
        assert!(relation
            .is_type_assignable_to_span(
                named_union(&result, "Loop"),
                named(&result, "Target"),
                span
            )
            .is_related());
    }

    #[test]
    fn self_referencing_union_target_terminates_and_is_unrelated_without_match() {
        let result = check_source("union Loop { recur: Loop } union Other { o: Loop, s: string }");
        let relation = TypeRelationChecker::new(&result.type_arena);
        let span = Span { pos: 0, end: 0 };
        let diagnostics = relation.check_type_assignable_span(
            TypeId::new(16),
            named_union(&result, "Loop"),
            span,
        );
        assert!(diagnostics.iter().any(|d| d.code == "unassignable"));
        assert!(relation
            .is_type_assignable_to_span(TypeId::new(16), named_union(&result, "Other"), span)
            .is_related());
    }
}
