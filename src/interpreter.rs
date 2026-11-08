//! The interpreter: `Interscript::Interpreter::Stage` parity. A
//! backtracking item matcher (the Ruby runtime's regex semantics
//! without compiling to regex — Rust's regex crate has no lookaround,
//! which the constraint semantics require) plus the stage executor
//! over parallel groups, sequential sub rules, run rules and funcalls.
//!
//! Semantics contract (from interpreter.rb / stdlib.rb):
//! * `before`     -> lookbehind  (?<=…)
//! * `not_before` -> (?<!…)
//! * `after`      -> lookahead   (?=…)
//! * `not_after`  -> (?!…)
//! * an EMPTY constraint item behaves like the empty regex in that
//!   position: positive constraints become no-ops, negative ones
//!   disable the rule (Ruby `(?<=)` always true / `(?<!)` never true).
//! * parallel groups: single left-to-right pass. Ruby's two paths are
//!   both mirrored (they disagree on duplicate `from` keys): the
//!   tree path (`parallel_replace_tree`, constraint-free groups) is a
//!   last-wins char trie with longest-match scanning; the megaregexp
//!   fallback tries rules in `deterministic_sort_by_max_length`
//!   order (-max_length*100000+idx), first match wins.
//! * replacement `to` items collapse through `nth_string`
//!   (Any -> first string) before building, exactly like build_item.

use crate::document::{Document, StageItem, Sub, SubTo};
use crate::error::{Error, Result};
use crate::item::Item;
use crate::stdlib::Prim;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;

/// A loaded-map cache bound to a load path. `Interscript.transliterate`
/// parity: `transliterate(map_id, string)` runs stage `main`.
pub struct Runtime {
    load_path: Vec<PathBuf>,
    cache: RefCell<BTreeMap<String, Rc<Document>>>,
    /// Dependency-cycle guard.
    loading: RefCell<Vec<String>>,
}

impl Runtime {
    pub fn new(load_path: Vec<PathBuf>) -> Runtime {
        Runtime {
            load_path,
            cache: RefCell::new(BTreeMap::new()),
            loading: RefCell::new(Vec::new()),
        }
    }

    /// Load (and dependency-close) a map document.
    pub fn load(&self, map_id: &str) -> Result<Rc<Document>> {
        if let Some(doc) = self.cache.borrow().get(map_id) {
            return Ok(doc.clone());
        }
        {
            let loading = self.loading.borrow();
            if loading.iter().any(|m| m == map_id) {
                let mut chain = loading.clone();
                chain.push(map_id.to_string());
                return Err(Error::DependencyCycle(chain));
            }
        }
        self.loading.borrow_mut().push(map_id.to_string());
        let result = self.load_uncached(map_id);
        self.loading.borrow_mut().pop();
        result
    }

    fn load_uncached(&self, map_id: &str) -> Result<Rc<Document>> {
        let path = crate::locate::locate(&self.load_path, map_id)?;
        let source = crate::locate::read_map(&path)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(map_id)
            .to_string();
        let tree = crate::grammar::parse_isc(&source).map_err(|(offset, expected)| Error::Parse {
            map: stem.clone(),
            offset,
            expected,
        })?;

        // Dependency documents load FIRST (NodeAdapter ordering).
        let targets = crate::builder::dependency_targets(&tree);
        let mut imports: Vec<Rc<Document>> = Vec::new();
        let mut dep_docs: BTreeMap<String, Rc<Document>> = BTreeMap::new();
        for (target, alias) in targets {
            let dep = self.load(&target)?;
            match alias {
                Some(name) => {
                    dep_docs.insert(name, dep);
                }
                None => imports.push(dep),
            }
        }

        let doc = Rc::new(crate::builder::build(&tree, &stem, &imports, &dep_docs)?);
        self.cache
            .borrow_mut()
            .insert(map_id.to_string(), doc.clone());
        Ok(doc)
    }

    /// `Interscript.transliterate(map_id, string)`.
    pub fn transliterate(&self, map_id: &str, input: &str) -> Result<String> {
        self.transliterate_stage(map_id, "main", input, None)
    }

