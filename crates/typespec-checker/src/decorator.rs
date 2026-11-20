use std::collections::HashMap;

use typespec_parser::ast::NodeId;

use crate::types::TypeId;

#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorValue {
    String(String),
    // Intentionally f64: decorator callback state mirrors JavaScript Number values, not literal storage.
    Number(f64),
    Boolean(bool),
    TypeRef(TypeId),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorArgument {
    String(String),
    // Intentionally f64: decorator callbacks receive JavaScript Number-compatible arguments.
    Number(f64),
    Boolean(bool),
    TypeRef(TypeId),
    None,
}

#[derive(Debug, Clone)]
pub struct DecoratorApplication {
    pub node: NodeId,
    pub name: String,
    pub args: Vec<DecoratorArgument>,
}

pub type DecoratorState = HashMap<TypeId, HashMap<String, DecoratorValue>>;

pub trait DecoratorContext {
    fn set_decorator_property(
        &mut self,
        decorator_name: &str,
        target: TypeId,
        property: &str,
        value: DecoratorValue,
    );

    fn get_decorator_property(
        &self,
        decorator_name: &str,
        target: TypeId,
        property: &str,
    ) -> Option<DecoratorValue>;

    fn get_type_name(&self, target: TypeId) -> String;

    fn get_type_kind(&self, target: TypeId) -> String;

    fn report_decorator_diagnostic(
        &mut self,
        code: String,
        message: String,
        target: Option<TypeId>,
        decorator_node: NodeId,
    );
}

pub trait DecoratorRunner {
    fn is_enabled(&self) -> bool;

    fn call_decorator(
        &mut self,
        context: &mut dyn DecoratorContext,
        target: TypeId,
        application: &DecoratorApplication,
    );
}

pub fn decorator_state_key(decorator_name: &str, property: &str) -> String {
    format!("{}:{property}", short_decorator_name(decorator_name))
}

pub fn short_decorator_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Options for [`crate::checker::check_with_options`] (M11).
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// TS `auto-decorators` compiler feature. When disabled `auto dec`
    /// declarations report `auto-decorator-disabled`.
    pub auto_decorators: bool,
    /// Resolve the TS-ordered decorator applications of every non-template type
    /// into [`crate::CheckResult::decorator_applications`] so a JS host can run
    /// the real decorator implementations.
    pub collect_decorator_applications: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoratorApplicationKind {
    /// `@dec` on a declaration or member.
    Decorator,
    /// `@@dec(target, ...)` augment decorator statement.
    Augment,
    /// Doc comment, applied like TS `createDocFromCommentDecorator`
    /// (`TypeSpec.@docFromComment` with `self`/`returns`/`errors`).
    DocComment,
}

/// A validated decorator application, ordered like TS `Type.decorators`.
#[derive(Debug, Clone)]
pub struct ResolvedDecoratorApplication {
    pub target: TypeId,
    /// Fully qualified decorator name, e.g. `TypeSpec.@doc` or `@track`.
    pub decorator: String,
    /// Decorator expression / augment statement; the declaration node for doc comments.
    pub node: NodeId,
    pub kind: DecoratorApplicationKind,
    pub args: Vec<ResolvedDecoratorArgument>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDecoratorArgument {
    /// Argument expression (absent for synthesized doc comment arguments).
    pub node: Option<NodeId>,
    pub value: ResolvedArgumentValue,
    /// `valueof` constraint options of the parameter (TS `extractValueOfConstraints`),
    /// used to marshal numeric values to JS numbers or `Numeric`.
    pub value_constraints: Vec<TypeId>,
}

#[derive(Debug, Clone)]
pub enum ResolvedArgumentValue {
    Type(TypeId),
    Value(DecoratorValueEntity),
}

/// Value passed to a decorator (TS `Value`). The full value system is M12;
/// object/array values are structural and not validated against their constraint.
#[derive(Debug, Clone)]
pub enum DecoratorValueEntity {
    String {
        value: String,
        type_id: Option<TypeId>,
    },
    Numeric {
        value: String,
        type_id: Option<TypeId>,
    },
    Boolean {
        value: bool,
        type_id: Option<TypeId>,
    },
    Null,
    EnumMember(TypeId),
    Object(Vec<(String, DecoratorValueEntity)>),
    Array(Vec<DecoratorValueEntity>),
    /// Value the Rust checker cannot evaluate yet (const references, scalar
    /// constructors, ...; M12).
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternDeclarationKind {
    Decorator,
    Function,
}

/// `dec` / `fn` declaration, exposed so the JS host can bind implementations
/// and report `missing-implementation`.
#[derive(Debug, Clone)]
pub struct ExternDeclarationInfo {
    /// Fully qualified name, e.g. `TypeSpec.@doc` or `TypeSpec.typeref`.
    pub fqn: String,
    pub kind: ExternDeclarationKind,
    pub node: NodeId,
    pub modifier_flags: u32,
    pub parameter_names: Vec<String>,
    pub last_parameter_rest: bool,
}

pub mod modifier_flags {
    pub const EXTERN: u32 = 1 << 1;
    pub const INTERNAL: u32 = 1 << 2;
    pub const AUTO: u32 = 1 << 3;
}
