//! `#deprecated` directive semantics (M11): TS `checkDirectives`,
//! `checkDeprecated`, `isTypeReferenceContextDeprecated`, `copyDeprecation` and
//! deprecated decorator applications (`applyDecoratorToType`).

use std::collections::HashMap;

use typespec_parser::ast::{NodeData, NodeId};
use typespec_parser::syntax_kind::SyntaxKind;
use typespec_source::{Diagnostic, DiagnosticSeverity};

use super::Checker;
use crate::types::{SymbolFlags, SymbolId, Type, TypeId};

impl<'p> Checker<'p> {
    fn is_deprecated_directive(&self, directive: NodeId) -> bool {
        matches!(
            &self.node(directive).data,
            NodeData::DirectiveExpression { target, .. } if self.identifier_name(*target) == "deprecated"
        )
    }

    /// TS `getDeprecationDetails(program, node)`: the first `#deprecated`
    /// directive's message when it is a string literal.
    pub(super) fn node_deprecation_message(&self, node: NodeId) -> Option<String> {
        let directive = self
            .node(node)
            .directives
            .iter()
            .copied()
            .find(|directive| self.is_deprecated_directive(*directive))?;
        let NodeData::DirectiveExpression { arguments, .. } = &self.node(directive).data else {
            return None;
        };
        match &self.node(*arguments.first()?).data {
            NodeData::StringLiteral { value } => Some(value.clone()),
            _ => None,
        }
    }

