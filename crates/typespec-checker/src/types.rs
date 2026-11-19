#![allow(non_upper_case_globals)]

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use typespec_parser::ast::NodeId;
use typespec_source::Diagnostic;
use typespec_source::Numeric;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(u32);

impl SymbolId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(u32);

impl ScopeId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(u32);

impl TypeId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SymbolFlags: u32 {
        const None = 0;
        const Model = 1 << 1;
        const Scalar = 1 << 2;
        const Operation = 1 << 3;
        const Enum = 1 << 4;
        const Interface = 1 << 5;
        const Union = 1 << 6;
        const Alias = 1 << 7;
        const Namespace = 1 << 8;
        const Decorator = 1 << 9;
        const TemplateParameter = 1 << 10;
        const Function = 1 << 11;
        const FunctionParameter = 1 << 12;
        const Using = 1 << 13;
        const DuplicateUsing = 1 << 14;
        const SourceFile = 1 << 15;
        const Member = 1 << 16;
        const Const = 1 << 17;
        const Declaration = 1 << 20;
        const Implementation = 1 << 21;
        const LateBound = 1 << 22;
        const Internal = 1 << 23;
    }
}

impl SymbolFlags {
    pub const ExportContainer: Self =
        Self::from_bits_retain(Self::Namespace.bits() | Self::SourceFile.bits());
    pub const MemberContainer: Self = Self::from_bits_retain(
        Self::Model.bits()
            | Self::Enum.bits()
            | Self::Union.bits()
            | Self::Interface.bits()
            | Self::Scalar.bits(),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Global,
    /// Per-file scope holding file-level usings (TS `TypeSpecScriptNode.locals`).
    File,
    Namespace,
    Model,
    ModelExpression,
    Interface,
    Operation,
    Enum,
    Union,
    Scalar,
    Alias,
    Const,
    Function,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Global => "Global",
            ScopeKind::File => "File",
            ScopeKind::Namespace => "Namespace",
            ScopeKind::Model => "Model",
            ScopeKind::ModelExpression => "ModelExpression",
            ScopeKind::Interface => "Interface",
            ScopeKind::Operation => "Operation",
            ScopeKind::Enum => "Enum",
            ScopeKind::Union => "Union",
            ScopeKind::Scalar => "Scalar",
            ScopeKind::Alias => "Alias",
            ScopeKind::Const => "Const",
            ScopeKind::Function => "Function",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub flags: SymbolFlags,
    pub declarations: Vec<NodeId>,
    pub parent_scope: ScopeId,
    pub parent_symbol: Option<SymbolId>,
    pub members: Option<ScopeId>,
    pub symbol_source: Option<SymbolId>,
    /// Type arena entry linked to this symbol during checking.
    ///
    /// M4 pre-binds intrinsic names as symbols before a type arena exists. M5a
    /// keeps the symbol shape ready for that link; `CheckResult::symbol_to_type`
    /// is the immutable per-check view exposed by the current checker API.
    pub r#type: Option<TypeId>,
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub symbols: HashMap<String, SymbolId>,
    pub usings: Vec<ScopeId>,
    pub using_statements: Vec<NodeId>,
    pub owning_node: Option<NodeId>,
    pub owning_symbol: Option<SymbolId>,
}

#[derive(Debug, Clone)]
pub struct SymbolTable {
    pub symbols: Vec<Symbol>,
    pub scopes: Vec<Scope>,
    pub global_scope: ScopeId,
    pub diagnostics: Vec<Diagnostic>,
    pub node_symbols: HashMap<NodeId, SymbolId>,
    pub node_scopes: HashMap<NodeId, ScopeId>,
    pub using_statements: Vec<NodeId>,
    /// Script node -> its [`ScopeKind::File`] scope.
    pub file_scopes: HashMap<NodeId, ScopeId>,
    /// Using statement -> file scope of the script declaring it.
    pub using_files: HashMap<NodeId, ScopeId>,
    /// Using statement -> scope its name is resolved from. File-level usings
    /// declared before a blockless namespace resolve from the global scope.
    pub using_resolution_scopes: HashMap<NodeId, ScopeId>,
    pub augment_decorators: Vec<NodeId>,
    pub duplicate_symbols_reported: HashSet<SymbolId>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let global_scope = ScopeId::new(0);
        Self {
            symbols: Vec::new(),
            scopes: vec![Scope {
                parent: None,
                kind: ScopeKind::Global,
                symbols: HashMap::new(),
                usings: Vec::new(),
                using_statements: Vec::new(),
                owning_node: None,
                owning_symbol: None,
            }],
            global_scope,
            diagnostics: Vec::new(),
            node_symbols: HashMap::new(),
            node_scopes: HashMap::new(),
            using_statements: Vec::new(),
            file_scopes: HashMap::new(),
            using_files: HashMap::new(),
            using_resolution_scopes: HashMap::new(),
            augment_decorators: Vec::new(),
            duplicate_symbols_reported: HashSet::new(),
        }
    }

