//! M11 decorator validation and resolution (TS `checkDecoratorApplication`,
//! `checkDecoratorDeclaration`, `checkModifiers`, `checkAugmentDecorator`).
//!
//! The Rust checker validates decorator applications (target, arity and argument
//! type/value checks) and resolves the valid ones, in TS execution order, into
//! [`ResolvedDecoratorApplication`]s. The JS host runs the real implementations.

use std::collections::{HashMap, HashSet};

use typespec_parser::ast::{NodeData, NodeId};
use typespec_parser::syntax_kind::SyntaxKind;
use typespec_source::{Diagnostic, DiagnosticSeverity, Span};

use super::Checker;
use crate::decorator::{
    modifier_flags, DecoratorApplicationKind, DecoratorValueEntity, ExternDeclarationInfo,
    ExternDeclarationKind, ResolvedArgumentValue, ResolvedDecoratorApplication,
    ResolvedDecoratorArgument,
};
use crate::type_relation::{type_name, TypeRelationChecker};
use crate::types::{StringTemplateType, SymbolFlags, SymbolId, Type, TypeId};

/// TS `MixedParameterConstraint`: `types` is the type part, `values` the
/// `valueof` part. Union constraints mixing both are split into their options.
#[derive(Debug, Clone, Default)]
struct MixedConstraint {
    types: Option<Vec<TypeId>>,
    values: Option<Vec<TypeId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Type,
    Value,
}

struct ModifierCompatibility {
    allowed: u32,
    required: u32,
    mutually_exclusive: &'static [(u32, u32)],
}

const ALL_MODIFIERS: u32 = modifier_flags::EXTERN | modifier_flags::INTERNAL | modifier_flags::AUTO;

type Mapper = HashMap<TypeId, TypeId>;

