use std::collections::HashSet;
use std::mem::discriminant;

use typespec_parser::ast::NodeId;
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use crate::numeric_ranges::{is_in_range, numeric_scalar_info, range_subsumes};
use crate::types::{ModelPropertyType, ModelType, ScalarType, Type, TypeArena, TypeId, UnionType};

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
}

struct RelationContext {
    visiting: HashSet<(TypeId, TypeId)>,
}

impl<'a> TypeRelationChecker<'a> {
    pub fn new(arena: &'a TypeArena) -> Self {
        Self { arena }
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
            if source_info.range.min.is_infinite()
                && source_info.range.max.is_infinite()
                && target_info.range.min.is_infinite()
                && target_info.range.max.is_infinite()
            {
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

        if self.is_reflection_assignment(source, target) {
            return Ok(());
        }

        if self.literal_assignable_to(source, target) {
            return Ok(());
        }

        match (self.arena.get(source), self.arena.get(target)) {
            (Type::Union(source_union), _) => {
                for variant in source_union.variants.values() {
                    let Some(variant_type) = self.union_variant_target(*variant) else {
                        continue;
                    };
                    self.assignable_internal(variant_type, target, target_span, ctx)
                        .map_err(|_| vec![self.unassignable(source, target, target_span)])?;
                }
                Ok(())
            }
            (_, Type::Union(target_union)) => {
                for variant in target_union.variants.values() {
                    let Some(variant_type) = self.union_variant_target(*variant) else {
                        continue;
                    };
                    if self
                        .assignable_internal(source, variant_type, target_span, ctx)
                        .is_ok()
                    {
                        return Ok(());
                    }
                }
                Err(vec![self.unassignable(source, target, target_span)])
            }
            (Type::Model(_), Type::Model(_)) => {
                self.models_assignable(source, target, target_span, ctx)
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
        for (target_name, target_prop_id) in self.walk_model_properties_inherited(target_id) {
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
            (Type::StringLiteral(_), Type::String) => true,
            (Type::BooleanLiteral(_), Type::Boolean) => true,
            (Type::StringLiteral(_), _) => {
                self.scalar_related(target, intrinsic_target_id(self.arena, "string"))
            }
            (Type::BooleanLiteral(_), _) => {
                self.scalar_related(target, intrinsic_target_id(self.arena, "boolean"))
            }
            (Type::NumericLiteral(source), _) => {
                self.numeric_literal_assignable_to_scalar(source.value, target)
            }
            _ => false,
        }
    }

    fn numeric_literal_assignable_to_scalar(&self, value: f64, target: TypeId) -> bool {
        let target = self.nearest_numeric_intrinsic(target);
        let Some(target) = target else {
            return false;
        };
        match self.arena.get(target) {
            Type::Numeric | Type::Decimal | Type::Decimal128 | Type::Float => true,
            Type::Integer => value.fract() == 0.0,
            ty => {
                let Some(info) = numeric_scalar_info(ty) else {
                    return false;
                };
                (!info.integer || value.fract() == 0.0) && is_in_range(value, &info.range)
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
                | "Type"
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
        Type::Scalar(s) => s.name.clone(),
        Type::Model(model) if model.name.is_empty() => "{}".to_string(),
        Type::Model(model) => model.name.clone(),
        Type::ModelProperty(prop) => prop.name.clone(),
        Type::Enum(en) => en.name.clone(),
        Type::EnumMember(member) => member.name.clone(),
        Type::Union(union) if union.name.is_empty() => union_type_name(arena, union),
        Type::Union(union) => union.name.clone(),
        Type::UnionVariant(variant) => type_name(arena, variant.variant_type),
        Type::Alias(alias) => alias.name.clone(),
        Type::Operation(operation) => operation.name.clone(),
        Type::Interface(interface) => interface.name.clone(),
        Type::Namespace(namespace) => namespace.name.clone(),
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
        Type::NumericLiteral(value) => format_number(value.value),
        Type::BooleanLiteral(value) => value.value.to_string(),
        Type::Error => "<error>".to_string(),
    }
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

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
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
}