    pub fn add_scope(
        &mut self,
        parent: Option<ScopeId>,
        kind: ScopeKind,
        owning_node: Option<NodeId>,
        owning_symbol: Option<SymbolId>,
    ) -> ScopeId {
        let id = ScopeId::new(self.scopes.len() as u32);
        self.scopes.push(Scope {
            parent,
            kind,
            symbols: HashMap::new(),
            usings: Vec::new(),
            using_statements: Vec::new(),
            owning_node,
            owning_symbol,
        });
        if let Some(node) = owning_node {
            self.node_scopes.insert(node, id);
        }
        id
    }

    pub fn add_symbol(
        &mut self,
        name: String,
        flags: SymbolFlags,
        declaration: Option<NodeId>,
        parent_scope: ScopeId,
        parent_symbol: Option<SymbolId>,
    ) -> SymbolId {
        let id = SymbolId::new(self.symbols.len() as u32);
        let declarations = declaration.into_iter().collect();
        self.symbols.push(Symbol {
            name,
            flags,
            declarations,
            parent_scope,
            parent_symbol,
            members: None,
            symbol_source: None,
            r#type: None,
        });
        if let Some(node) = declaration {
            self.node_symbols.insert(node, id);
        }
        id
    }

    pub fn add_using_symbol(
        &mut self,
        name: String,
        parent_scope: ScopeId,
        symbol_source: SymbolId,
    ) -> SymbolId {
        let id = SymbolId::new(self.symbols.len() as u32);
        let parent_symbol = self.scopes[parent_scope.index()].owning_symbol;
        self.symbols.push(Symbol {
            name,
            flags: SymbolFlags::Using,
            declarations: Vec::new(),
            parent_scope,
            parent_symbol,
            members: None,
            symbol_source: Some(symbol_source),
            r#type: None,
        });
        id
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Type {
    Scalar(ScalarType),
    Model(ModelType),
    ModelProperty(ModelPropertyType),
    Enum(EnumType),
    EnumMember(EnumMemberType),
    Union(UnionType),
    UnionVariant(UnionVariantType),
    Alias(AliasType),
    Operation(OperationType),
    Interface(InterfaceType),
    Namespace(NamespaceType),
    TemplateParameter(TemplateParameterType),

    // Intrinsics are separate variants rather than `Intrinsic(IntrinsicKind)`
    // so common checker matches stay exhaustive and adding non-intrinsic M5b-e
    // variants does not require a second dispatch enum.
    String,
    Bytes,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float,
    Float32,
    Float64,
    Decimal,
    Decimal128,
    Integer,
    Numeric,
    SafeInt,
    Boolean,
    Url,
    PlainDate,
    PlainTime,
    UtcDateTime,
    OffsetDateTime,
    Duration,
    Void,
    Never,
    Unknown,
    Null,

    StringLiteral(StringLiteralType),
    NumericLiteral(NumericLiteralType),
    BooleanLiteral(BooleanLiteralType),

    Error,
}

impl Type {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Type::Scalar(_) => "Scalar",
            Type::Model(_) => "Model",
            Type::ModelProperty(_) => "ModelProperty",
            Type::Enum(_) => "Enum",
            Type::EnumMember(_) => "EnumMember",
            Type::Union(_) => "Union",
            Type::UnionVariant(_) => "UnionVariant",
            Type::Alias(_) => "Alias",
            Type::Operation(_) => "Operation",
            Type::Interface(_) => "Interface",
            Type::Namespace(_) => "Namespace",
            Type::TemplateParameter(_) => "TemplateParameter",
            Type::String => "String",
            Type::Bytes => "Bytes",
            Type::Int8 => "Int8",
            Type::Int16 => "Int16",
            Type::Int32 => "Int32",
            Type::Int64 => "Int64",
            Type::Uint8 => "Uint8",
            Type::Uint16 => "Uint16",
            Type::Uint32 => "Uint32",
            Type::Uint64 => "Uint64",
            Type::Float => "Float",
            Type::Float32 => "Float32",
            Type::Float64 => "Float64",
            Type::Decimal => "Decimal",
            Type::Decimal128 => "Decimal128",
            Type::Integer => "Integer",
            Type::Numeric => "Numeric",
            Type::SafeInt => "SafeInt",
            Type::Boolean => "Boolean",
            Type::Url => "Url",
            Type::PlainDate => "PlainDate",
            Type::PlainTime => "PlainTime",
            Type::UtcDateTime => "UtcDateTime",
            Type::OffsetDateTime => "OffsetDateTime",
            Type::Duration => "Duration",
            Type::Void => "Void",
            Type::Never => "Never",
            Type::Unknown => "Unknown",
            Type::Null => "Null",
            Type::StringLiteral(_) => "StringLiteral",
            Type::NumericLiteral(_) => "NumericLiteral",
            Type::BooleanLiteral(_) => "BooleanLiteral",
            Type::Error => "Error",
        }
    }
}

