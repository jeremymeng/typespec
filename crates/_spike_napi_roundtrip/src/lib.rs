#![allow(deprecated)]

use std::sync::atomic::{AtomicUsize, Ordering};

use bumpalo::Bump;
use napi::bindgen_prelude::*;
use napi::JsExternal;
use napi_derive::napi;

const DEFAULT_MODEL_COUNT: u32 = 500;
const DEFAULT_PROPS_PER_MODEL: u32 = 5;

static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

pub type NodeId = u32;

// Throwaway spike type: this deliberately mirrors the planned arena shape without
// trying to be a reusable parser AST abstraction.
pub struct MiniArena {
    bump: Bump,
    nodes: Vec<MiniNode>,
    root: NodeId,
}

pub enum MiniNode {
    Module {
        children: Vec<NodeId>,
    },
    ModelDecl {
        name: NodeId,
        properties: Vec<NodeId>,
    },
    Property {
        name: NodeId,
        type_ref: NodeId,
    },
    TypeRef {
        name: NodeId,
    },
    Identifier {
        text: &'static str,
    },
}

struct WrongExternal {
    _value: u32,
}

impl Drop for MiniArena {
    fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

impl MiniArena {
    fn new() -> Self {
        Self {
            bump: Bump::new(),
            nodes: Vec::new(),
            root: 0,
        }
    }

    fn push(&mut self, node: MiniNode) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(node);
        id
    }

    fn push_identifier(&mut self, text: &str) -> NodeId {
        let text = self.bump.alloc_str(text);
        // Spike-only compromise: storing references into a Bump owned by the same
        // struct is self-referential. The arena outlives all nodes, and references
        // are not used during Drop, so this keeps the experiment focused on FFI cost.
        let text: &'static str = unsafe { std::mem::transmute::<&str, &'static str>(text) };
        self.push(MiniNode::Identifier { text })
    }

    fn approximate_external_size(&self) -> i64 {
        let node_bytes = self.nodes.len() * std::mem::size_of::<MiniNode>();
        let vec_bytes = self
            .nodes
            .iter()
            .map(|node| match node {
                MiniNode::Module { children } => {
                    children.capacity() * std::mem::size_of::<NodeId>()
                }
                MiniNode::ModelDecl { properties, .. } => {
                    properties.capacity() * std::mem::size_of::<NodeId>()
                }
                _ => 0,
            })
            .sum::<usize>();
        (std::mem::size_of::<Self>() + node_bytes + vec_bytes) as i64
    }
}

#[napi(object)]
pub struct SerializedTree {
    pub root: u32,
    pub node_count: u32,
    pub nodes: Vec<NodeInfo>,
}

#[napi(object)]
pub struct NodeInfo {
    pub id: u32,
    pub kind: String,
    pub children: Vec<u32>,
    pub name: Option<u32>,
    pub type_ref: Option<u32>,
    pub text: Option<String>,
}

#[napi(object)]
pub struct ArenaStats {
    pub root: u32,
    pub node_count: u32,
}

#[napi(js_name = "parseMiniEager")]
pub fn parse_mini_eager(
    model_count: Option<u32>,
    props_per_model: Option<u32>,
) -> Result<SerializedTree> {
    let arena = build_tree(
        model_count.unwrap_or(DEFAULT_MODEL_COUNT),
        props_per_model.unwrap_or(DEFAULT_PROPS_PER_MODEL),
    )?;
    let nodes = (0..arena.nodes.len())
        .map(|id| node_info(&arena, id as NodeId))
        .collect::<Result<Vec<_>>>()?;

    Ok(SerializedTree {
        root: arena.root,
        node_count: arena.nodes.len() as u32,
        nodes,
    })
}

