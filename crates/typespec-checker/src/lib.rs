#![doc = "Checker crate for the TypeSpec Rust port."]

pub mod binder;
pub mod checker;
pub mod decorator;
pub mod instantiation;
pub mod intrinsics;
pub mod name_resolver;
pub mod numeric_ranges;
pub mod type_relation;
pub mod types;

pub use binder::bind;
pub use checker::{check, check_with_decorator_runner, CheckResult};
pub use decorator::{
    DecoratorApplication, DecoratorArgument, DecoratorContext, DecoratorRunner, DecoratorState,
    DecoratorValue,
};
pub use name_resolver::{resolve_names, NameResolution, ResolutionResult};
pub use types::{
    AliasType, BooleanLiteralType, DecoratorRef, EnumMemberType, EnumMemberValue, EnumType,
    InterfaceType, IntrinsicDeclaration, ModelIndexer, ModelPropertyType, ModelType, NamespaceType,
    NumericLiteralType, OperationType, ScalarType, Scope, ScopeId, ScopeKind, StringLiteralType,
    Symbol, SymbolFlags, SymbolId, SymbolTable, TemplateParameterType, Type, TypeArena, TypeId,
    UnionType, UnionVariantType,
};

#[cfg(test)]
mod tests {
    use typespec_parser::parse;
    use typespec_source::SourceFile;

    use super::*;

    fn bind_source(source: &str) -> (typespec_parser::ParseResult, SymbolTable) {
        let source = SourceFile::new(source.to_string());
        let parse_result = parse(&source);
        let table = bind(&parse_result);
        (parse_result, table)
    }

    fn declared_symbol<'a>(table: &'a SymbolTable, name: &str) -> &'a Symbol {
        table
            .symbols
            .iter()
            .find(|symbol| symbol.name == name && !symbol.declarations.is_empty())
            .expect("symbol should exist")
    }

    #[test]
    fn binds_simple_model_with_property() {
        let (_parse_result, table) = bind_source("model Foo { x: string; }");
        let foo = declared_symbol(&table, "Foo");
        assert!(foo.flags.contains(SymbolFlags::Model));
        let members = foo.members.expect("model should have member scope");
        assert!(table.scopes[members.index()].symbols.contains_key("x"));
    }

    #[test]
    fn creates_nested_namespace_scope_hierarchy() {
        let (_parse_result, table) = bind_source("namespace A { namespace B { model C {} } }");
        let a = declared_symbol(&table, "A");
        let b = declared_symbol(&table, "B");
        let a_scope = a.members.expect("namespace A scope");
        assert_eq!(
            table.symbols[table.scopes[a_scope.index()].owning_symbol.unwrap().index()].name,
            "A"
        );
        assert_eq!(table.symbols[b.parent_symbol.unwrap().index()].name, "A");
    }

    #[test]
    fn reports_duplicate_model_names() {
        let (_parse_result, table) = bind_source("model Foo {}\nmodel Foo {}");
        let diagnostics: Vec<_> = table
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "duplicate-symbol")
            .collect();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].message, "Duplicate name: \"Foo\"");
    }

    #[test]
    fn resolves_stubbed_intrinsic_string() {
        let (parse_result, mut table) = bind_source("model Foo { x: string; }");
        let resolution = resolve_names(&parse_result, &mut table);
        let string_symbol = table
            .symbols
            .iter()
            .position(|symbol| symbol.name == "string")
            .expect("string intrinsic");
        assert!(resolution.resolutions.values().any(
            |result| matches!(result, ResolutionResult::Resolved(id) if id.index() == string_symbol)
        ));
    }
}