// Template convention used by the Rust checker:
// - The original declaration (`model Foo<T> {}`) is a normal arena type with
//   `is_template_declaration = true` and `template_parameters` pointing at
//   `Type::TemplateParameter` entries.
// - Each instantiation (`Foo<string>`) is a separate arena type with
//   `is_template_declaration = false`, `template_node` pointing back to the
//   declaration node, and `template_arguments` holding the canonical arguments
//   after defaults have been applied.
#[derive(Debug, Clone)]
pub struct TemplateParameterType {
    pub name: String,
    pub node: NodeId,
    pub symbol: SymbolId,
    pub constraint: Option<TypeId>,
    pub default: Option<TypeId>,
}

#[derive(Debug, Clone)]
pub struct ScalarType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub base_scalar: Option<TypeId>,
    pub decorators: Vec<DecoratorRef>,
    pub namespace: Option<TypeId>,
    /// Whether this scalar was declared in expression position.
    pub expression: bool,
    pub template_node: Option<NodeId>,
    pub template_arguments: Vec<TypeId>,
    pub template_parameters: Vec<TypeId>,
    pub is_template_declaration: bool,
}

#[derive(Debug, Clone)]
pub struct ModelType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub properties: IndexMap<String, TypeId>,
    pub base_model: Option<TypeId>,
    pub source_model: Option<TypeId>,
    pub indexer: Option<TypeId>,
    pub decorators: Vec<DecoratorRef>,
    pub namespace: Option<TypeId>,
    pub derived_models: Vec<TypeId>,
    /// Whether this model was declared in expression position.
    pub expression: bool,
    pub template_node: Option<NodeId>,
    pub template_arguments: Vec<TypeId>,
    pub template_parameters: Vec<TypeId>,
    pub is_template_declaration: bool,
}

#[derive(Debug, Clone)]
pub struct ModelPropertyType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub model: TypeId,
    pub property_type: TypeId,
    pub optional: bool,
    pub default: Option<TypeId>,
    pub source_property: Option<TypeId>,
    pub decorators: Vec<DecoratorRef>,
}

#[derive(Debug, Clone)]
pub struct EnumType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub members: IndexMap<String, TypeId>,
    pub decorators: Vec<DecoratorRef>,
    pub namespace: Option<TypeId>,
    /// Whether this enum was declared in expression position.
    pub expression: bool,
}

#[derive(Debug, Clone)]
pub struct EnumMemberType {
    pub name: String,
    pub node: Option<NodeId>,
    pub r#enum: TypeId,
    pub value: Option<EnumMemberValue>,
    pub decorators: Vec<DecoratorRef>,
}

