//! M11: serialization of validated decorator applications, extern declarations
//! and node locations for the JS host that runs the real decorator
//! implementations, plus a type relation registry used by its checker shim
//! (`isTypeAssignableTo`).

use std::cell::RefCell;
use std::collections::BTreeSet;

use napi_derive::napi;
use typespec_checker::{
    type_relation::TypeRelationChecker, BooleanLiteralType, DecoratorApplicationKind,
    DecoratorValueEntity, ExternDeclarationInfo, ExternDeclarationKind, NumericLiteralType,
    ResolvedArgumentValue, ResolvedDecoratorApplication, StringLiteralType, Type, TypeArena,
    TypeId,
};
use typespec_parser::ParseResult;
use typespec_source::{Numeric, ProgramSources, Span};

use crate::checker::SerializedType;

#[napi(object)]
pub struct DecoratorArgumentJs {
    /// `"type"` or `"value"`.
    pub kind: String,
    pub type_id: Option<u32>,
    /// JSON encoded value (`valueKind`, `value`, `typeId`, `properties`, `values`).
    pub value_json: Option<String>,
    pub node_id: Option<u32>,
    /// `valueof` constraint options used to marshal numeric values.
    pub value_constraint_ids: Vec<u32>,
}

#[napi(object)]
pub struct DecoratorApplicationJs {
    pub target_id: u32,
    /// Fully qualified decorator name, e.g. `TypeSpec.@doc`.
    pub decorator: String,
    /// `"decorator"`, `"augment"` or `"docComment"`.
    pub kind: String,
    pub node_id: u32,
    pub args: Vec<DecoratorArgumentJs>,
}

#[napi(object)]
pub struct ExternDeclarationJs {
    pub fqn: String,
    /// `"decorator"` or `"function"`.
    pub kind: String,
    pub node_id: u32,
    pub is_auto: bool,
    pub parameter_names: Vec<String>,
    pub last_parameter_rest: bool,
}

/// Location of an AST node referenced by a type, decorator application or
/// extern declaration (TS `Node` shims).
#[napi(object)]
pub struct NodeInfoJs {
    pub node_id: u32,
    /// TS `SyntaxKind` member name.
    pub kind: String,
    pub file: Option<String>,
    pub pos: u32,
    pub end: u32,
    /// Span of the node's identifier (TS `getSourceLocation` with `locateId`), M13.
    pub id_pos: Option<u32>,
    pub id_end: Option<u32>,
}

pub(crate) struct DecoratorSerialization {
    pub applications: Vec<DecoratorApplicationJs>,
    pub extern_declarations: Vec<ExternDeclarationJs>,
    pub nodes: Vec<NodeInfoJs>,
}

pub(crate) fn serialize_decorators(
    applications: &[ResolvedDecoratorApplication],
    extern_declarations: &[ExternDeclarationInfo],
    types: &[SerializedType],
    parse_result: &ParseResult,
    sources: &ProgramSources,
) -> DecoratorSerialization {
    let mut node_ids = BTreeSet::new();
    node_ids.extend(types.iter().filter_map(|ty| ty.node_id));

    let applications = applications
        .iter()
        .map(|application| {
            node_ids.insert(application.node.as_u32());
            DecoratorApplicationJs {
                target_id: application.target.as_u32(),
                decorator: application.decorator.clone(),
                kind: match application.kind {
                    DecoratorApplicationKind::Decorator => "decorator",
                    DecoratorApplicationKind::Augment => "augment",
                    DecoratorApplicationKind::DocComment => "docComment",
                }
                .to_string(),
                node_id: application.node.as_u32(),
                args: application
                    .args
                    .iter()
                    .map(|arg| {
                        if let Some(node) = arg.node {
                            node_ids.insert(node.as_u32());
                        }
                        let (kind, type_id, value_json) = match &arg.value {
                            ResolvedArgumentValue::Type(id) => ("type", Some(id.as_u32()), None),
                            ResolvedArgumentValue::Value(value) => {
                                let mut json = String::new();
                                write_value_json(&mut json, value);
                                ("value", None, Some(json))
                            }
                        };
                        DecoratorArgumentJs {
                            kind: kind.to_string(),
                            type_id,
                            value_json,
                            node_id: arg.node.map(|node| node.as_u32()),
                            value_constraint_ids: arg
                                .value_constraints
                                .iter()
                                .map(|id| id.as_u32())
                                .collect(),
                        }
                    })
                    .collect(),
            }
        })
        .collect();

    let extern_declarations = extern_declarations
        .iter()
        .map(|declaration| {
            node_ids.insert(declaration.node.as_u32());
            ExternDeclarationJs {
                fqn: declaration.fqn.clone(),
                kind: match declaration.kind {
                    ExternDeclarationKind::Decorator => "decorator",
                    ExternDeclarationKind::Function => "function",
                }
                .to_string(),
                node_id: declaration.node.as_u32(),
                is_auto: declaration.modifier_flags
                    & typespec_checker::decorator::modifier_flags::AUTO
                    != 0,
                parameter_names: declaration.parameter_names.clone(),
                last_parameter_rest: declaration.last_parameter_rest,
            }
        })
        .collect();

    let nodes = node_ids
        .into_iter()
        .filter(|id| (*id as usize) < parse_result.arena.nodes().len())
        .map(|id| {
            let node = parse_result
                .arena
                .get(typespec_parser::ast::NodeId::new(id));
            let (file, pos, end) = match sources.locate_span(Span {
                pos: node.pos,
                end: node.end,
            }) {
                Some((file, local)) => (Some(sources.get(file).path.clone()), local.pos, local.end),
                None => (None, node.pos, node.end),
            };
            let id_span = node.data.locate_id().map(|id_node| {
                let id_node = parse_result.arena.get(id_node);
                let offset = node.pos.saturating_sub(pos);
                (
                    id_node.pos.saturating_sub(offset),
                    id_node.end.saturating_sub(offset),
                )
            });
            NodeInfoJs {
                node_id: id,
                kind: format!("{:?}", node.kind),
                file,
                pos,
                end,
                id_pos: id_span.map(|(pos, _)| pos),
                id_end: id_span.map(|(_, end)| end),
            }
        })
        .collect();

    DecoratorSerialization {
        applications,
        extern_declarations,
        nodes,
    }
}

fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_type_id(out: &mut String, type_id: Option<TypeId>) {
    if let Some(id) = type_id {
        out.push_str(&format!(",\"typeId\":{}", id.as_u32()));
    }
}

pub(crate) fn write_value_json(out: &mut String, value: &DecoratorValueEntity) {
    match value {
        DecoratorValueEntity::String { value, type_id } => {
            out.push_str("{\"valueKind\":\"StringValue\",\"value\":");
            write_json_string(out, value);
            write_type_id(out, *type_id);
            out.push('}');
        }
        DecoratorValueEntity::Numeric { value, type_id } => {
            out.push_str("{\"valueKind\":\"NumericValue\",\"value\":");
            write_json_string(out, value);
            write_type_id(out, *type_id);
            out.push('}');
        }
        DecoratorValueEntity::Boolean { value, type_id } => {
            out.push_str(&format!(
                "{{\"valueKind\":\"BooleanValue\",\"value\":{value}"
            ));
            write_type_id(out, *type_id);
            out.push('}');
        }
        DecoratorValueEntity::Null => out.push_str("{\"valueKind\":\"NullValue\"}"),
        DecoratorValueEntity::EnumMember(id) => out.push_str(&format!(
            "{{\"valueKind\":\"EnumValue\",\"typeId\":{}}}",
            id.as_u32()
        )),
        DecoratorValueEntity::Object(properties) => {
            out.push_str("{\"valueKind\":\"ObjectValue\",\"properties\":[");
            for (index, (name, value)) in properties.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('[');
                write_json_string(out, name);
                out.push(',');
                write_value_json(out, value);
                out.push(']');
            }
            out.push_str("]}");
        }
        DecoratorValueEntity::Array(values) => {
            out.push_str("{\"valueKind\":\"ArrayValue\",\"values\":[");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value_json(out, value);
            }
            out.push_str("]}");
        }
        DecoratorValueEntity::Unsupported => out.push_str("{\"valueKind\":\"Unsupported\"}"),
    }
}

/// Most recent checked type arenas, addressed by `relationHandle`.
const MAX_RETAINED_ARENAS: usize = 8;

thread_local! {
    static ARENAS: RefCell<(u32, Vec<(u32, TypeArena)>)> = const { RefCell::new((0, Vec::new())) };
}

pub(crate) fn retain_arena(arena: TypeArena) -> u32 {
    ARENAS.with(|arenas| {
        let mut arenas = arenas.borrow_mut();
        arenas.0 += 1;
        let handle = arenas.0;
        arenas.1.push((handle, arena));
        if arenas.1.len() > MAX_RETAINED_ARENAS {
            arenas.1.remove(0);
        }
        handle
    })
}

/// Source of an assignability query: a checked type or a literal created on
/// the JS side (TS `checker.createLiteralType`).
#[napi(object)]
pub struct RelationSourceJs {
    pub type_id: Option<u32>,
    pub string: Option<String>,
    /// Numeric literal in its source text form.
    pub numeric: Option<String>,
    pub boolean: Option<bool>,
}

#[napi(object)]
pub struct RelationDiagnosticJs {
    pub code: String,
    pub message: String,
}

#[napi(object)]
pub struct AssignabilityResultJs {
    pub related: bool,
    pub diagnostics: Vec<RelationDiagnosticJs>,
}

/// TS `checker.isTypeAssignableTo` over a program checked with `checkProgram`
/// (`relationHandle`). Returns `undefined` when the handle was released.
#[napi]
pub fn is_type_assignable_to(
    handle: u32,
    source: RelationSourceJs,
    target_id: u32,
) -> Option<AssignabilityResultJs> {
    ARENAS.with(|arenas| {
        let mut arenas = arenas.borrow_mut();
        let (_, arena) = arenas.1.iter_mut().find(|(id, _)| *id == handle)?;
        if target_id as usize >= arena.types().len() {
            return None;
        }
        let source_id = if let Some(id) = source.type_id {
            if id as usize >= arena.types().len() {
                return None;
            }
            TypeId::new(id)
        } else if let Some(value) = source.string {
            arena.add(Type::StringLiteral(StringLiteralType { value }))
        } else if let Some(value) = source.numeric {
            let value = Numeric::from_str(&value).ok()?;
            arena.add(Type::NumericLiteral(NumericLiteralType { value }))
        } else {
            let value = source.boolean?;
            arena.add(Type::BooleanLiteral(BooleanLiteralType { value }))
        };
        let diagnostics = TypeRelationChecker::new(arena).check_type_assignable_span(
            source_id,
            TypeId::new(target_id),
            Span { pos: 0, end: 0 },
        );
        Some(AssignabilityResultJs {
            related: diagnostics.is_empty(),
            diagnostics: diagnostics
                .into_iter()
                .map(|diagnostic| RelationDiagnosticJs {
                    code: diagnostic.code.to_string(),
                    message: diagnostic.message,
                })
                .collect(),
        })
    })
}
