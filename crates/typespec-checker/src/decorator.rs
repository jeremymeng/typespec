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