#[napi(js_name = "parseMiniLazy")]
pub fn parse_mini_lazy(
    env: Env,
    model_count: Option<u32>,
    props_per_model: Option<u32>,
) -> Result<JsExternal> {
    let arena = build_tree(
        model_count.unwrap_or(DEFAULT_MODEL_COUNT),
        props_per_model.unwrap_or(DEFAULT_PROPS_PER_MODEL),
    )?;
    let size_hint = arena.approximate_external_size();
    env.create_external(arena, Some(size_hint))
}

#[napi(js_name = "getNode")]
pub fn get_node(handle: External<MiniArena>, id: u32) -> Result<NodeInfo> {
    node_info(handle.as_ref(), id)
}

#[napi(js_name = "getArenaStats")]
pub fn get_arena_stats(handle: External<MiniArena>) -> ArenaStats {
    ArenaStats {
        root: handle.root,
        node_count: handle.nodes.len() as u32,
    }
}

#[napi(js_name = "getDropCount")]
pub fn get_drop_count() -> u32 {
    DROP_COUNT.load(Ordering::SeqCst) as u32
}

#[napi(js_name = "resetDropCount")]
pub fn reset_drop_count() {
    DROP_COUNT.store(0, Ordering::SeqCst);
}

#[napi(js_name = "parseWrongExternal")]
pub fn parse_wrong_external(env: Env) -> Result<JsExternal> {
    env.create_external(WrongExternal { _value: 42 }, None)
}

pub fn build_tree(model_count: u32, props_per_model: u32) -> Result<MiniArena> {
    if model_count == 0 || props_per_model == 0 {
        return Err(Error::from_reason(
            "model_count and props_per_model must be greater than 0".to_string(),
        ));
    }

    let mut arena = MiniArena::new();
    let string_ident = arena.push_identifier("string");
    let string_type = arena.push(MiniNode::TypeRef { name: string_ident });
    let mut models = Vec::with_capacity(model_count as usize);

    for model_index in 0..model_count {
        let model_name = arena.push_identifier(&format!("Model{model_index}"));
        let mut properties = Vec::with_capacity(props_per_model as usize);

        for prop_index in 0..props_per_model {
            let prop_name = arena.push_identifier(&format!("prop{model_index}_{prop_index}"));
            let property = arena.push(MiniNode::Property {
                name: prop_name,
                type_ref: string_type,
            });
            properties.push(property);
        }

        let model = arena.push(MiniNode::ModelDecl {
            name: model_name,
            properties,
        });
        models.push(model);
    }

    let root = arena.push(MiniNode::Module { children: models });
    arena.root = root;
    Ok(arena)
}

fn node_info(arena: &MiniArena, id: NodeId) -> Result<NodeInfo> {
    let node = arena
        .nodes
        .get(id as usize)
        .ok_or_else(|| Error::from_reason(format!("node id {id} is out of range")))?;

    let info = match node {
        MiniNode::Module { children } => NodeInfo {
            id,
            kind: "Module".to_string(),
            children: children.clone(),
            name: None,
            type_ref: None,
            text: None,
        },
        MiniNode::ModelDecl { name, properties } => {
            let mut children = Vec::with_capacity(properties.len() + 1);
            children.push(*name);
            children.extend(properties.iter().copied());
            NodeInfo {
                id,
                kind: "ModelDecl".to_string(),
                children,
                name: Some(*name),
                type_ref: None,
                text: None,
            }
        }
        MiniNode::Property { name, type_ref } => NodeInfo {
            id,
            kind: "Property".to_string(),
            children: vec![*name, *type_ref],
            name: Some(*name),
            type_ref: Some(*type_ref),
            text: None,
        },
        MiniNode::TypeRef { name } => NodeInfo {
            id,
            kind: "TypeRef".to_string(),
            children: vec![*name],
            name: Some(*name),
            type_ref: None,
            text: None,
        },
        MiniNode::Identifier { text } => NodeInfo {
            id,
            kind: "Identifier".to_string(),
            children: Vec::new(),
            name: None,
            type_ref: None,
            text: Some((*text).to_string()),
        },
    };

    Ok(info)
}