    /// ISC-WF-1 W3: transliterate with a language selection (§5.1).
    /// Selecting a language the map does not declare behaves like no
    /// selection (every `when` guard inert).
    pub fn transliterate_lang(
        &self,
        map_id: &str,
        input: &str,
        language: Option<&str>,
    ) -> Result<String> {
        self.transliterate_stage(map_id, "main", input, language)
    }

    pub fn transliterate_stage(
        &self,
        map_id: &str,
        stage: &str,
        input: &str,
        language: Option<&str>,
    ) -> Result<String> {
        let doc = self.load(map_id)?;
        execute_stage(&doc, stage, input, language)
    }

    /// Run a document's embedded tests; returns (passed, failed, failures).
    /// Embedded tests run WITHOUT a language selection (§5.1 default).
    pub fn run_tests(&self, map_id: &str) -> Result<(usize, usize, Vec<String>)> {
        let doc = self.load(map_id)?;
        let mut passed = 0usize;
        let mut failed = 0usize;
        let mut failures = Vec::new();
        for (input, expected) in &doc.tests.data {
            match self.transliterate(map_id, input) {
                Ok(actual) if &actual == expected => passed += 1,
                Ok(actual) => {
                    failed += 1;
                    failures.push(format!("{input:?} -> {actual:?} (expected {expected:?})"));
                }
                Err(e) => {
                    failed += 1;
                    failures.push(format!("{input:?} -> ERROR {e}"));
                }
            }
        }
        Ok((passed, failed, failures))
    }
}

// ----------------------------------------------------------------------
// Stage execution
// ----------------------------------------------------------------------

/// Public alias for diagnostics and tests.
pub fn execute_stage_pub(
    doc: &Document,
    stage_name: &str,
    input: &str,
    language: Option<&str>,
) -> Result<String> {
    execute_stage(doc, stage_name, input, language)
}

/// W3 (§5.2) pass filtering: the applicable rule set = unguarded rules
/// ∪ guarded rules whose `when` matches the selected language — and a
/// guard-applicable rule BEATS an unguarded rule with the same from
/// (a language-specific value overrides the generic value without
/// restating the generic row). With no selection every guard is inert,
/// so this reduces to the unguarded set (§5.1).
fn guard_filter<'a>(rules: &[&'a Sub], language: Option<&str>) -> Vec<&'a Sub> {
    let applicable: Vec<&Sub> = rules
        .iter()
        .copied()
        .filter(|r| r.guard_applies(language))
        .collect();
    let guarded_froms: BTreeSet<String> = applicable
        .iter()
        .filter(|r| !r.when.is_empty())
        .map(|r| r.from_key())
        .collect();
    if guarded_froms.is_empty() {
        return applicable;
    }
    // Guarded rows always stay; an UNGUARDED row drops when an
    // applicable guarded row shares its from (§5.2 precedence).
    applicable
        .into_iter()
        .filter(|r| !r.when.is_empty() || !guarded_froms.contains(&r.from_key()))
        .collect()
}

fn execute_stage(
    doc: &Document,
    stage_name: &str,
    input: &str,
    language: Option<&str>,
) -> Result<String> {
    let stage = doc
        .imported_stages()
        .remove(stage_name)
        .ok_or_else(|| Error::MapLogic(format!("stage {stage_name:?} not found in {}", doc.name)))?;
    // Ruby execute_rule: `return if r.reverse_run == true` — a rule or
    // stage carrying reverse_run == true is skipped entirely.
    if stage.reverse_run == Some(true) {
        return Ok(input.to_string());
    }
    let mut s = input.to_string();
    for item in &stage.items {
        // Ruby execute_rule top: reverse_run == true skips the rule.
        let skipped = match item {
            StageItem::Funcall(f) => f.reverse_run == Some(true),
            StageItem::Run { reverse_run, .. } => *reverse_run == Some(true),
            _ => false,
        };
        if skipped {
            continue;
        }
        s = match item {
            StageItem::Parallel(rules) => apply_parallel(doc, rules, &s, language)?,
            StageItem::Sequential(rules) => {
                let mut out = s;
                for rule in rules {
                    if rule.reverse_run == Some(true) {
                        continue;
                    }
                    // W3 §5.2: a guarded rule whose guard is inert is
                    // skipped entirely (behaves as if absent).
                    if !rule.guard_applies(language) {
                        continue;
                    }
                    out = apply_sub(doc, rule, &out)?;
                }
                out
            }
            StageItem::Run { dep, stage, .. } => match dep {
                Some(alias) => {
                    let dep_doc = doc.dep_aliases.get(alias).ok_or_else(|| {
                        Error::MapLogic(format!("dep alias {alias:?} not found in {}", doc.name))
                    })?;
                    // W3 §5.3: the selection threads into dependency
                    // stage runs (each document's own declaration
                    // gates its guards).
                    execute_stage(dep_doc, stage, &s, language)?
                }
                None => execute_stage(doc, stage, &s, language)?,
            },
            StageItem::Funcall(funcall) => funcall.name.apply(s, &funcall.kwargs)?,
        };
    }
    Ok(s)
}