#[derive(Debug, Clone)]
pub enum EnumMemberValue {
    String(String),
    Numeric(Numeric),
}

#[derive(Debug, Clone)]
pub struct UnionType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub variants: IndexMap<String, TypeId>,
    pub expression: bool,
    /// Constraint from the union `extends` clause.
    pub base_type: Option<TypeId>,
    pub decorators: Vec<DecoratorRef>,
    pub namespace: Option<TypeId>,
    pub template_node: Option<NodeId>,
    pub template_arguments: Vec<TypeId>,
    pub template_parameters: Vec<TypeId>,
    pub is_template_declaration: bool,
}

#[derive(Debug, Clone)]
pub struct UnionVariantType {
    pub name: String,
    pub node: Option<NodeId>,
    pub union: TypeId,
    pub variant_type: TypeId,
    pub decorators: Vec<DecoratorRef>,
}

#[derive(Debug, Clone)]
pub struct AliasType {
    pub name: String,
    pub node: NodeId,
    pub symbol: SymbolId,
    pub target: TypeId,
    pub namespace: Option<TypeId>,
    pub template_node: Option<NodeId>,
    pub template_arguments: Vec<TypeId>,
    pub template_parameters: Vec<TypeId>,
    pub is_template_declaration: bool,
}

#[derive(Debug, Clone)]
pub struct OperationType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub parameters: TypeId,
    pub return_type: TypeId,
    pub decorators: Vec<DecoratorRef>,
    pub interface: Option<TypeId>,
    pub namespace: Option<TypeId>,
    pub source_operation: Option<TypeId>,
    pub template_node: Option<NodeId>,
    pub template_arguments: Vec<TypeId>,
    pub template_parameters: Vec<TypeId>,
    pub is_template_declaration: bool,
}

#[derive(Debug, Clone)]
pub struct InterfaceType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub operations: IndexMap<String, TypeId>,
    pub source_interfaces: Vec<TypeId>,
    pub decorators: Vec<DecoratorRef>,
    pub namespace: Option<TypeId>,
    pub template_node: Option<NodeId>,
    pub template_arguments: Vec<TypeId>,
    pub template_parameters: Vec<TypeId>,
    pub is_template_declaration: bool,
}

#[derive(Debug, Clone)]
pub struct NamespaceType {
    pub name: String,
    pub node: Option<NodeId>,
    pub symbol: Option<SymbolId>,
    pub namespace: Option<TypeId>,
    pub models: IndexMap<String, TypeId>,
    pub scalars: IndexMap<String, TypeId>,
    pub enums: IndexMap<String, TypeId>,
    pub unions: IndexMap<String, TypeId>,
    pub aliases: IndexMap<String, TypeId>,
    pub operations: IndexMap<String, TypeId>,
    pub interfaces: IndexMap<String, TypeId>,
    pub namespaces: IndexMap<String, TypeId>,
    pub decorators: Vec<DecoratorRef>,
}

#[derive(Debug, Clone)]
pub struct StringLiteralType {
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct NumericLiteralType {
    pub value: Numeric,
}

#[derive(Debug, Clone)]
pub struct BooleanLiteralType {
    pub value: bool,
}

#[derive(Debug, Clone)]
pub struct DecoratorRef {
    pub node: NodeId,
    pub name: String,
    pub args: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub struct TypeArena {
    types: Vec<Type>,
}

impl TypeArena {
    pub fn new() -> Self {
        Self { types: Vec::new() }
    }

    pub fn add(&mut self, ty: Type) -> TypeId {
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(ty);
        id
    }

    pub fn reserve(&mut self) -> TypeId {
        self.add(Type::Error)
    }

    pub fn replace(&mut self, id: TypeId, ty: Type) {
        self.types[id.index()] = ty;
    }

    pub fn get(&self, id: TypeId) -> &Type {
        &self.types[id.index()]
    }

    pub fn get_mut(&mut self, id: TypeId) -> &mut Type {
        &mut self.types[id.index()]
    }

    pub fn types(&self) -> &[Type] {
        &self.types
    }
}

impl Default for TypeArena {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}
