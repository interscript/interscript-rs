//! `Interscript::Node::*#reverse` parity for the slice-1 subset:
//! reversed documents, stages, sub rules (with boundary-alias
//! transfer + capture/ref exchange), funcall inversion, and the
//! `reverse_name` convention.
//!
//! Ground truth is `interscript-ruby` origin/main
//! (`lib/interscript/node/*.rb`); every rule here mirrors the Ruby
//! method named in its comment. The reversed document runs through
//! the NORMAL executor — the interpreter skips rules whose
//! `reverse_run == Some(true)`, so the inversion lives entirely in
//! the produced document. Capture numbering is positional: refs stay
//! untouched numbers and groups re-number in regex order over the
//! reversed rule (`assign_group_ids`), reproducing Ruby's
//! `build_regexp` numbering exactly.
//!
//! Known deviations (all documented at the site): where the Ruby
//! reverse would crash (NoMethodError on repeat items in case-swap
//! subs, `children=` on a compacted-away from group, an `upcase`
//! funcall missing from `reverse_function`), this implementation
//! raises a clean `Unsupported` carrying the same information.

use crate::builder::assign_group_ids;
use crate::document::{Document, Stage, StageItem, Sub, SubTo, Tests};
use crate::error::{Error, Result};
use crate::item::Item;
use crate::stdlib::{Funcall, Prim};
use std::collections::BTreeMap;
use std::rc::Rc;

// ----------------------------------------------------------------------
// Document / stage / rule level
// ----------------------------------------------------------------------

/// `Interscript::Node::Document.reverse_name`: split on `-`, swap
/// positions 2/3 (source/target code) when there are at least 4
/// segments; otherwise strip a `-reverse` suffix, otherwise append one.
pub fn reverse_name(name: &str) -> String {
    let mut parts: Vec<&str> = name.split('-').collect();
    if parts.len() >= 4 {
        parts.swap(2, 3);
        let swapped = parts.join("-");
        if swapped != name {
            return swapped;
        }
    }
    let stripped = name.replace("-reverse", "");
    if stripped != name {
        stripped
    } else {
        format!("{name}-reverse")
    }
}

/// `Interscript::Node::Document#reverse`. Aliases are NOT reversed
/// (`rdoc.aliases = self.aliases`); tests swap their pairs; dependency
/// documents reverse recursively.
pub fn reverse_document(doc: &Document) -> Result<Document> {
    let mut stages = BTreeMap::new();
    for (name, stage) in &doc.stages {
        stages.insert(name.clone(), reverse_stage(stage)?);
    }
    // ISC-WF-1 W1: rulesets reverse like stages (they only matter if a
    // dependent map splices them after reversing the dependency).
    let mut rulesets = BTreeMap::new();
    for (name, stage) in &doc.rulesets {
        rulesets.insert(name.clone(), reverse_stage(stage)?);
    }
    let mut dep_aliases = BTreeMap::new();
    for (alias, dep) in &doc.dep_aliases {
        dep_aliases.insert(alias.clone(), Rc::new(reverse_document(dep)?));
    }
    let mut imports = Vec::with_capacity(doc.imports.len());
    for dep in &doc.imports {
        imports.push(Rc::new(reverse_document(dep)?));
    }
    Ok(Document {
        name: reverse_name(&doc.name),
        system_code: doc.system_code.clone(),
        aliases: doc.aliases.clone(),
        stages,
        rulesets,
        dep_aliases,
        imports,
        // Tests#reverse: swap (from, to) per row (the ISC model has no
        // per-row reverse_run flag).
        tests: Tests {
            data: doc
                .tests
                .data
                .iter()
                .map(|(a, b)| (b.clone(), a.clone()))
                .collect(),
        },
        // MetaData#reverse swaps source_script/destination_script.
        metadata: reverse_metadata(&doc.metadata),
        // ISC-WF-1 W3: language declarations are surface-independent.
        languages: doc.languages.clone(),
    })
}

fn reverse_metadata(metadata: &[(String, String)]) -> Vec<(String, String)> {
    let src = metadata
        .iter()
        .find(|(k, _)| k == "source_script")
        .map(|(_, v)| v.clone());
    let dst = metadata
        .iter()
        .find(|(k, _)| k == "destination_script")
        .map(|(_, v)| v.clone());
    let mut out = metadata.to_vec();
    for (k, v) in out.iter_mut() {
        match k.as_str() {
            "source_script" => {
                if let Some(d) = &dst {
                    *v = d.clone();
                }
            }
            "destination_script" => {
                if let Some(s) = &src {
                    *v = s.clone();
                }
            }
            _ => {}
        }
    }
    out
}