/// Single left-to-right simultaneous pass. Ruby has TWO paths and
/// they disagree on duplicate `from` keys, so both are mirrored:
///
/// * tree path (`Stdlib.parallel_replace_tree`), used when every rule
///   is constraint-free and builds to plain from-strings: a
///   last-wins char-trie, longest match at each position;
/// * megaregexp path (the rescue fallback): rules tried in
///   `deterministic_sort_by_max_length` order (-max_length*100000+idx),
///   first match wins.
fn apply_parallel(doc: &Document, rules: &[Sub], s: &str, language: Option<&str>) -> Result<String> {
    // Ruby: `next if i.reverse_run == true` in both the tree and the
    // megaregexp branch — skipped rules join neither array.
    let live: Vec<&Sub> = rules
        .iter()
        .filter(|r| r.reverse_run != Some(true))
        .collect();
    // W3 §5.2: the pass runs over the language-applicable set, with
    // guard-applicable beating unguarded same-from.
    let applicable = guard_filter(&live, language);
    if let Some(tree) = build_tree(&applicable, doc)? {
        return tree_replace(&tree, s);
    }

    let chars: Vec<char> = s.chars().collect();
    // deterministic_sort_by_max_length: -max_length * 100000 + idx.
    let mut order: Vec<usize> = (0..applicable.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(applicable[i].max_length()), i));

    let mut out = String::with_capacity(s.len());
    let mut copied = 0usize; // chars[..copied] are already emitted
    let mut pos = 0usize;
    while pos < chars.len() {
        let mut matched: Option<(usize, String)> = None;
        for &i in &order {
            let rule = applicable[i];
            let m = Matcher::new(&chars, doc);
            if let Some((end, replacement)) = m.match_rule(rule, pos)? {
                matched = Some((end, replacement));
                break;
            }
        }
        match matched {
            Some((end, replacement)) => {
                out.extend(&chars[copied..pos]);
                out.push_str(&replacement);
                copied = end;
                // Ruby gsub: a zero-width match cannot loop — advance
                // at least one char.
                pos = std::cmp::max(end, pos + 1);
            }
            None => pos += 1,
        }
    }
    out.extend(&chars[copied..]);
    Ok(out)
}

/// A char-trie node with an optional terminal replacement.
type Trie = std::collections::BTreeMap<char, TrieNode>;
#[derive(Default)]
struct TrieNode {
    children: Trie,
    replacement: Option<String>,
}

