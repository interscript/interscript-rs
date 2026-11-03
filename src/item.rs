//! The item model — mirrors `Interscript::Node::Item` as produced by
//! `Interscript::Isc::NodeAdapter` from a parsed map.

use crate::stdlib::Prim;

/// A `sub`-rule item tree (`from`, `to`, constraint items).
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Literal string (Item::String).
    Str(String),
    /// The `none` keyword — empty string.
    None,
    /// Zero-width / class primitive (NodeAdapter: Alias(stdlib symbol)).
    Prim(Prim),
    /// upcase/downcase function item. Only usable as a `sub` target and
    /// only as Upcase/Downcase (the Ruby adapter raises on the rest).
    Func(FuncName),
    /// Alias reference: `map` for `dep_alias.name` qualified refs.
    Alias { map: Option<String>, name: String },
    /// capture(...). `id` is the 1-based regex group number, assigned
    /// by the builder in lookaround assembly order
    /// (before, not_before, from, not_after, after).
    CaptureGroup { id: usize, inner: Box<Item> },
    /// ref(N) — backreference to the Nth capture group.
    CaptureRef(usize),
    /// maybe(...)
    Maybe(Box<Item>),
    /// some(...)
    Some(Box<Item>),
    /// any("lo".."hi") — inclusive codepoint range.
    Range(char, char),
    /// any(...) — char set / alternation list (entries may be nested
    /// Items: ranges, sets, primitives, alias refs).
    Set(Vec<Item>),
    /// Concatenation (`+` / whitespace joins). Also Item::Group.
    Concat(Vec<Item>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncName {
    Upcase,
    Downcase,
}

impl Item {
    /// `Interscript::Node::Item::Group`-style concatenation with
    /// flattening and None-elimination (`Items::Concat.from_parts`).
    pub fn concat_from_parts(parts: Vec<Item>) -> Item {
        let mut flat: Vec<Item> = Vec::new();
        for p in parts {
            match p {
                Item::Concat(inner) => flat.extend(inner),
                Item::None => {}
                other => flat.push(other),
            }
        }
        match flat.len() {
            0 => Item::None,
            1 => flat.pop().unwrap(),
            _ => Item::Concat(flat),
        }
    }

    /// `first_string` collapse (Ruby `nth_string` without the fiber
    /// selection): what this item yields as a single replacement
    /// string. Any → first entry; Range → lo. Alias/Prim/CaptureRef
    /// keep identity (resolved later, like build_item).
    pub fn first_string(&self) -> Item {
        match self {
            Item::Set(entries) => entries
                .first()
                .map(|e| e.first_string())
                .unwrap_or(Item::None),
            Item::Range(lo, _) => Item::Str(lo.to_string()),
            Item::Maybe(inner) | Item::Some(inner) => inner.first_string(),
            Item::CaptureGroup { inner, .. } => inner.first_string(),
            Item::Concat(parts) => Item::Concat(parts.iter().map(|p| p.first_string()).collect()),
            other => other.clone(),
        }
    }

    /// `max_length` for the deterministic parallel sort. Primitives
    /// count as 1 (0 for none), mirroring `Node::Item::Alias#max_length`.
    pub fn max_length(&self) -> usize {
        match self {
            Item::Str(s) => s.chars().count(),
            Item::None => 0,
            Item::Prim(_) | Item::Func(_) | Item::Alias { .. } | Item::CaptureRef(_) => 1,
            Item::CaptureGroup { inner, .. }
            | Item::Maybe(inner)
            | Item::Some(inner) => inner.max_length(),
            Item::Range(_, _) => 1,
            Item::Set(entries) => entries.iter().map(|e| e.max_length()).max().unwrap_or(0),
            Item::Concat(parts) => parts.iter().map(|p| p.max_length()).sum(),
        }
    }
}