/// `Interscript::Node::Stage#reverse` / `Group#reverse`: children in
/// REVERSE order, each reversed, reverse_run toggled. (The ISC grammar
/// cannot express `dont_reverse`, so stages always reverse.)
pub fn reverse_stage(stage: &Stage) -> Result<Stage> {
    let mut items: Vec<StageItem> = Vec::with_capacity(stage.items.len());
    for item in stage.items.iter().rev() {
        items.push(reverse_stage_item(item)?);
    }
    Ok(Stage {
        name: stage.name.clone(),
        items,
        reverse_run: toggle(stage.reverse_run),
    })
}

fn toggle(run: Option<bool>) -> Option<bool> {
    run.map(|b| !b)
}

fn reverse_stage_item(item: &StageItem) -> Result<StageItem> {
    Ok(match item {
        StageItem::Parallel(rules) => {
            let mut out = Vec::with_capacity(rules.len());
            for rule in rules.iter().rev() {
                out.push(reverse_sub(rule)?);
            }
            StageItem::Parallel(out)
        }
        StageItem::Sequential(rules) => {
            let mut out = Vec::with_capacity(rules.len());
            for rule in rules.iter().rev() {
                out.push(reverse_sub(rule)?);
            }
            StageItem::Sequential(out)
        }
        StageItem::Run {
            dep,
            stage,
            reverse_run,
        } => StageItem::Run {
            dep: dep.clone(),
            stage: stage.clone(),
            reverse_run: toggle(*reverse_run),
        },
        StageItem::Funcall(funcall) => StageItem::Funcall(reverse_funcall(funcall)?),
    })
}

/// `Interscript::Node::Rule::Funcall#reverse`: the name maps through
/// `Stdlib.reverse_function`; kwargs carry over.
fn reverse_funcall(funcall: &Funcall) -> Result<Funcall> {
    let name = funcall.name.reverse_name().ok_or_else(|| {
        Error::Unsupported(
            "funcall upcase has no reverse (the Ruby reverse_function map omits it)".into(),
        )
    })?;
    Ok(Funcall {
        name,
        kwargs: funcall.kwargs.clone(),
        reverse_run: toggle(funcall.reverse_run),
    })
}

/// `Interscript::Node::Rule::Sub#reverse`.
pub fn reverse_sub(sub: &Sub) -> Result<Sub> {
    let (xfrom, xto) = match &sub.to {
        // `sub from, to upcase` reverses to `sub from.downcase, to downcase`
        // (Item#downcase / Item#upcase parity, and vice versa).
        SubTo::Upcase => (item_downcase(&sub.from)?, SubTo::Downcase),
        SubTo::Downcase => (item_upcase(&sub.from)?, SubTo::Upcase),
        SubTo::Item(to) => {
            let (f, t) = reverse_transfer(sub.from.clone(), to.clone())?;
            (f, SubTo::Item(t))
        }
    };

    // reverse_run toggles; a rule whose reversed `from` is empty (and
    // carries no assertions) would match EVERYWHERE — the Ruby
    // Sub#reverse marks those reverse_run: true so the executor skips
    // them.
    let mut rrun = toggle(sub.reverse_run);
    if rrun.is_none()
        && sub.before.is_none()
        && sub.not_before.is_none()
        && sub.after.is_none()
        && sub.not_after.is_none()
        && is_empty_from(&xfrom)
    {
        rrun = Some(true);
    }

    let mut reversed = Sub {
        // Constraints pass through UNCHANGED (Ruby keeps
        // before/after/not_before/not_after as-is).
        before: sub.before.clone(),
        not_before: sub.not_before.clone(),
        after: sub.after.clone(),
        not_after: sub.not_after.clone(),
        from: xfrom,
        to: xto,
        group_count: 0,
        reverse_run: rrun,
        // ISC-WF-1 W3: guards carry over unchanged — the language
        // selection filters the same rows on either surface.
        when: sub.when.clone(),
    };
    // Regexp-order group numbering over the reversed rule — the same
    // walk `build_regexp` performs in Ruby. Refs keep their numbers.
    reversed.group_count = assign_group_ids(&mut reversed);
    Ok(reversed)
}

/// The Ruby empty-from test: `xfrom == "" || String("") || Alias(:none)`.
fn is_empty_from(item: &Item) -> bool {
    match item {
        Item::None | Item::Prim(Prim::None_) => true,
        Item::Str(s) => s.is_empty(),
        _ => false,
    }
}

// ----------------------------------------------------------------------
// Item#downcase / Item#upcase (Sub#reverse to upcase/downcase case)
// ----------------------------------------------------------------------

/// `Item#downcase`: String data downcases; Group/CaptureGroup
/// recurse; Alias (incl. boundary prims) and refs/ranges are
/// identity; Any rebuilds from per-char entries.
pub fn item_downcase(item: &Item) -> Result<Item> {
    item_case(item, false)
}