impl Checker<'_> {
    pub(super) fn all_nodes(&self) -> Vec<NodeId> {
        let mut result = Vec::new();
        let mut stack: Vec<NodeId> = self.parse_result.scripts.iter().rev().copied().collect();
        while let Some(node) = stack.pop() {
            result.push(node);
            let mut children = self.node(node).children();
            children.reverse();
            stack.extend(children);
        }
        result
    }

    /// TS `checkModifiers` (modifiers.ts).
    pub(super) fn check_declaration_modifiers(&mut self) {
        for node in self.all_nodes() {
            let (modifiers, flags, kind_text, compatibility) = match &self.node(node).data {
                NodeData::NamespaceStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "namespace",
                    NO_MODIFIERS,
                ),
                NodeData::OperationStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (modifiers.clone(), *modifier_flags, "op", DEFAULT_MODIFIERS),
                NodeData::ModelStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "model",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::ScalarStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "scalar",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::InterfaceStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "interface",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::UnionStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "union",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::EnumStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "enum",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::AliasStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "alias",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::ConstStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "const",
                    DEFAULT_MODIFIERS,
                ),
                NodeData::DecoratorDeclarationStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "dec",
                    ModifierCompatibility {
                        allowed: ALL_MODIFIERS,
                        required: modifier_flags::EXTERN | modifier_flags::AUTO,
                        mutually_exclusive: &[(modifier_flags::EXTERN, modifier_flags::AUTO)],
                    },
                ),
                NodeData::FunctionDeclarationStatement {
                    modifiers,
                    modifier_flags,
                    ..
                } => (
                    modifiers.clone(),
                    *modifier_flags,
                    "function",
                    ModifierCompatibility {
                        allowed: modifier_flags::EXTERN | modifier_flags::INTERNAL,
                        required: modifier_flags::EXTERN,
                        mutually_exclusive: &[],
                    },
                ),
                _ => continue,
            };
            let invalid = flags & !compatibility.allowed;
            if invalid != 0 {
                for modifier in modifiers {
                    let Some((text, flag)) = modifier_text(self.node(modifier).kind) else {
                        continue;
                    };
                    if flag & invalid != 0 {
                        self.report(
                            "invalid-modifier",
                            &format!(
                                "Modifier '{text}' cannot be used on declarations of type '{kind_text}'."
                            ),
                            modifier,
                        );
                    }
                }
            }
            if compatibility.required != 0 && flags & compatibility.required == 0 {
                let names = modifier_names(compatibility.required);
                let message = if names.len() == 1 {
                    format!(
                        "Declaration of type '{kind_text}' is missing required modifier '{}'.",
                        names[0]
                    )
                } else {
                    let list = names
                        .iter()
                        .map(|name| format!("'{name}'"))
                        .collect::<Vec<_>>()
                        .join(" or ");
                    format!(
                        "Declaration of type '{kind_text}' is missing one of the required modifiers: {list}."
                    )
                };
                self.report("invalid-modifier", &message, node);
            }
            for (a, b) in compatibility.mutually_exclusive {
                if flags & a != 0 && flags & b != 0 {
                    let a = modifier_names(*a)[0];
                    let b = modifier_names(*b)[0];
                    self.report(
                        "invalid-modifier",
                        &format!("Modifiers '{a}' and '{b}' cannot be used together."),
                        node,
                    );
                }
            }
        }
    }

    /// `dec` / `fn` declarations: `auto-decorator-disabled` and the extern
    /// declaration list the JS host uses for `missing-implementation`.
    pub(super) fn check_decorator_declarations(&mut self) {
        for node in self.all_nodes() {
            let (kind, flags, parameters) = match &self.node(node).data {
                NodeData::DecoratorDeclarationStatement {
                    modifier_flags,
                    parameters,
                    ..
                } => (
                    ExternDeclarationKind::Decorator,
                    *modifier_flags,
                    parameters.clone(),
                ),
                NodeData::FunctionDeclarationStatement {
                    modifier_flags,
                    parameters,
                    ..
                } => (
                    ExternDeclarationKind::Function,
                    *modifier_flags,
                    parameters.clone(),
                ),
                _ => continue,
            };
            if kind == ExternDeclarationKind::Decorator
                && flags & modifier_flags::AUTO != 0
                && !(self.options.auto_decorators || self.feature_enabled("auto-decorators", node))
            {
                self.report(
                    "auto-decorator-disabled",
                    "Auto decorator declarations require the 'auto-decorators' feature to be enabled. Add 'auto-decorators' to the 'features' list in your tspconfig.yaml.",
                    node,
                );
            }
            // TS reports this for project files only (the stdlib's `extern fn typeref` is not).
            if kind == ExternDeclarationKind::Function
                && !(self.options.function_declarations
                    || self.feature_enabled("function-declarations", node))
                && !self.is_library_node(node)
            {
                self.diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: "experimental-feature",
                    message: "Function declarations are an experimental feature that may change in the future. Use with caution and consider providing feedback to the TypeSpec team.".to_string(),
                    target: self.node_span(node),
                });
            }
            let Some(symbol) = self.symbol_for_node(node) else {
                continue;
            };
            let mut parameter_names = Vec::new();
            let mut last_parameter_rest = false;
            for parameter in parameters {
                if let NodeData::FunctionParameter { id, rest, .. } = &self.node(parameter).data {
                    parameter_names.push(self.identifier_name(*id));
                    last_parameter_rest = *rest;
                }
            }
            self.extern_declarations.push(ExternDeclarationInfo {
                fqn: self.symbol_fqn(symbol),
                kind,
                node,
                modifier_flags: flags,
                parameter_names,
                last_parameter_rest,
            });
        }
    }

    /// TS `getFullyQualifiedSymbolName` (namespace path + symbol name).
    pub(super) fn symbol_fqn(&self, symbol: SymbolId) -> String {
        let mut parts = vec![self.symbol_table.symbols[symbol.index()].name.clone()];
        let mut current = self.symbol_table.symbols[symbol.index()].parent_symbol;
        while let Some(parent) = current {
            let parent_symbol = &self.symbol_table.symbols[parent.index()];
            if !parent_symbol.name.is_empty()
                && parent_symbol.flags.contains(SymbolFlags::Namespace)
            {
                parts.push(parent_symbol.name.clone());
            }
            current = parent_symbol.parent_symbol;
        }
        parts.reverse();
        parts.join(".")
    }

    /// TS `checkAugmentDecorator` target checks for resolved targets.
    pub(super) fn check_augment_decorator_targets(&mut self) {
        for augment in self.symbol_table.augment_decorators.clone() {
            let NodeData::AugmentDecoratorStatement { target_type, .. } =
                self.node(augment).data.clone()
            else {
                continue;
            };
            let Some(message) = self.augment_target_error(target_type) else {
                continue;
            };
            self.invalid_augment_decorators.insert(augment);
            self.report("augment-decorator-target", message, target_type);
        }
    }

    fn augment_target_error(&self, target_type: NodeId) -> Option<&'static str> {
        let symbol = self.resolved_symbol(target_type)?;
        if let NodeData::TypeReference { arguments, .. } = &self.node(target_type).data {
            if !arguments.is_empty() {
                return Some("Cannot reference template instances.");
            }
        }
        let value = self.alias_value(symbol)?;
        Some(self.augment_alias_value_error(value, &mut HashSet::new()))
    }

    fn alias_value(&self, symbol: SymbolId) -> Option<NodeId> {
        let symbol = &self.symbol_table.symbols[symbol.index()];
        if !symbol.flags.contains(SymbolFlags::Alias) {
            return None;
        }
        symbol
            .declarations
            .iter()
            .find_map(|node| match &self.node(*node).data {
                NodeData::AliasStatement { value, .. } => Some(*value),
                _ => None,
            })
    }

    fn augment_alias_value_error(&self, value: NodeId, seen: &mut HashSet<NodeId>) -> &'static str {
        const DEFAULT: &str = "Augment decorator first argument must be a type reference.";
        match &self.node(value).data {
            NodeData::TypeReference { arguments, .. } if !arguments.is_empty() => {
                "Cannot reference template instances."
            }
            NodeData::ModelExpression { .. } | NodeData::IntersectionExpression { .. } => {
                "Cannot augment model expressions."
            }
            NodeData::UnionExpression { .. } => "Cannot augment union expressions.",
            NodeData::TypeReference { .. } | NodeData::MemberExpression { .. }
                if seen.insert(value) =>
            {
                match self
                    .resolved_symbol(value)
                    .and_then(|symbol| self.alias_value(symbol))
                {
                    Some(nested) => self.augment_alias_value_error(nested, seen),
                    None => DEFAULT,
                }
            }
            _ => DEFAULT,
        }
    }

    /// Validates every decorator application on every checked type (TS checks
    /// decorators for template declarations and each instance) and, when
    /// requested, records the applications of decorated types in TS order.
    pub(super) fn check_decorator_applications(&mut self) {
        let template_declaration_nodes = self.template_declaration_nodes();
        let augments = self.augment_targets();
        let collect = self.options.collect_decorator_applications;
        // Types cloned from the same declaration with the same mapper (e.g. spread
        // properties) only report diagnostics once, like TS which clones the
        // already-checked decorator list.
        let mut reported: HashSet<(NodeId, Vec<(TypeId, TypeId)>)> = HashSet::new();
        let mut index = 0;
        while index < self.type_arena.types().len() {
            let type_id = TypeId::new(index as u32);
            index += 1;
            let mapper = self.instance_context(type_id);
            let mapper_key = {
                let mut entries = mapper
                    .as_ref()
                    .map(|mapper| mapper.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>())
                    .unwrap_or_default();
                entries.sort_by_key(|(k, v)| (k.index(), v.index()));
                entries
            };
            let mut inline = Vec::new();
            for decorator in self.decorators_for_type(type_id) {
                let first = reported.insert((decorator.node, mapper_key.clone()));
                if let Some(application) = self.check_decorator_application_reporting(
                    type_id,
                    decorator.node,
                    DecoratorApplicationKind::Decorator,
                    mapper.as_ref(),
                    first,
                ) {
                    inline.push(application);
                }
            }
            let mut augmented = Vec::new();
            for (augment, target) in &augments {
                if !self.augment_applies_to(*target, type_id) {
                    continue;
                }
                let first = reported.insert((*augment, mapper_key.clone()));
                if let Some(application) = self.check_decorator_application_reporting(
                    type_id,
                    *augment,
                    DecoratorApplicationKind::Augment,
                    None,
                    first,
                ) {
                    augmented.push(application);
                }
            }
            if self
                .decorator_context(type_id, &template_declaration_nodes)
                .is_none()
            {
                continue;
            }
            // TS `applyDecoratorToType`: reported each time a deprecated decorator runs.
            for application in inline.iter().rev().chain(augmented.iter().rev()) {
                if let Some(message) = self.decorator_deprecation(application.node) {
                    self.report_deprecated(&message, application.node);
                }
            }
            if !collect {
                continue;
            }
            self.push_doc_comment_applications(type_id);
            self.decorator_applications.extend(inline.into_iter().rev());
            self.decorator_applications
                .extend(augmented.into_iter().rev());
        }
    }

    fn check_decorator_application_reporting(
        &mut self,
        target_type: TypeId,
        node: NodeId,
        kind: DecoratorApplicationKind,
        mapper: Option<&Mapper>,
        report: bool,
    ) -> Option<ResolvedDecoratorApplication> {
        let diagnostics = self.diagnostics.len();
        let result = self.check_decorator_application(target_type, node, kind, mapper);
        if !report {
            self.diagnostics.truncate(diagnostics);
        }
        result
    }

    fn augment_targets(&mut self) -> Vec<(NodeId, TypeId)> {
        let mut result = Vec::new();
        for augment in self.symbol_table.augment_decorators.clone() {
            if self.invalid_augment_decorators.contains(&augment) {
                continue;
            }
            let NodeData::AugmentDecoratorStatement { target_type, .. } =
                self.node(augment).data.clone()
            else {
                continue;
            };
            if self.resolved_symbol(target_type).is_none() {
                continue;
            }
            let target = self.type_for_node(target_type);
            if matches!(self.type_arena.get(target), Type::Error) {
                continue;
            }
            result.push((augment, target));
        }
        result
    }

    /// Augment decorators are bound to the target symbol in TS, so they also
    /// apply to template instances and clones of the target declaration.
    fn augment_applies_to(&self, target: TypeId, type_id: TypeId) -> bool {
        if target == type_id {
            return true;
        }
        if matches!(self.type_arena.get(type_id), Type::Namespace(_))
            || std::mem::discriminant(self.type_arena.get(type_id))
                != std::mem::discriminant(self.type_arena.get(target))
        {
            return false;
        }
        match (self.type_node(target), self.type_node(type_id)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// TS `checkDecorators` doc comment handling: `@docFromComment("self", ..)`
    /// plus `returns` / `errors` for operations, executed before decorators.
    fn push_doc_comment_applications(&mut self, type_id: TypeId) {
        let Some(node) = self.type_node(type_id) else {
            return;
        };
        let mut docs = Vec::new();
        if matches!(self.type_arena.get(type_id), Type::Operation(_)) {
            if let Some(errors) = self.doc_tag_content(node, SyntaxKind::DocErrorsTag) {
                docs.push(("errors", errors));
            }
            if let Some(returns) = self.doc_tag_content(node, SyntaxKind::DocReturnsTag) {
                docs.push(("returns", returns));
            }
        }
        if let Some(doc) = self.doc_comment_for_type(type_id) {
            docs.push(("self", doc));
        }
        for (key, doc) in docs {
            self.decorator_applications
                .push(ResolvedDecoratorApplication {
                    target: type_id,
                    decorator: "TypeSpec.@docFromComment".to_string(),
                    node,
                    kind: DecoratorApplicationKind::DocComment,
                    args: [key.to_string(), doc]
                        .into_iter()
                        .map(|value| ResolvedDecoratorArgument {
                            node: None,
                            value: ResolvedArgumentValue::Value(DecoratorValueEntity::String {
                                value,
                                type_id: None,
                            }),
                            value_constraints: Vec::new(),
                        })
                        .collect(),
                });
        }
    }

    /// Last `@returns` / `@errors` doc tag content (TS `getDocContent`).
    fn doc_tag_content(&self, node: NodeId, tag_kind: SyntaxKind) -> Option<String> {
        let mut result = None;
        for doc in &self.node(node).docs {
            let NodeData::Doc { tags, .. } = &self.node(*doc).data else {
                continue;
            };
            for tag in tags {
                if self.node(*tag).kind != tag_kind {
                    continue;
                }
                let NodeData::DocTag { content, .. } = &self.node(*tag).data else {
                    continue;
                };
                let mut text = String::new();
                for chunk in content {
                    if let NodeData::DocText { text: chunk } = &self.node(*chunk).data {
                        text.push_str(chunk);
                    }
                }
                result = Some(text.trim().to_string());
            }
        }
        result
    }

    fn decorator_deprecation(&self, application: NodeId) -> Option<String> {
        let (NodeData::DecoratorExpression { target, .. }
        | NodeData::AugmentDecoratorStatement { target, .. }) = &self.node(application).data
        else {
            return None;
        };
        let declaration = self.decorator_declaration_node(self.resolved_symbol(*target)?)?;
        self.node_deprecation_message(declaration)
    }

    fn decorator_declaration_node(&self, symbol: SymbolId) -> Option<NodeId> {
        self.symbol_table.symbols[symbol.index()]
            .declarations
            .iter()
            .copied()
            .find(|declaration| {
                matches!(
                    self.node(*declaration).data,
                    NodeData::DecoratorDeclarationStatement { .. }
                )
            })
    }

    /// TS `checkDecoratorApplication`: returns the resolved application when
    /// valid, reporting diagnostics otherwise.
    fn check_decorator_application(
        &mut self,
        target_type: TypeId,
        node: NodeId,
        kind: DecoratorApplicationKind,
        mapper: Option<&Mapper>,
    ) -> Option<ResolvedDecoratorApplication> {
        let (target, arguments) = match self.node(node).data.clone() {
            NodeData::DecoratorExpression { target, arguments }
            | NodeData::AugmentDecoratorStatement {
                target, arguments, ..
            } => (target, arguments),
            _ => return None,
        };
        let symbol = self.resolved_symbol(target)?;
        let symbol_data = &self.symbol_table.symbols[symbol.index()];
        if !symbol_data.flags.contains(SymbolFlags::Decorator) {
            let name = symbol_data.name.clone();
            self.report(
                "invalid-decorator",
                &format!("{name} is not a decorator"),
                node,
            );
            return None;
        }
        let decorator = self.symbol_fqn(symbol);
        let Some(declaration) = self.decorator_declaration_node(symbol) else {
            // JS-only decorator (no `dec` declaration): arguments are passed as types.
            let mut args = Vec::new();
            for argument in arguments {
                let arg_type = self.decorator_arg_type(argument, mapper);
                if matches!(self.type_arena.get(arg_type), Type::Error) {
                    return None;
                }
                args.push(ResolvedDecoratorArgument {
                    node: Some(argument),
                    value: ResolvedArgumentValue::Type(arg_type),
                    value_constraints: Vec::new(),
                });
            }
            return Some(ResolvedDecoratorApplication {
                target: target_type,
                decorator,
                node,
                kind,
                args,
            });
        };
        let NodeData::DecoratorDeclarationStatement {
            target: target_parameter,
            parameters,
            ..
        } = self.node(declaration).data.clone()
        else {
            return None;
        };

        let mut has_error = false;
        let target_constraint = self.parameter_constraint(target_parameter);
        // TS `checkUnion` validates a union's decorators (including augments) before its
        // variants are checked, so the (still empty) union is vacuously assignable to any target.
        let unchecked_union_target = matches!(self.type_arena.get(target_type), Type::Union(_));
        if let Some(types) = target_constraint
            .types
            .as_ref()
            .filter(|_| !unchecked_union_target)
        {
            if !self.is_assignable_to_any(target_type, types) {
                let decorator_name = self.symbol_table.symbols[symbol.index()].name.clone();
                let message = format!(
                    "Cannot apply {decorator_name} decorator to {} since it is not assignable to {}",
                    type_name(&self.type_arena, target_type),
                    self.constraint_name(&target_constraint),
                );
                self.report("decorator-wrong-target", &message, node);
                has_error = true;
            }
        }

        let parameter_info = parameters
            .iter()
            .filter_map(|parameter| match &self.node(*parameter).data {
                NodeData::FunctionParameter {
                    type_ref,
                    optional,
                    rest,
                    ..
                } => Some((*type_ref, *optional, *rest)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let min = parameter_info
            .iter()
            .filter(|(_, optional, rest)| !optional && !rest)
            .count();
        let max = match parameter_info.last() {
            Some((_, _, true)) => None,
            _ => Some(parameter_info.len()),
        };
        let actual = arguments.len();
        if actual < min || max.is_some_and(|max| actual > max) {
            if actual < min {
                has_error = true;
            }
            let message = match max {
                None => format!("Expected at least {min} arguments, but got {actual}."),
                Some(max) if max == min => format!("Expected {min} arguments, but got {actual}."),
                Some(max) => format!("Expected {min}-{max} arguments, but got {actual}."),
            };
            self.report("invalid-argument-count", &message, node);
            if actual < min {
                return None;
            }
        }

        let mut args = Vec::new();
        for (index, (type_ref, _, rest)) in parameter_info.iter().enumerate() {
            if *rest {
                let Some(constraint) = self.rest_parameter_constraint(*type_ref) else {
                    break;
                };
                for argument in arguments.iter().skip(index) {
                    match self.resolve_decorator_argument(*argument, &constraint, mapper) {
                        Some(arg) => args.push(arg),
                        None => has_error = true,
                    }
                }
                break;
            }
            let Some(argument) = arguments.get(index) else {
                continue;
            };
            let constraint = self.parameter_constraint_from_type_ref(*type_ref);
            match self.resolve_decorator_argument(*argument, &constraint, mapper) {
                Some(arg) => args.push(arg),
                None => has_error = true,
            }
        }
        if has_error {
            return None;
        }
        Some(ResolvedDecoratorApplication {
            target: target_type,
            decorator,
            node,
            kind,
            args,
        })
    }

    fn parameter_constraint(&mut self, parameter: NodeId) -> MixedConstraint {
        match self.node(parameter).data.clone() {
            NodeData::FunctionParameter { type_ref, .. } => {
                self.parameter_constraint_from_type_ref(type_ref)
            }
            _ => self.parameter_constraint_from_type_ref(None),
        }
    }

    fn parameter_constraint_from_type_ref(&mut self, type_ref: Option<NodeId>) -> MixedConstraint {
        let Some(type_ref) = type_ref else {
            return MixedConstraint {
                types: Some(vec![self.intrinsics.unknown]),
                values: None,
            };
        };
        self.constraint_from_node(type_ref)
    }

    /// TS `checkFunctionParameter` + `checkMixedParameterConstraintUnion`.
    fn constraint_from_node(&mut self, node: NodeId) -> MixedConstraint {
        match self.node(node).data.clone() {
            NodeData::ValueOfExpression { target } => MixedConstraint {
                types: None,
                values: Some(vec![self.type_for_node(target)]),
            },
            NodeData::UnionExpression { options }
                if options.iter().any(|option| {
                    matches!(self.node(*option).data, NodeData::ValueOfExpression { .. })
                }) =>
            {
                let mut types = Vec::new();
                let mut values = Vec::new();
                for option in options {
                    match self.node(option).data.clone() {
                        NodeData::ValueOfExpression { target } => {
                            values.push(self.type_for_node(target))
                        }
                        _ => types.push(self.type_for_node(option)),
                    }
                }
                MixedConstraint {
                    types: (!types.is_empty()).then_some(types),
                    values: (!values.is_empty()).then_some(values),
                }
            }
            _ => MixedConstraint {
                types: Some(vec![self.type_for_node(node)]),
                values: None,
            },
        }
    }

    /// TS `extractRestParamConstraint`: `T[]` / `valueof T[]` -> `T` / `valueof T`.
    fn rest_parameter_constraint(&mut self, type_ref: Option<NodeId>) -> Option<MixedConstraint> {
        let type_ref = type_ref?;
        match self.node(type_ref).data.clone() {
            NodeData::ArrayExpression { element_type } => Some(MixedConstraint {
                types: Some(vec![self.type_for_node(element_type)]),
                values: None,
            }),
            NodeData::ValueOfExpression { target } => match self.node(target).data.clone() {
                NodeData::ArrayExpression { element_type } => Some(MixedConstraint {
                    types: None,
                    values: Some(vec![self.type_for_node(element_type)]),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    fn constraint_name(&self, constraint: &MixedConstraint) -> String {
        let join = |ids: &Vec<TypeId>| {
            ids.iter()
                .map(|id| type_name(&self.type_arena, *id))
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let mut parts = Vec::new();
        if let Some(types) = &constraint.types {
            parts.push(join(types));
        }
        if let Some(values) = &constraint.values {
            parts.push(format!("valueof {}", join(values)));
        }
        parts.join(" | ")
    }

    fn is_assignable(&self, source: TypeId, target: TypeId) -> bool {
        TypeRelationChecker::new(&self.type_arena)
            .with_object_value_properties(&self.object_value_properties)
            .is_type_assignable_to_span(source, target, Span { pos: 0, end: 0 })
            .is_related()
    }

    fn is_assignable_to_any(&self, source: TypeId, targets: &[TypeId]) -> bool {
        targets
            .iter()
            .any(|target| self.is_assignable(source, *target))
    }

    fn decorator_arg_type(&mut self, argument: NodeId, mapper: Option<&Mapper>) -> TypeId {
        let type_id = self.type_for_node(argument);
        match mapper {
            Some(mapper) => self.substitute_type(type_id, mapper),
            None => type_id,
        }
    }

    /// Whether the argument is an "indeterminate" entity in TS (literal, enum
    /// member, union variant or `null`) usable as either a type or a value.
    fn is_indeterminate_argument(&self, argument: NodeId, arg_type: TypeId) -> bool {
        match &self.node(argument).data {
            NodeData::StringLiteral { .. }
            | NodeData::NumericLiteral { .. }
            | NodeData::BooleanLiteral { .. }
            | NodeData::StringTemplateExpression { .. } => true,
            _ => matches!(
                self.type_arena.get(arg_type),
                Type::EnumMember(_) | Type::UnionVariant(_) | Type::Null
            ),
        }
    }

    fn is_value_expression(&self, argument: NodeId) -> bool {
        match &self.node(argument).data {
            NodeData::ObjectLiteral { .. }
            | NodeData::ArrayLiteral { .. }
            | NodeData::CallExpression { .. } => true,
            _ => self.resolved_symbol(argument).is_some_and(|symbol| {
                self.symbol_table.symbols[symbol.index()]
                    .flags
                    .contains(SymbolFlags::Const)
            }),
        }
    }

    fn resolve_decorator_argument(
        &mut self,
        argument: NodeId,
        constraint: &MixedConstraint,
        mapper: Option<&Mapper>,
    ) -> Option<ResolvedDecoratorArgument> {
        let value_constraints = constraint.values.clone().unwrap_or_default();
        if self.is_value_expression(argument) {
            // TS `isAssignableToMixedParameterConstraint` for object/array values, scalar
            // constructors and const references.
            if let (Some(values), Some(value_type)) =
                (&constraint.values, self.value_type(argument))
            {
                if !self.is_assignable_to_any(value_type, values) {
                    let message = format!(
                        "Argument of type '{}' is not assignable to parameter of type '{}'",
                        type_name(&self.type_arena, value_type),
                        values
                            .iter()
                            .map(|id| type_name(&self.type_arena, *id))
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                    self.report("invalid-argument", &message, argument);
                    return None;
                }
            }
            let value = self.value_from_node(argument, mapper);
            return Some(ResolvedDecoratorArgument {
                node: Some(argument),
                value: ResolvedArgumentValue::Value(value),
                value_constraints,
            });
        }
        let arg_type = self.decorator_arg_type(argument, mapper);
        if matches!(self.type_arena.get(arg_type), Type::Error) {
            return None;
        }
        if let Some(value_node) = self.value_placeholders.get(&arg_type).copied() {
            // A template parameter mapped to an object / array value argument.
            let value = self.value_from_node(value_node, None);
            return Some(ResolvedDecoratorArgument {
                node: Some(argument),
                value: ResolvedArgumentValue::Value(value),
                value_constraints,
            });
        }
        if constraint.types.is_none() {
            let message = match &self.node(argument).data {
                NodeData::ModelExpression { .. } => Some(
                    "Is a model expression type, but is being used as a value here. Use #{} to create an object value.",
                ),
                NodeData::TupleExpression { .. } => Some(
                    "Is a tuple type, but is being used as a value here. Use #[] to create an array value.",
                ),
                _ => None,
            };
            if let Some(message) = message {
                self.report("expect-value", message, argument);
                return None;
            }
        }
        let Some(kind) = self.argument_kind(argument, arg_type, constraint) else {
            let message = format!(
                "Argument of type '{}' is not assignable to parameter of type '{}'",
                type_name(&self.type_arena, arg_type),
                self.constraint_name(constraint),
            );
            self.report("invalid-argument", &message, argument);
            return None;
        };
        let value = match kind {
            ArgKind::Type => ResolvedArgumentValue::Type(arg_type),
            ArgKind::Value => {
                if !self.check_string_template_serializable(arg_type) {
                    return None;
                }
                ResolvedArgumentValue::Value(self.value_from_type(arg_type))
            }
        };
        Some(ResolvedDecoratorArgument {
            node: Some(argument),
            value,
            value_constraints,
        })
    }

    /// TS `isAssignableToMixedParameterConstraint` for the resolved argument.
    fn argument_kind(
        &mut self,
        argument: NodeId,
        arg_type: TypeId,
        constraint: &MixedConstraint,
    ) -> Option<ArgKind> {
        if let Type::TemplateParameter(parameter) = self.type_arena.get(arg_type).clone() {
            let source = match self.node(parameter.node).data.clone() {
                NodeData::TemplateParameterDeclaration {
                    constraint: Some(node),
                    ..
                } => self.constraint_from_node(node),
                _ => MixedConstraint {
                    types: Some(vec![self.intrinsics.unknown]),
                    values: None,
                },
            };
            if let (Some(source), Some(target)) = (&source.types, &constraint.types) {
                return source
                    .iter()
                    .all(|source| self.is_assignable_to_any(*source, target))
                    .then_some(ArgKind::Type);
            }
            if let (Some(source), Some(target)) = (&source.values, &constraint.values) {
                return source
                    .iter()
                    .all(|source| self.is_assignable_to_any(*source, target))
                    .then_some(ArgKind::Value);
            }
            return None;
        }
        if self.is_indeterminate_argument(argument, arg_type) {
            if let Some(values) = &constraint.values {
                if self.is_assignable_to_any(arg_type, values) {
                    return Some(ArgKind::Value);
                }
            }
        }
        if let Some(types) = &constraint.types {
            if self.is_assignable_to_any(arg_type, types) {
                return Some(ArgKind::Type);
            }
        }
        None
    }

    /// TS `checkStringValue` on a string template: reports
    /// `explainStringTemplateNotSerializable` when it has no string value.
    fn check_string_template_serializable(&mut self, type_id: TypeId) -> bool {
        let Type::StringTemplate(template) = self.type_arena.get(type_id).clone() else {
            return true;
        };
        if template.string_value.is_some() {
            return true;
        }
        self.report_non_serializable_spans(&template);
        false
    }

    fn report_non_serializable_spans(&mut self, template: &StringTemplateType) {
        for span in &template.spans {
            let Type::StringTemplateSpan(span) = self.type_arena.get(*span).clone() else {
                continue;
            };
            if !span.is_interpolated {
                continue;
            }
            let span_type = self.get_non_alias_type(span.span_type);
            match self.type_arena.get(span_type).clone() {
                Type::StringLiteral(_)
                | Type::NumericLiteral(_)
                | Type::BooleanLiteral(_)
                | Type::Error => {}
                Type::StringTemplate(nested) => self.report_non_serializable_spans(&nested),
                Type::TemplateParameter(param)
                    if self.template_parameter_has_value_constraint(param.node) => {}
                _ => {
                    if let Some(node) = span.node {
                        self.report(
                            "non-literal-string-template",
                            "Value interpolated in this string template cannot be converted to a string. Only literal types can be automatically interpolated.",
                            node,
                        );
                    }
                }
            }
        }
    }

    fn template_parameter_has_value_constraint(&mut self, param_node: NodeId) -> bool {
        match self.node(param_node).data.clone() {
            NodeData::TemplateParameterDeclaration {
                constraint: Some(node),
                ..
            } => self.constraint_from_node(node).values.is_some(),
            _ => false,
        }
    }

    fn value_from_type(&self, type_id: TypeId) -> DecoratorValueEntity {
        match self.type_arena.get(type_id) {
            Type::StringTemplate(StringTemplateType {
                string_value: Some(value),
                ..
            }) => DecoratorValueEntity::String {
                value: value.clone(),
                type_id: Some(type_id),
            },
            Type::StringLiteral(literal) => DecoratorValueEntity::String {
                value: literal.value.clone(),
                type_id: Some(type_id),
            },
            Type::NumericLiteral(literal) => DecoratorValueEntity::Numeric {
                value: literal.value.to_string(),
                type_id: Some(type_id),
            },
            Type::BooleanLiteral(literal) => DecoratorValueEntity::Boolean {
                value: literal.value,
                type_id: Some(type_id),
            },
            Type::Null => DecoratorValueEntity::Null,
            Type::EnumMember(_) => DecoratorValueEntity::EnumMember(type_id),
            _ => DecoratorValueEntity::Unsupported,
        }
    }

    /// TS `ModelProperty.defaultValue` for every property declared with a default.
    pub(super) fn collect_default_values(&mut self) -> Vec<(TypeId, DecoratorValueEntity)> {
        let properties = (0..self.type_arena.types().len())
            .filter_map(|index| {
                let type_id = TypeId::new(index as u32);
                match self.type_arena.get(type_id) {
                    Type::ModelProperty(prop) => prop.node.map(|node| (type_id, node)),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        let mut by_node: HashMap<NodeId, DecoratorValueEntity> = HashMap::new();
        let mut result = Vec::new();
        for (type_id, node) in properties {
            let NodeData::ModelProperty {
                default: Some(default),
                ..
            } = self.node(node).data
            else {
                continue;
            };
            let value = match by_node.get(&default) {
                Some(value) => value.clone(),
                None => {
                    let value = self.value_from_node(default, None);
                    by_node.insert(default, value.clone());
                    value
                }
            };
            result.push((type_id, value));
        }
        result
    }

    fn value_from_node(&mut self, node: NodeId, mapper: Option<&Mapper>) -> DecoratorValueEntity {
        match self.node(node).data.clone() {
            NodeData::ObjectLiteral { properties, .. } => {
                let mut result = Vec::new();
                for property in properties {
                    if let NodeData::ObjectLiteralProperty { id, value } =
                        self.node(property).data.clone()
                    {
                        let name = self.identifier_name(id);
                        let value = self.value_from_node(value, mapper);
                        result.push((name, value));
                    }
                }
                DecoratorValueEntity::Object(result)
            }
            NodeData::ArrayLiteral { values } => DecoratorValueEntity::Array(
                values
                    .into_iter()
                    .map(|value| self.value_from_node(value, mapper))
                    .collect(),
            ),
            _ if self.is_value_expression(node) => DecoratorValueEntity::Unsupported,
            _ => {
                let type_id = self.decorator_arg_type(node, mapper);
                match self.value_placeholders.get(&type_id).copied() {
                    Some(value_node) => self.value_from_node(value_node, None),
                    None => self.value_from_type(type_id),
                }
            }
        }
    }
}

const DEFAULT_MODIFIERS: ModifierCompatibility = ModifierCompatibility {
    allowed: modifier_flags::INTERNAL,
    required: 0,
    mutually_exclusive: &[],
};

const NO_MODIFIERS: ModifierCompatibility = ModifierCompatibility {
    allowed: 0,
    required: 0,
    mutually_exclusive: &[],
};

fn modifier_text(kind: SyntaxKind) -> Option<(&'static str, u32)> {
    match kind {
        SyntaxKind::ExternKeyword => Some(("extern", modifier_flags::EXTERN)),
        SyntaxKind::InternalKeyword => Some(("internal", modifier_flags::INTERNAL)),
        SyntaxKind::AutoKeyword => Some(("auto", modifier_flags::AUTO)),
        _ => None,
    }
}

fn modifier_names(flags: u32) -> Vec<&'static str> {
    let mut names = Vec::new();
    if flags & modifier_flags::EXTERN != 0 {
        names.push("extern");
    }
    if flags & modifier_flags::INTERNAL != 0 {
        names.push("internal");
    }
    if flags & modifier_flags::AUTO != 0 {
        names.push("auto");
    }
    names
}
