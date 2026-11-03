//! Resolved parse tree: the parslet-shaped value tree the parsanol
//! portable engine produces for the ISC grammar, mirrored into plain
//! Rust types (`TNode`). Mirrors `parsanol::portable::parslet_transform`
//! output consumed by `Interscript::Isc::DocumentBuilder` on the Ruby
//! side.

use parsanol::portable::ast::AstNode;
use parsanol::portable::AstArena;

#[derive(Debug, Clone)]
pub enum TNode {
    Str(String),
    Array(Vec<TNode>),
    /// Ordered key/value pairs (a parslet capture hash).
    Hash(Vec<(String, TNode)>),
    Nil,
}

/// Ordered-pair lookup on a capture-hash slice.
pub fn pget<'a>(pairs: &'a [(String, TNode)], key: &str) -> Option<&'a TNode> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

impl TNode {
    pub fn get(&self, key: &str) -> Option<&TNode> {
        match self {
            TNode::Hash(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The ordered pairs when this node is a capture hash.
    pub fn ruby_array_hash(&self) -> Option<&Vec<(String, TNode)>> {
        match self {
            TNode::Hash(pairs) => Some(pairs),
            _ => None,
        }
    }

    pub fn has_key(&self, key: &str) -> bool {
        matches!(self, TNode::Hash(pairs) if pairs.iter().any(|(k, _)| k == key))
    }

    /// First key of a hash node (dispatch on single-key captures).
    pub fn single_key(&self) -> Option<&str> {
        match self {
            TNode::Hash(pairs) if pairs.len() == 1 => Some(pairs[0].0.as_str()),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            TNode::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[TNode]> {
        match self {
            TNode::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Ruby `Array(value)`: nil -> [], scalar -> [v], array -> itself.
    pub fn ruby_array(&self) -> &[TNode] {
        match self {
            TNode::Array(items) => items,
            TNode::Nil => &[],
            other => std::slice::from_ref(other),
        }
    }
}

/// Resolve the portable engine's arena-backed AST into owned `TNode`s.
pub fn resolve(node: &AstNode, arena: &AstArena, input: &str) -> TNode {
    match node {
        AstNode::Nil => TNode::Nil,
        AstNode::InputRef { offset, length } => {
            let start = *offset as usize;
            let end = start + *length as usize;
            TNode::Str(input.get(start..end).unwrap_or_default().to_string())
        }
        AstNode::StringRef { pool_index } => {
            TNode::Str(arena.get_string(*pool_index as usize).to_string())
        }
        AstNode::Array { pool_index, length } => TNode::Array(
            arena
                .get_array(*pool_index as usize, *length as usize)
                .iter()
                .map(|child| resolve(child, arena, input))
                .collect(),
        ),
        AstNode::Hash { pool_index, length } => {
            let pairs = arena.get_hash_items(*pool_index as usize, *length as usize);
            TNode::Hash(
                pairs
                    .into_iter()
                    .map(|(k, v)| (k, resolve(&v, arena, input)))
                    .collect(),
            )
        }
        other => TNode::Str(format!("{other:?}")),
    }
}

/// Collect leaf text under a node, concatenating string leaves in
/// order (used for `run` capture values and raw repetitions).
pub fn leaf_text(node: &TNode) -> String {
    match node {
        TNode::Str(s) => s.clone(),
        TNode::Array(items) => items.iter().map(leaf_text).collect(),
        TNode::Hash(pairs) => pairs.iter().map(|(_, v)| leaf_text(v)).collect(),
        TNode::Nil => String::new(),
    }
}