/// `Item#upcase`.
pub fn item_upcase(item: &Item) -> Result<Item> {
    item_case(item, true)
}

fn item_case(item: &Item, up: bool) -> Result<Item> {
    let apply = |s: &str| {
        if up {
            s.to_uppercase()
        } else {
            s.to_lowercase()
        }
    };
    Ok(match item {
        Item::Str(s) => Item::Str(apply(s)),
        Item::None => Item::None,
        // Item::Alias#downcase/#upcase = self.
        Item::Prim(_) | Item::Alias { .. } | Item::CaptureRef(_) => item.clone(),
        Item::CaptureGroup { id, inner } => Item::CaptureGroup {
            id: *id,
            inner: Box::new(item_case(inner, up)?),
        },
        Item::Concat(parts) => Item::Concat(
            parts
                .iter()
                .map(|p| item_case(p, up))
                .collect::<Result<Vec<_>>>()?,
        ),
        // Any.new(data.map(&:downcase)): a Range rebuilds as the set
        // of its per-char entries (Any with an Array value).
        Item::Range(lo, hi) => {
            let mut entries = Vec::new();
            let mut c = *lo;
            loop {
                entries.push(Item::Str(apply(&c.to_string())));
                if c == *hi {
                    break;
                }
                match char::from_u32(c as u32 + 1) {
                    Some(n) => c = n,
                    None => break,
                }
            }
            Item::Set(entries)
        }
        Item::Set(entries) => Item::Set(
            entries
                .iter()
                .map(|e| item_case(e, up))
                .collect::<Result<Vec<_>>>()?,
        ),
        // Ruby Item::Repeat has no downcase/upcase: NoMethodError.
        Item::Maybe(_) | Item::Some(_) => {
            return Err(Error::Unsupported(
                "repeat item in a to-upcase/downcase sub (the Ruby reverse raises NoMethodError)"
                    .into(),
            ))
        }
        // Function items are a slice-1 SubTo marker, never a from tree.
        Item::Func(_) => {
            return Err(Error::Unsupported(
                "function item in a sub `from` (the Ruby adapter rejects it)".into(),
            ))
        }
    })
}

// ----------------------------------------------------------------------
// Sub#reverse_transfer: boundary hoisting + capture/ref exchange
// ----------------------------------------------------------------------

/// `Interscript::Stdlib.boundary_like_alias?` (the slice-1 subset:
/// string_start/string_end do not exist in the ISC grammar).
fn boundary_like(item: &Item) -> bool {
    matches!(
        item,
        Item::Prim(Prim::Boundary | Prim::NonWordBoundary | Prim::LineStart | Prim::LineEnd)
    )
}

/// Ruby `Group.new + a + b ...`: a fresh group whose children are the
/// leading items plus `last` — flattened when `last` is itself a
/// Group (`Group#+` merges children) — then `Group#compact` (drop
/// none/"" children; 0 -> Alias(:none), 1 -> the child).
fn group_plus_compact(prefix: Vec<Item>, last: Item) -> Item {
    let mut children = prefix;
    match last {
        Item::Concat(parts) => children.extend(parts),
        other => children.push(other),
    }
    let kept: Vec<Item> = children
        .into_iter()
        .filter(|c| match c {
            Item::None | Item::Prim(Prim::None_) => false,
            Item::Str(s) => !s.is_empty(),
            _ => true,
        })
        .collect();
    match kept.len() {
        0 => Item::None,
        1 => kept.into_iter().next().unwrap(),
        _ => Item::Concat(kept),
    }
}