    /// TS `checkDirectives`, run for declarations that create a type (the
    /// state-map deprecation itself is derived by [`Self::type_deprecation`]).
    pub(super) fn check_deprecation_directives(&mut self) {
        for node in self.all_nodes() {
            if self.node(node).directives.is_empty() || !creates_type(self.node(node).kind) {
                continue;
            }
            let mut has_deprecation = false;
            for directive in self.node(node).directives.clone() {
                if !self.is_deprecated_directive(directive) {
                    continue;
                }
                let NodeData::DirectiveExpression { arguments, .. } =
                    self.node(directive).data.clone()
                else {
                    continue;
                };
                let Some(message) = arguments.first().copied() else {
                    self.report(
                        "invalid-deprecation-argument",
                        "#deprecation directive is expecting a message argument but none was provided.",
                        directive,
                    );
                    continue;
                };
                if self.node(message).kind != SyntaxKind::StringLiteral {
                    self.report(
                        "invalid-deprecation-argument",
                        &format!(
                            "#deprecation directive is expecting a string literal as the message but got a \"{:?}\"",
                            self.node(message).kind
                        ),
                        message,
                    );
                }
                if has_deprecation {
                    self.diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Warning,
                        code: "duplicate-deprecation",
                        message: "The #deprecated directive cannot be used more than once on the same declaration.".to_string(),
                        target: self.node_span(node),
                    });
                } else {
                    has_deprecation = true;
                }
            }
        }
    }

    /// TS `getDeprecationDetails(program, type)`: the type's own directive,
    /// overridden by a deprecated base model / scalar (`copyDeprecation`).
    pub(crate) fn type_deprecation(&self, type_id: TypeId) -> Option<String> {
        let base = match self.type_arena.get(type_id) {
            Type::Model(model) => model.base_model,
            Type::Scalar(scalar) => scalar.base_scalar,
            _ => None,
        };
        base.filter(|base| *base != type_id)
            .and_then(|base| self.type_deprecation(base))
            .or_else(|| self.own_type_deprecation(type_id))
    }

    fn own_type_deprecation(&self, type_id: TypeId) -> Option<String> {
        let node = self.type_node(type_id)?;
        if !creates_type(self.node(node).kind) {
            return None;
        }
        // Only directives with a valid message mark the type (invalid ones use
        // `<missing message>` in TS).
        let directive = self
            .node(node)
            .directives
            .iter()
            .copied()
            .find(|directive| self.is_deprecated_directive(*directive))?;
        let NodeData::DirectiveExpression { arguments, .. } = &self.node(directive).data else {
            return None;
        };
        match &self.node(*arguments.first()?).data {
            NodeData::StringLiteral { value } => Some(value.clone()),
            _ => Some("<missing message>".to_string()),
        }
    }

    /// Deprecation details of every deprecated type, for the JS adapter's
    /// `TypeSpec.deprecated` state map.
    pub(super) fn collect_deprecations(&self) -> Vec<(TypeId, String)> {
        (0..self.type_arena.types().len())
            .map(|index| TypeId::new(index as u32))
            .filter_map(|type_id| {
                self.type_deprecation(type_id)
                    .map(|message| (type_id, message))
            })
            .collect()
    }

    /// TS `checkTypeReferenceSymbol` deprecation check: report a reference to a
    /// deprecated declaration unless the referencing context is deprecated.
    pub(super) fn check_reference_deprecation(
        &mut self,
        reference: NodeId,
        symbol: SymbolId,
        type_id: TypeId,
    ) {
        let symbol = self.source_symbol(symbol);
        let symbol_data = &self.symbol_table.symbols[symbol.index()];
        if symbol_data.flags.contains(SymbolFlags::TemplateParameter) {
            return;
        }
        let Some(declaration) = symbol_data.declarations.first().copied() else {
            return;
        };
        if !self.reported_deprecations.insert(reference) {
            return;
        }
        if self
            .parent_node(reference)
            .is_some_and(|parent| self.is_reference_context_deprecated(parent))
        {
            return;
        }
        let message = self
            .node_deprecation_message(declaration)
            .or_else(|| self.type_deprecation(type_id));
        if let Some(message) = message {
            self.report_deprecated(&message, reference);
        }
    }

    pub(super) fn report_deprecated(&mut self, message: &str, target: NodeId) {
        self.diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Warning,
            code: "deprecated",
            message: format!("Deprecated: {message}"),
            target: self.node_span(target),
        });
    }

    fn source_symbol(&self, mut symbol: SymbolId) -> SymbolId {
        for _ in 0..64 {
            match self.symbol_table.symbols[symbol.index()].symbol_source {
                Some(source) if source != symbol => symbol = source,
                _ => break,
            }
        }
        symbol
    }

    fn parent_node(&mut self, node: NodeId) -> Option<NodeId> {
        if self.node_parents.is_none() {
            let mut parents = HashMap::new();
            for (index, parent) in self.parse_result.arena.nodes().iter().enumerate() {
                for child in parent.children() {
                    parents.insert(child, NodeId::new(index as u32));
                }
            }
            self.node_parents = Some(parents);
        }
        self.node_parents.as_ref()?.get(&node).copied()
    }

    /// TS `isTypeReferenceContextDeprecated`.
    fn is_reference_context_deprecated(&mut self, node: NodeId) -> bool {
        let has_directive = |checker: &Self, node: NodeId| {
            checker
                .node(node)
                .directives
                .iter()
                .any(|directive| checker.is_deprecated_directive(*directive))
        };
        match self.node(node).kind {
            SyntaxKind::ModelStatement
            | SyntaxKind::InterfaceStatement
            | SyntaxKind::UnionStatement => has_directive(self, node),
            SyntaxKind::OperationStatement => {
                has_directive(self, node)
                    || self.parent_node(node).is_some_and(|parent| {
                        self.node(parent).kind == SyntaxKind::InterfaceStatement
                            && self.is_reference_context_deprecated(parent)
                    })
            }
            SyntaxKind::IntersectionExpression
            | SyntaxKind::UnionExpression
            | SyntaxKind::ArrayExpression
            | SyntaxKind::TupleExpression
            | SyntaxKind::ModelExpression
            | SyntaxKind::ModelProperty
            | SyntaxKind::ModelSpreadProperty
            | SyntaxKind::TypeReference
            | SyntaxKind::TemplateArgument
            | SyntaxKind::TemplateParameterDeclaration
            | SyntaxKind::OperationSignatureDeclaration
            | SyntaxKind::OperationSignatureReference => self
                .parent_node(node)
                .is_some_and(|parent| self.is_reference_context_deprecated(parent)),
            _ => false,
        }
    }
}

/// Declarations TS creates a type for (`createType` runs `checkDirectives`).
fn creates_type(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::ModelStatement
            | SyntaxKind::ScalarStatement
            | SyntaxKind::EnumStatement
            | SyntaxKind::UnionStatement
            | SyntaxKind::InterfaceStatement
            | SyntaxKind::OperationStatement
            | SyntaxKind::NamespaceStatement
            | SyntaxKind::ModelProperty
            | SyntaxKind::EnumMember
            | SyntaxKind::UnionVariant
            | SyntaxKind::DecoratorDeclarationStatement
            | SyntaxKind::FunctionDeclarationStatement
    )
}