/// `Interscript::Stdlib.parallel_replace_compile_tree` parity: build
/// the trie when the group is tree-eligible (no constraints; every
/// from builds via build_item(:par) without raising; every to builds
/// via build_item(:parstr)). Duplicate from-strings: later rules
/// overwrite earlier terminals (hash assignment semantics).
fn build_tree(rules: &[&Sub], doc: &Document) -> Result<Option<TrieNode>> {
    let m = Matcher::new(&[], doc);
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for rule in rules {
        if rule.before.is_some()
            || rule.not_before.is_some()
            || rule.after.is_some()
            || rule.not_after.is_some()
        {
            return Ok(None);
        }
        let to = match &rule.to {
            SubTo::Item(item) => match m.build_par_str(item) {
                Ok(s) => s,
                Err(_) => return Ok(None),
            },
            // to upcase/downcase inside a parallel group has no
            // build_item case (nil replacement) — Ruby silently
            // skips nothing here; the tree hash gets nil, matching
            // `Array(nil)` → no keys. Treat as skip.
            SubTo::Upcase | SubTo::Downcase => String::new(),
        };
        let _ = to;
        match m.build_par_from(&rule.from) {
            Ok(keys) => {
                for key in keys {
                    map.insert(key, to.clone());
                }
            }
            Err(_) => return Ok(None),
        }
    }
    let mut root = TrieNode::default();
    for (from, to) in map {
        let chars: Vec<char> = from.chars().collect();
        if chars.is_empty() {
            // The Ruby trie cannot represent an empty key (the scan
            // only follows char edges), so it is unreachable there.
            continue;
        }
        let mut node = &mut root;
        for c in chars {
            node = node.children.entry(c).or_default();
        }
        node.replacement = Some(to);
    }
    Ok(Some(root))
}