/// `Interscript::Node::Rule::Sub#reverse_transfer`: boundary-like
/// aliases at the from edges move onto the to side; then the
/// capture/ref exchange runs. Returns (reversed_from, reversed_to) —
/// the Ruby multi-assign `xto, xfrom = reverse_transfer(from, to)`
/// swaps the results.
fn reverse_transfer(from: Item, to: Item) -> Result<(Item, Item)> {
    let mut from = from;
    let mut to = to;

    match &from {
        Item::Concat(parts) => {
            // Ruby reads first/last BEFORE either mutation.
            let first = parts.first().cloned();
            let last = parts.last().cloned();

            // first child boundary-like -> prepended to `to`
            if let Some(f) = &first {
                if boundary_like(f) {
                    to = group_plus_compact(vec![f.clone()], to);
                    let mut rest = parts.clone();
                    rest.remove(0);
                    from = compact_children(rest);
                }
            }
            // last child boundary-like -> appended to `to`. Ruby
            // mutates `from.children` here, which requires `from` to
            // still be a Group; a first-hoist that consumed the whole
            // group crashes there (NoMethodError).
            if let Some(l) = &last {
                if boundary_like(l) {
                    match &from {
                        Item::Concat(prev) => {
                            let mut shorter = prev.clone();
                            shorter.pop();
                            to = group_plus_compact(vec![to], l.clone());
                            from = compact_children(shorter);
                        }
                        _ => {
                            return Err(Error::Unsupported(
                                "boundary hoist consumed the whole from group \
                                 (the Ruby reverse raises NoMethodError)"
                                    .into(),
                            ))
                        }
                    }
                }
            }
        }
        single => {
            // Whole-from boundary alias: `_end`-suffixed names append
            // after `to`, everything else prepends before it; from
            // becomes `none`.
            if boundary_like(single) {
                // Ruby: from.name.end_with?("_end") — among the ISC
                // prims only line_end qualifies (string_end is not in
                // the grammar).
                to = if matches!(single, Item::Prim(Prim::LineEnd)) {
                    group_plus_compact(vec![to], single.clone())
                } else {
                    group_plus_compact(vec![single.clone()], to)
                };
                from = Item::Prim(Prim::None_);
            }
        }
    }

    // --- capture/ref exchange ---
    let mut state = TransferState::default();
    let visited_from = transfer_visit(&from, Side::From, &mut state)?;
    let visited_to = transfer_visit(&to, Side::To, &mut state)?;
    // xto, xfrom = reverse_transfer(from, to)
    Ok((visited_to, visited_from))
}

/// `Group#compact` on an already-collected children list.
fn compact_children(children: Vec<Item>) -> Item {
    let kept: Vec<Item> = children
        .into_iter()
        .filter(|c| match c {
            Item::None | Item::Prim(Prim::None_) => false,
            Item::Str(s) => !s.is_empty(),
            _ => true,
        })
        .collect();
    match kept.len() {
        0 => Item::None,
        1 => kept.into_iter().next().unwrap(),
        _ => Item::Concat(kept),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    From,
    To,
}

#[derive(Default)]
struct TransferState {
    /// Ruby state[:left]: every CaptureGroup / kor_maybedash seen, in
    /// visit order; minted ref ids index into it (1-based).
    #[allow(dead_code)]
    left: Vec<Item>,
    /// Ruby state[:right]: ref ids already substituted on the `to`
    /// side (a second occurrence stays a literal ref).
    right: std::collections::BTreeSet<usize>,
}

/// `reverse_transfer_visit`. Refs stay positional numbers — the
/// renumber pass on the reversed rule (`assign_group_ids`) produces
/// the same numbers Ruby's `build_regexp` assigns at build time.
fn transfer_visit(node: &Item, side: Side, state: &mut TransferState) -> Result<Item> {
    Ok(match node {
        // Alias :kor_maybedash -> CaptureRef(left.length). (Our builder
        // expands the alias to its capture(maybe("-")) tree before
        // reverse sees it, where the CaptureGroup branch handles it
        // with identical numbering — this branch guards direct alias
        // items and documents the Ruby special case.)
        Item::Alias { name, .. } if name == "kor_maybedash" => {
            state.left.push(node.clone());
            Item::CaptureRef(state.left.len())
        }
        Item::Str(_) | Item::None | Item::Prim(_) | Item::Alias { .. } | Item::Range(_, _) => {
            node.clone()
        }
        Item::Set(entries) => Item::Set(
            entries
                .iter()
                .map(|e| transfer_visit(e, side, state))
                .collect::<Result<Vec<_>>>()?,
        ),
        Item::Concat(parts) => Item::Concat(
            parts
                .iter()
                .map(|p| transfer_visit(p, side, state))
                .collect::<Result<Vec<_>>>()?,
        ),
        // Repeat: only the data is visited; the repeat kind survives.
        Item::Maybe(inner) => Item::Maybe(Box::new(transfer_visit(inner, side, state)?)),
        Item::Some(inner) => Item::Some(Box::new(transfer_visit(inner, side, state)?)),
        Item::CaptureRef(id) => match side {
            // refs on the from side stay refs
            Side::From => node.clone(),
            Side::To => {
                if state.right.contains(id) {
                    node.clone()
                } else {
                    state.right.insert(*id);
                    match state.left.get(id.wrapping_sub(1)) {
                        Some(group) => group.clone(),
                        None => {
                            return Err(Error::MapLogic(
                                "Capture count doesn't match".into(),
                            ))
                        }
                    }
                }
            }
        },
        Item::CaptureGroup { inner, .. } => {
            state.left.push(node.clone());
            let out = Item::CaptureRef(state.left.len());
            // Visit but don't care (inner groups still register).
            transfer_visit(inner, side, state)?;
            out
        }
        Item::Func(_) => {
            return Err(Error::Unsupported(
                "Type Item::Func unhandled in reverse_transfer_visit (the Ruby raises \
                 \"Type ... unhandled!\")"
                    .into(),
            ))
        }
    })
}