/// `parallel_replace_tree` parity: greedy longest-match scan.
fn tree_replace(root: &TrieNode, s: &str) -> Result<String> {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < len {
        let mut branch = &root.children;
        let mut matched: Option<(usize, &String)> = None;
        let mut j = 0usize;
        while j < len - i {
            let cc = chars[i + j];
            match branch.get(&cc) {
                Some(node) => {
                    branch = &node.children;
                    if let Some(repl) = &node.replacement {
                        matched = Some((j + 1, repl));
                    }
                    j += 1;
                }
                None => break,
            }
        }
        match matched {
            Some((mlen, repl)) => {
                out.push_str(repl);
                i += mlen;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Sequential gsub pass for one sub rule.
fn apply_sub(doc: &Document, rule: &Sub, s: &str) -> Result<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut copied = 0usize; // chars[..copied] are already emitted
    let mut pos = 0usize;
    // A gsub pass attempts the pattern at every position INCLUDING the
    // end (a zero-width pattern produces one final empty match).
    while pos <= chars.len() {
        let m = Matcher::new(&chars, doc);
        match m.match_rule(rule, pos)? {
            Some((end, replacement)) => {
                out.extend(&chars[copied..pos]);
                out.push_str(&replacement);
                copied = end;
                pos = std::cmp::max(end, pos + 1);
            }
            None => pos += 1,
        }
    }
    out.extend(&chars[copied..]);
    Ok(out)
}

// ----------------------------------------------------------------------
// The matcher
// ----------------------------------------------------------------------

/// Per-rule match environment. Group slots are char-index ranges,
/// 1-based (slot 0 unused), mirroring regex group numbering.
struct Matcher<'a> {
    chars: &'a [char],
    doc: &'a Document,
    groups: RefCell<Vec<Option<(usize, usize)>>>,
    error: RefCell<Option<Error>>,
}

impl<'a> Matcher<'a> {
    fn new(chars: &'a [char], doc: &'a Document) -> Matcher<'a> {
        Matcher {
            chars,
            doc,
            groups: RefCell::new(Vec::new()),
            error: RefCell::new(None),
        }
    }

    /// Match one sub rule at `pos`: constraints + from, then build the
    /// replacement. Returns (end, replacement).
    fn match_rule(&self, rule: &Sub, pos: usize) -> Result<Option<(usize, String)>> {
        let slots = rule.group_count + 1;
        self.groups.borrow_mut().resize_with(slots, || None);

        // before -> (?<=…): some k <= pos where before ends at pos.
        if let Some(before) = &rule.before {
            if !self.lookbehind(before, pos)? {
                return Ok(None);
            }
        }
        // not_before -> (?<!…): the item must NOT match ending at pos.
        // An empty item matches the empty string at every position, so
        // `not_before ""` disables the rule — the Ruby (?<!) behavior.
        if let Some(nb) = &rule.not_before {
            if self.lookbehind(nb, pos)? {
                return Ok(None);
            }
        }

        let mut end_slot: Option<usize> = None;
        let matched = {
            let slot_ref = &mut end_slot;
            self.match_item(&rule.from, pos, &mut |m, end| {
                // not_after -> (?!…)
                if let Some(na) = &rule.not_after {
                    if m.lookahead(na, end)? {
                        return Ok(false);
                    }
                }
                // after -> (?=…)
                if let Some(a) = &rule.after {
                    if !m.lookahead(a, end)? {
                        return Ok(false);
                    }
                }
                *slot_ref = Some(end);
                Ok(true)
            })
        };

        if let Some(e) = self.error.borrow_mut().take() {
            return Err(e);
        }
        if !matched {
            return Ok(None);
        }
        let end = end_slot.unwrap_or(pos);

        let replacement = match &rule.to {
            SubTo::Upcase => self.chars[pos..end].iter().collect::<String>().to_uppercase(),
            SubTo::Downcase => self.chars[pos..end].iter().collect::<String>().to_lowercase(),
            SubTo::Item(to) => {
                let collapsed = to.first_string();
                let built = self.build_replacement(&collapsed)?;
                self.decode_replacement(&built, pos, end)
            }
        };
        Ok(Some((end, replacement)))
    }

    /// Positive lookbehind: does `item` match SOME span ending at pos?
    fn lookbehind(&self, item: &Item, pos: usize) -> Result<bool> {
        for start in (0..=pos).rev() {
            let ok = self.match_item(item, start, &mut |_m, end| Ok(end == pos));
            if let Some(e) = self.error.borrow_mut().take() {
                return Err(e);
            }
            if ok {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Lookahead: does `item` match starting at pos (any length)?
    fn lookahead(&self, item: &Item, pos: usize) -> Result<bool> {
        let ok = self.match_item(item, pos, &mut |_m, _end| Ok(true));
        if let Some(e) = self.error.borrow_mut().take() {
            return Err(e);
        }
        Ok(ok)
    }

    /// CPS backtracking matcher: match `item` at `pos`, calling `k`
    /// with each candidate end; success when any continuation accepts.
    fn match_item(
        &self,
        item: &Item,
        pos: usize,
        k: &mut dyn FnMut(&Matcher, usize) -> Result<bool>,
    ) -> bool {
        match item {
            Item::None => self.call(k, pos),
            Item::Str(s) => {
                let pat: Vec<char> = s.chars().collect();
                if pos + pat.len() <= self.chars.len()
                    && self.chars[pos..pos + pat.len()] == pat[..]
                {
                    self.call(k, pos + pat.len())
                } else {
                    false
                }
            }
            Item::Prim(prim) => match prim.matches_zero_width(self.chars, pos) {
                Some(ok) => ok && self.call(k, pos),
                None => {
                    if pos < self.chars.len() && prim.matches_char(self.chars[pos]).unwrap_or(false)
                    {
                        self.call(k, pos + 1)
                    } else {
                        false
                    }
                }
            },
            Item::Concat(parts) => self.match_seq(parts, pos, k),
            Item::Set(entries) => {
                for entry in entries {
                    if self.match_item(entry, pos, k) {
                        return true;
                    }
                }
                false
            }
            Item::Range(lo, hi) => {
                if pos < self.chars.len() && *lo <= self.chars[pos] && self.chars[pos] <= *hi {
                    self.call(k, pos + 1)
                } else {
                    false
                }
            }
            Item::Maybe(inner) => {
                let extended = self.match_item(inner, pos, k);
                if extended {
                    return true;
                }
                self.call(k, pos)
            }
            Item::Some(inner) => self.match_some(inner, pos, 1, k),
            Item::CaptureGroup { id, inner } => {
                let snapshot = self.groups.borrow().clone();
                let id = *id;
                let ok = self.match_item(inner, pos, &mut |m, end| {
                    m.groups.borrow_mut()[id] = Some((pos, end));
                    match k(m, end) {
                        Ok(true) => Ok(true),
                        failed => {
                            *m.groups.borrow_mut() = snapshot.clone();
                            failed
                        }
                    }
                });
                if !ok {
                    *self.groups.borrow_mut() = snapshot;
                }
                ok
            }
            Item::CaptureRef(n) => {
                let text = self
                    .groups
                    .borrow()
                    .get(*n)
                    .and_then(|g| g.as_ref())
                    .map(|(s, e)| self.chars[*s..*e].iter().collect::<String>());
                match text {
                    Some(t) => {
                        let pat: Vec<char> = t.chars().collect();
                        if pos + pat.len() <= self.chars.len()
                            && self.chars[pos..pos + pat.len()] == pat[..]
                        {
                            self.call(k, pos + pat.len())
                        } else {
                            false
                        }
                    }
                    // Ruby \N with an unparticipated group fails to match.
                    None => false,
                }
            }
            Item::Alias { map, name } => match self.resolve_alias(map.as_deref(), name) {
                Ok(Some(resolved)) => self.match_item(&resolved, pos, k),
                Ok(None) => false,
                Err(e) => {
                    self.record_error(e);
                    false
                }
            },
            Item::Func(_) => {
                self.record_error(Error::MapLogic("function item in match position".into()));
                false
            }
        }
    }

    fn match_seq(
        &self,
        parts: &[Item],
        pos: usize,
        k: &mut dyn FnMut(&Matcher, usize) -> Result<bool>,
    ) -> bool {
        match parts.split_first() {
            None => self.call(k, pos),
            Some((first, rest)) => {
                let rest: Vec<Item> = rest.to_vec();
                self.match_item(first, pos, &mut |m, end| Ok(m.match_seq(&rest, end, k)))
            }
        }
    }

    fn match_some(
        &self,
        inner: &Item,
        pos: usize,
        count: usize,
        k: &mut dyn FnMut(&Matcher, usize) -> Result<bool>,
    ) -> bool {
        // Greedy: extend first (one more inner match), then fall back
        // to the continuation. Zero-width inner matches stop the loop
        // (regex empty-loop guard).
        let extended = self.match_item(inner, pos, &mut |m, end| {
            if end > pos {
                Ok(m.match_some(inner, end, count + 1, k))
            } else {
                Ok(false)
            }
        });
        if extended {
            return true;
        }
        if count >= 1 {
            self.call(k, pos)
        } else {
            false
        }
    }

    fn call(&self, k: &mut dyn FnMut(&Matcher, usize) -> Result<bool>, pos: usize) -> bool {
        match k(self, pos) {
            Ok(true) => true,
            Ok(false) => false,
            Err(e) => {
                self.record_error(e);
                false
            }
        }
    }

    fn record_error(&self, e: Error) {
        let mut slot = self.error.borrow_mut();
        if slot.is_none() {
            *slot = Some(e);
        }
    }

    fn resolve_alias(&self, map: Option<&str>, name: &str) -> Result<Option<Item>> {
        // Stdlib primitives (checked after dep maps, like Ruby: the
        // `i.map` branch comes first, stdlib second, local last).
        if map.is_none() {
            if let Some(prim) = Prim::from_name(name) {
                return Ok(Some(Item::Prim(prim)));
            }
        }
        match map {
            Some(alias) => {
                let doc = self
                    .doc
                    .dep_aliases
                    .get(alias)
                    .ok_or_else(|| Error::MapLogic(format!("dep alias {alias:?} not found")))?;
                doc.lookup_alias(name).map(Some).ok_or_else(|| {
                    Error::MapLogic(format!("alias {name:?} of dep {alias:?} not found"))
                })
            }
            None => self.doc.lookup_alias(name).map(Some).ok_or_else(|| {
                Error::MapLogic(format!("alias {name:?} not found in {}", self.doc.name))
            }),
        }
    }

    /// build_item(to, :str) semantics for the collapsed replacement.
    fn build_replacement(&self, item: &Item) -> Result<String> {
        match item {
            Item::Str(s) => Ok(s.clone()),
            Item::None => Ok(String::new()),
            Item::Concat(parts) => {
                let mut out = String::new();
                for p in parts {
                    out.push_str(&self.build_replacement(p)?);
                }
                Ok(out)
            }
            Item::CaptureRef(n) => Ok(format!("\\{n}")),
            // Maybe/Some/CaptureGroup in :str build their data plus a
            // literal suffix — the Ruby replacement-string quirk.
            Item::Maybe(inner) => Ok(format!("{}?", self.build_replacement(inner)?)),
            Item::Some(inner) => Ok(format!("{}+", self.build_replacement(inner)?)),
            Item::CaptureGroup { inner, .. } => {
                Ok(format!("({})", self.build_replacement(inner)?))
            }
            Item::Prim(prim) => match prim {
                Prim::None_ => Ok(String::new()),
                Prim::Space => Ok(" ".to_string()),
                other => Err(Error::MapLogic(format!(
                    "can't use {} in a string context",
                    other.name()
                ))),
            },
            Item::Alias { map, name } => {
                if map.is_none() {
                    if let Some(prim) = Prim::from_name(name) {
                        return self.build_replacement(&Item::Prim(prim));
                    }
                }
                match self.resolve_alias(map.as_deref(), name)? {
                    Some(resolved) => self.build_replacement(&resolved),
                    None => Err(Error::MapLogic(format!("alias {name:?} not found"))),
                }
            }
            // nth_string collapse already handled Any; a residual
            // Set/Range in replacement position is the Ruby
            // SystemConversionError ("Can't use Any in a string context").
            Item::Set(_) | Item::Range(_, _) => {
                Err(Error::MapLogic("can't use Any in a string context".into()))
            }
            Item::Func(_) => Err(Error::MapLogic(
                "function item in replacement position".into(),
            )),
        }
    }

    /// Ruby gsub replacement-string decoding over the built string:
    /// \1..\9 group refs, \0 whole match, \\ literal backslash,
    /// \& whole match; unknown escapes drop the backslash.
    fn decode_replacement(&self, built: &str, start: usize, end: usize) -> String {        let chars: Vec<char> = built.chars().collect();
        let mut out = String::with_capacity(built.len());
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                let next = chars[i + 1];
                match next {
                    '0' | '1'..='9' => {
                        let n = next.to_digit(10).unwrap() as usize;
                        if n == 0 {
                            out.extend(&self.chars[start..end]);
                        } else if let Some(Some((gs, ge))) = self.groups.borrow().get(n) {
                            out.extend(&self.chars[*gs..*ge]);
                        }
                        i += 2;
                        continue;
                    }
                    '\\' => {
                        out.push('\\');
                        i += 2;
                        continue;
                    }
                    '&' => {
                        out.extend(&self.chars[start..end]);
                        i += 2;
                        continue;
                    }
                    _ => {
                        // Ruby drops the backslash on unknown escapes.
                        out.push(next);
                        i += 2;
                        continue;
                    }
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }
}

impl<'a> Matcher<'a> {
    // ------------------------------------------------------------------
    // build_item(:par) / build_item(:parstr) parity — the tree-path
    // eligibility check for parallel groups. Errors mirror the Ruby
    // SystemConversionError raises that force the megaregexp fallback.
    // ------------------------------------------------------------------

    /// build_item(item, :par): the key strings this `from` contributes
    /// to the parallel tree. `Ok(vec![])` mirrors Ruby's silent nil
    /// (function items); `Err` forces the megaregexp path.
    fn build_par_from(&self, item: &Item) -> Result<Vec<String>> {
        self.build_par_from_depth(item, 0)
    }

    fn build_par_from_depth(&self, item: &Item, depth: usize) -> Result<Vec<String>> {
        if depth > 64 {
            return Err(Error::MapLogic("alias expansion too deep".into()));
        }
        match item {
            Item::Str(s) => Ok(vec![s.clone()]),
            Item::None => Ok(vec![String::new()]),
            Item::Prim(prim) => match prim {
                Prim::None_ => Ok(vec![String::new()]),
                Prim::Space => Ok(vec![" ".to_string()]),
                other => Err(Error::MapLogic(format!(
                    "can't use {} in a par context",
                    other.name()
                ))),
            },
            // Any(String/Range) -> one key per char; Any(Array) ->
            // one key per entry. Nested Any entries produce nested
            // arrays in Ruby (NoMethodError on `.chars`) -> fallback.
            Item::Range(lo, hi) => {
                let mut keys = Vec::new();
                let mut c = *lo;
                loop {
                    keys.push(c.to_string());
                    if c == *hi {
                        break;
                    }
                    match char::from_u32(c as u32 + 1) {
                        Some(n) => c = n,
                        None => break,
                    }
                }
                Ok(keys)
            }
            Item::Set(entries) => {
                let mut keys = Vec::new();
                for entry in entries {
                    match entry {
                        Item::Str(s) => keys.push(s.clone()),
                        // Nested Any / Range / Alias entries become
                        // arrays in the Ruby build -> raise -> fallback.
                        _ => {
                            return Err(Error::MapLogic(
                                "can't parallelize nested any() entries".into(),
                            ))
                        }
                    }
                }
                Ok(keys)
            }
            // Group in :par is the cartesian product of its children.
            Item::Concat(parts) => {
                let mut acc: Vec<String> = vec![String::new()];
                for part in parts {
                    let next = self.build_par_from_depth(part, depth)?;
                    let mut joined = Vec::new();
                    for a in &acc {
                        for b in &next {
                            joined.push(format!("{a}{b}"));
                        }
                    }
                    acc = joined;
                }
                Ok(acc)
            }
            Item::Alias { map, name } => {
                if map.is_none() {
                    if let Some(prim) = Prim::from_name(name) {
                        return self.build_par_from_depth(&Item::Prim(prim), depth);
                    }
                }
                match self.resolve_alias(map.as_deref(), name)? {
                    Some(resolved) => self.build_par_from_depth(&resolved, depth + 1),
                    None => Ok(vec![]),
                }
            }
            // Maybe/Some/CaptureGroup/CaptureRef raise in :par.
            Item::Maybe(_) | Item::Some(_) => Err(Error::MapLogic(
                "can't use a MaybeSome in a par context".into(),
            )),
            Item::CaptureGroup { .. } => Err(Error::MapLogic(
                "can't use a CaptureGroup in a par context".into(),
            )),
            Item::CaptureRef(_) => Err(Error::MapLogic(
                "Can't use CaptureRef in parallel mode".into(),
            )),
            // Function items have no build_item case -> nil -> no keys.
            Item::Func(_) => Ok(vec![]),
        }
    }

    /// build_item(item, :parstr): nth_string collapse, then the :par
    /// string build. `to` in a parallel group must yield ONE string.
    fn build_par_str(&self, item: &Item) -> Result<String> {
        let collapsed = item.first_string();
        self.build_par_str_inner(&collapsed, 0)
    }

    fn build_par_str_inner(&self, item: &Item, depth: usize) -> Result<String> {
        if depth > 64 {
            return Err(Error::MapLogic("alias expansion too deep".into()));
        }
        match item {
            Item::Str(s) => Ok(s.clone()),
            Item::None => Ok(String::new()),
            Item::Prim(prim) => match prim {
                Prim::None_ => Ok(String::new()),
                Prim::Space => Ok(" ".to_string()),
                other => Err(Error::MapLogic(format!(
                    "can't use {} in a par context",
                    other.name()
                ))),
            },
            Item::Concat(parts) => {
                let mut out = String::new();
                for p in parts {
                    out.push_str(&self.build_par_str_inner(p, depth)?);
                }
                Ok(out)
            }
            Item::Alias { map, name } => {
                if map.is_none() {
                    if let Some(prim) = Prim::from_name(name) {
                        return self.build_par_str_inner(&Item::Prim(prim), depth);
                    }
                }
                match self.resolve_alias(map.as_deref(), name)? {
                    Some(resolved) => self.build_par_str_inner(&resolved, depth + 1),
                    None => Err(Error::MapLogic(format!("alias {name:?} not found"))),
                }
            }
            // A residual Any after the nth_string collapse yields an
            // array in Ruby (not a replacement string) -> broken map.
            Item::Set(_) | Item::Range(_, _) => Err(Error::MapLogic(
                "parallel replacement is not a single string".into(),
            )),
            Item::Maybe(_) | Item::Some(_) => Err(Error::MapLogic(
                "can't use a MaybeSome in a par context".into(),
            )),
            Item::CaptureGroup { .. } => Err(Error::MapLogic(
                "can't use a CaptureGroup in a par context".into(),
            )),
            Item::CaptureRef(_) => Err(Error::MapLogic(
                "Can't use CaptureRef in parallel mode".into(),
            )),
            Item::Func(_) => Err(Error::MapLogic(
                "function item in a par context".into(),
            )),
        }
    }
}
