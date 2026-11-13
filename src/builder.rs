//! Tree → Document builder. Mirrors the interscript-ruby pipeline:
//! `Isc::Normalizer` (scalar folding) + `Isc::Transform` (item
//! materialization) + `Isc::DocumentBuilder` + `Isc::NodeAdapter`
//! (document assembly). Dependencies load BEFORE conversion so
//! class-context charset resolution sees imported aliases
//! (NodeAdapter#build ordering).
//!
//! ISC-WF-1 (this crate's workflow-extension layer): W1 named
//! rulesets and W2 `use` + `override` resolve AT LOAD TIME — the
//! parser captures the constructs, the builder expands `use` sites
//! inline (recursive, cycle = load error) and merges overrides per
//! spec §4.2, so the Document stays flat (expanded StageItems only).
//! W3 (`languages` / `when`) is runtime — see interpreter.rs.

use crate::document::{Dependency, Document, Stage, StageItem, Sub, SubTo, Tests};
use crate::error::{Error, Result};
use crate::item::{FuncName, Item};
use crate::stdlib::{Funcall, FuncallName, Prim};
use crate::tree::{leaf_text, pget, TNode};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// Dependency targets declared by a parsed tree, in order
/// (`dependency "x" [as alias]`), read before documents are loaded.
pub fn dependency_targets(tree: &TNode) -> Vec<(String, Option<String>)> {
    let system = tree.get("system").unwrap_or(tree);
    let mut out = Vec::new();
    let Some(body) = system.get("body") else { return out };
    for block in body.ruby_array() {
        if let Some(pair) = block.get("target") {
            let target = unquote(pair);
            let alias = block.get("alias").map(ident);
            out.push((target, alias));
        }
    }
    out
}

pub fn build(
    tree: &TNode,
    filename_stem: &str,
    imports: &[Rc<Document>],
    dep_docs: &BTreeMap<String, Rc<Document>>,
) -> Result<Document> {
    let system = tree.get("system").unwrap_or(tree);
    let system_code = system.get("system_code").map(unquote).unwrap_or_default();

    let body: Vec<&TNode> = match system.get("body") {
        Some(b) => b.ruby_array().iter().collect(),
        None => Vec::new(),
    };

    // Pass 1: collect raw sections in document order.
    let mut metadata = Vec::new();
    let mut alias_nodes: Vec<(String, TNode)> = Vec::new();
    let mut tests = Tests::default();
    let mut stage_nodes: Vec<(String, &TNode)> = Vec::new();
    let mut dependencies: Vec<Dependency> = Vec::new();
    // ISC-WF-1: named rulesets (raw bodies) + declared languages.
    let mut ruleset_nodes: Vec<(String, &TNode)> = Vec::new();
    let mut languages: Vec<String> = Vec::new();

    for block in &body {
        let Some(hash) = block.ruby_array_hash() else { continue };
        if let Some(fields) = pget(hash, "metadata") {
            extract_metadata(fields, &mut metadata);
        } else if let Some(arr) = pget(hash, "aliases") {
            for a in arr.ruby_array() {
                if let Some(name_node) = a.get("name") {
                    let name = ident(name_node);
                    let value = a.get("value").cloned().unwrap_or(TNode::Nil);
                    alias_nodes.push((name, value));
                }
            }
        } else if let Some(arr) = pget(hash, "tests") {
            for t in arr.ruby_array() {
                let input = t.get("input").map(unquote).unwrap_or_default();
                let expected = t.get("expected").map(unquote).unwrap_or_default();
                tests.data.push((input, expected));
            }
        } else if let (Some(name_node), Some(stage_node)) = (pget(hash, "stage_name"), pget(hash, "stage")) {
            stage_nodes.push((ident(name_node), stage_node));
        } else if let (Some(name_node), Some(rs_node)) = (pget(hash, "ruleset_name"), pget(hash, "ruleset")) {
            ruleset_nodes.push((ident(name_node), rs_node));
        } else if let Some(lang_list) = pget(hash, "language") {
            for l in lang_list.ruby_array() {
                languages.push(ident(l));
            }
        } else if let Some(t) = pget(hash, "target") {
            let target = unquote(t);
            let alias = pget(hash, "alias").map(ident);
            dependencies.push(Dependency { target, alias });
        }
    }

    // A ruleset declared twice is a load error (the py canon): both
    // first-wins and order-dependent merges silently pick a body.
    {
        let mut seen = BTreeSet::new();
        for (name, _) in &ruleset_nodes {
            if !seen.insert(name.clone()) {
                return Err(Error::MapLogic(format!(
                    "ruleset {name:?} declared more than once in {filename_stem}"
                )));
            }
        }
    }

    // Conversion context: local raws + imported alias items
    // (NodeAdapter @local_raw / @imported).
    let ctx = Ctx {
        imported: {
            let mut m = BTreeMap::new();
            for doc in imports {
                for (k, v) in &doc.aliases {
                    m.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            m
        },
        local_raw: alias_nodes.clone(),
        dep_docs,
        ruleset_raw: ruleset_nodes,
        ruleset_memo: RefCell::new(BTreeMap::new()),
        ruleset_stack: RefCell::new(Vec::new()),
    };

    // Phase A (W1): expand every declared ruleset (memoized, cycle
    // checked) BEFORE the stages, so `use` sites hit the memo. A
    // declared-but-never-used ruleset still expands — a broken or
    // cyclic definition is a load error regardless of use (§6.4).
    for (name, _) in &ctx.ruleset_raw {
        ctx.local_ruleset(name)?;
    }
    let rulesets: BTreeMap<String, Stage> = ctx.ruleset_memo.borrow().clone();

    // §5.1: languages are declared once per map; repeated declarations
    // union in declaration order.
    {
        let mut seen = BTreeSet::new();
        languages.retain(|l| seen.insert(l.clone()));
    }

    // Pass 2: convert (aliases first — stages may reference them).
    let mut aliases = BTreeMap::new();
    for (name, value) in &alias_nodes {
        aliases.insert(name.clone(), ctx.materialize(value)?);
    }

    let mut stages = BTreeMap::new();
    for (name, node) in stage_nodes {
        let stage = ctx.extract_stage(&name, node)?;
        stages.insert(stage.name.clone(), stage);
    }

    Ok(Document {
        name: filename_stem.to_string(),
        system_code,
        aliases,
        stages,
        rulesets,
        dep_aliases: dep_docs
        .iter()
        .filter_map(|(k, v)| {
            dependencies
                .iter()
                .find(|d| d.alias.as_deref() == Some(k.as_str()))
                .map(|_| (k.clone(), v.clone()))
        })
        .collect(),
        imports: imports.to_vec(),
        tests,
        metadata,
        languages,
    })
}

struct Ctx<'a> {
    /// Imported library aliases (import deps' own aliases merged).
    imported: BTreeMap<String, Item>,
    /// Local alias raw nodes.
    local_raw: Vec<(String, TNode)>,
    /// Aliased dependency documents (qualified-alias expansion).
    dep_docs: &'a BTreeMap<String, Rc<Document>>,
    /// ISC-WF-1 W1: raw local ruleset bodies, declaration order.
    ruleset_raw: Vec<(String, &'a TNode)>,
    /// W1 expansion memo: fully expanded rulesets. A ruleset currently
    /// being expanded is NOT in the memo (the stack holds it instead).
    ruleset_memo: RefCell<BTreeMap<String, Stage>>,
    /// W1 expansion stack — bare `use` chain cycle detection (§3.2:
    /// cycles are a load error).
    ruleset_stack: RefCell<Vec<String>>,
}

impl<'a> Ctx<'a> {
    // ------------------------------------------------------------------
    // Stage extraction
    // ------------------------------------------------------------------

    fn extract_stage(&self, name: &str, node: &TNode) -> Result<Stage> {
        let items = self.extract_stage_items(node)?;
        Ok(Stage { name: name.to_string(), items, reverse_run: None })
    }

    /// The stage-body item loop. A ruleset body is stage items too
    /// (§3.1), so `local_ruleset` runs this same loop over its body —
    /// which is what makes nested `use` expansion uniform.
    fn extract_stage_items(&self, node: &TNode) -> Result<Vec<StageItem>> {
        let mut items: Vec<StageItem> = Vec::new();
        for entry in node.ruby_array() {
            let Some(hash) = entry.ruby_array_hash() else { continue };
            let keys: Vec<&str> = hash.iter().map(|(k, _)| k.as_str()).collect();
            if keys.contains(&"use_map") {
                // ISC-WF-1 W1/W2: splice the expansion at this position.
                items.extend(self.resolve_use_items(hash)?);
            } else if keys.contains(&"sequence") {
                let node = pget(hash, "sequence").unwrap();
                items.push(StageItem::Sequential(self.extract_rule_list(node)?));
            } else if keys.contains(&"parallel") {
                let node = pget(hash, "parallel").unwrap();
                items.push(StageItem::Parallel(self.extract_rule_list(node)?));
            } else if keys.contains(&"separate") {
                let separator = pget(hash, "separator")
                    .map(|v| self.materialize(v))
                    .transpose()?
                    .map(|item| render_str(&item.first_string()));
                items.push(StageItem::Funcall(Funcall {
                    name: FuncallName::Separate,
                    kwargs: separator
                        .map(|s| vec![("separator".to_string(), s)])
                        .unwrap_or_default(),
                    reverse_run: None,
                }));
            } else if keys.contains(&"compose") {
                let op = leaf_text(pget(hash, "compose").unwrap());
                let name = if op == "decompose" {
                    FuncallName::Decompose
                } else {
                    FuncallName::Compose
                };
                items.push(StageItem::Funcall(Funcall { name, kwargs: vec![], reverse_run: None }));
            } else if keys.contains(&"case") {
                let op = leaf_text(pget(hash, "case").unwrap());
                let name = match op.as_str() {
                    "upcase" => FuncallName::Upcase,
                    "downcase" => FuncallName::Downcase,
                    "title_case" => FuncallName::TitleCase,
                    other => return Err(Error::MapLogic(format!("unknown case op {other:?}"))),
                };
                let kwargs = pget(hash, "case_kwargs").map(extract_kwargs).unwrap_or_default();
                items.push(StageItem::Funcall(Funcall { name, kwargs, reverse_run: None }));
            } else if keys.contains(&"funcall_name") {
                let fname = leaf_text(pget(hash, "funcall_name").unwrap());
                let kwargs = pget(hash, "funcall_kwargs").map(extract_kwargs).unwrap_or_default();
                let fname_enum = FuncallName::from_name(&fname)
                    .ok_or_else(|| crate::stdlib::unsupported_funcall(&fname))?;
                items.push(StageItem::Funcall(Funcall { name: fname_enum, kwargs, reverse_run: None }));
            } else if keys.contains(&"dep") {
                let dep = ident(pget(hash, "dep").unwrap());
                let stage = ident(pget(hash, "stage").unwrap());
                items.push(StageItem::Run {
                    dep: Some(dep),
                    stage,
                    reverse_run: None,
                });
            } else if keys.contains(&"run_stage_only") {
                let stage = pget(hash, "run_stage_only")
                    .and_then(|v| v.get("stage"))
                    .map(ident)
                    .unwrap_or_default();
                items.push(StageItem::Run { dep: None, stage, reverse_run: None });
            } else if keys.contains(&"bare_rule") {
                let rule = pget(hash, "bare_rule").unwrap();
                items.push(StageItem::Sequential(vec![self.extract_rule(rule)?]));
            }
            // comment / noop entries: skipped
        }
        Ok(items)
    }

    /// The expanded, CLONED target of one `use` directive. Qualified
    /// references (`use alias.<ruleset>`, §3.3) resolve through the
    /// alias's dependency document — whose rulesets are already fully
    /// expanded, because it was built first (dependency cycles are
    /// caught in Runtime::load). Bare references go through this map's
    /// own expansion memo.
    fn resolve_use_stage(&self, hash: &[(String, TNode)]) -> Result<Stage> {
        let use_map = pget(hash, "use_map")
            .map(ident)
            .ok_or_else(|| Error::MapLogic("use directive without a target".into()))?;
        match pget(hash, "use_ruleset") {
            Some(rs_node) => {
                let rs = ident(rs_node);
                let doc = self.dep_docs.get(&use_map).ok_or_else(|| {
                    Error::MapLogic(format!("dep alias {use_map:?} not found"))
                })?;
                doc.rulesets.get(&rs).cloned().ok_or_else(|| {
                    Error::MapLogic(format!("ruleset {rs:?} of dep {use_map:?} not found"))
                })
            }
            None => self.local_ruleset(&use_map),
        }
    }

    /// The fully expanded local ruleset `name` (§3.2): memoized; bare
    /// `use` chains walk the raw declarations under a cycle stack.
    fn local_ruleset(&self, name: &str) -> Result<Stage> {
        if let Some(built) = self.ruleset_memo.borrow().get(name) {
            return Ok(built.clone());
        }
        {
            let stack = self.ruleset_stack.borrow();
            if stack.iter().any(|n| n == name) {
                let mut chain = stack.clone();
                chain.push(name.to_string());
                return Err(Error::MapLogic(format!(
                    "use cycle: {}",
                    chain.join(" -> ")
                )));
            }
        }
        let node = self
            .ruleset_raw
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, node)| *node)
            .ok_or_else(|| Error::MapLogic(format!("ruleset {name:?} not found")))?;
        self.ruleset_stack.borrow_mut().push(name.to_string());
        let expanded = self.extract_stage_items(node);
        self.ruleset_stack.borrow_mut().pop();
        let stage = Stage {
            name: name.to_string(),
            items: expanded?,
            reverse_run: None,
        };
        self.ruleset_memo
            .borrow_mut()
            .insert(name.to_string(), stage.clone());
        Ok(stage)
    }

    /// `use` at a stage/ruleset-body position: the expansion splices
    /// in place, in document order, with the override merge applied
    /// (§4.2) when the directive carries an override block.
    fn resolve_use_items(&self, hash: &[(String, TNode)]) -> Result<Vec<StageItem>> {
        let mut stage = self.resolve_use_stage(hash)?;
        if let Some(orules) = pget(hash, "override_rules") {
            let overrides = self.extract_rule_list(orules)?;
            stage.items = merge_override_items(stage.items, overrides)?;
        }
        Ok(stage.items)
    }

    /// `use` inside a parallel/sequence block (§3.2: the expansion
    /// inherits the enclosing pass structure): the target's rule
    /// content flattens into the enclosing collection. Non-rule items
    /// (funcalls, stage runs) cannot join a pass — load error.
    fn resolve_use_rules(&self, hash: &[(String, TNode)]) -> Result<Vec<Sub>> {
        let stage = self.resolve_use_stage(hash)?;
        let mut rules = Vec::new();
        flatten_rules(&stage.items, &mut rules)?;
        if let Some(orules) = pget(hash, "override_rules") {
            let overrides = self.extract_rule_list(orules)?;
            rules = merge_override_rules(rules, overrides);
        }
        Ok(rules)
    }

    fn extract_rule_list(&self, node: &TNode) -> Result<Vec<Sub>> {
        let mut rules = Vec::new();
        for r in node.ruby_array() {
            // ISC-WF-1: `use` inside a rule collection joins the
            // enclosing pass (§3.2) — expansion flattens into this
            // list. Must precede the noop filter (a use entry has no
            // from/to and would look like a comment).
            if r.has_key("use_map") {
                let Some(hash) = r.ruby_array_hash() else { continue };
                rules.extend(self.resolve_use_rules(hash)?);
                continue;
            }
            // Noop filter: from none + to none (comment-only blocks).
            if is_none_capture(r.get("from")) && is_none_capture(r.get("to")) {
                continue;
            }
            rules.push(self.extract_rule(r)?);
        }
        Ok(rules)
    }

    fn extract_rule(&self, r: &TNode) -> Result<Sub> {
        let from = match r.get("from") {
            Some(n) => self.materialize(n)?,
            None => Item::None,
        };
        let to = match r.get("to") {
            Some(n) => self.materialize(n)?,
            None => Item::None,
        };

        let mut before = None;
        let mut not_before = None;
        let mut after = None;
        let mut not_after = None;

        if let Some(constraints) = r.get("constraints") {
            for c in constraints.ruby_array() {
                let Some(hash) = c.ruby_array_hash() else { continue };
                for (kind, item_node) in hash.iter() {
                    let target = match kind.as_str() {
                        "before" => &mut before,
                        "not_before" => &mut not_before,
                        "after" => &mut after,
                        "not_after" => &mut not_after,
                        _ => continue,
                    };
                    // Class context: charset aliases resolve to
                    // character classes (NodeAdapter ctx=:class).
                    *target = Some(self.materialize_class(item_node)?);
                }
            }
        }

        // `to`: only upcase/downcase functions become SubTo targets
        // (Sub#initialize); other functions crash the Ruby adapter.
        let to_target = match &to {
            Item::Func(FuncName::Upcase) => SubTo::Upcase,
            Item::Func(FuncName::Downcase) => SubTo::Downcase,
            item => SubTo::Item(item.clone()),
        };

        // ISC-WF-1 W3: the `when` language guards (§5) — the list of
        // languages this row applies under. Empty = unguarded.
        let mut when = Vec::new();
        if let Some(wc) = r.get("when_clause") {
            if let Some(langs) = wc.get("when_language") {
                for l in langs.ruby_array() {
                    when.push(ident(l));
                }
            }
        }

        let mut sub = Sub {
            from,
            to: to_target,
            before,
            not_before,
            after,
            not_after,
            group_count: 0,
            reverse_run: None,
            when,
        };
        // Alias expansion (Ruby's build_item resolves aliases inline,
        // so capture-group numbers follow EXPANSION order): expand
        // before/not_before/from/not_after/after in regex order, then
        // number groups on the expanded trees.
        sub.from = self.expand_aliases(&sub.from, 0)?;
        for c in [&mut sub.before, &mut sub.not_before] {
            if let Some(i) = c {
                *i = self.expand_aliases(i, 0)?;
            }
        }
        // from already expanded; do the after-side constraints.
        for c in [&mut sub.not_after, &mut sub.after] {
            if let Some(i) = c {
                *i = self.expand_aliases(i, 0)?;
            }
        }
        sub.group_count = assign_group_ids(&mut sub);
        Ok(sub)
    }

    /// Deep-clone `item` with bare/qualified alias references replaced
    /// by their resolved value trees (match-time resolve_alias
    /// semantics: dep-map aliases first, stdlib primitives kept as
    /// Prims, then imported/local aliases). Depth-limited against
    /// alias cycles.
    fn expand_aliases(&self, item: &Item, depth: usize) -> Result<Item> {
        if depth > 32 {
            return Err(Error::MapLogic(
                "alias expansion too deep (alias cycle?)".into(),
            ));
        }
        Ok(match item {
            Item::Alias { map, name } => {
                if map.is_some() {
                    // Qualified: expand via the dep document's aliases.
                    let alias = map.clone().unwrap();
                    let doc = self
                        .dep_docs
                        .get(&alias)
                        .ok_or_else(|| Error::MapLogic(format!("dep alias {alias:?} not found")))?;
                    match doc.lookup_alias(name) {
                        Some(resolved) => self.expand_aliases(&resolved, depth + 1)?,
                        None => {
                            return Err(Error::MapLogic(format!(
                                "alias {name:?} of dep {alias:?} not found"
                            )))
                        }
                    }
                } else if let Some(prim) = Prim::from_name(name) {
                    Item::Prim(prim)
                } else if let Some(data) = self.imported.get(name) {
                    self.expand_aliases(data, depth + 1)?
                } else if let Some((_, raw)) = self
                    .local_raw
                    .iter()
                    .find(|(n, _)| n.as_str() == name)
                {
                    let resolved = self.materialize(raw)?;
                    self.expand_aliases(&resolved, depth + 1)?
                } else {
                    return Err(Error::MapLogic(format!(
                        "alias {name:?} not found during expansion"
                    )));
                }
            }
            Item::CaptureGroup { id, inner } => Item::CaptureGroup {
                id: *id,
                inner: Box::new(self.expand_aliases(inner, depth)?),
            },
            Item::Maybe(inner) => Item::Maybe(Box::new(self.expand_aliases(inner, depth)?)),
            Item::Some(inner) => Item::Some(Box::new(self.expand_aliases(inner, depth)?)),
            Item::Concat(parts) => Item::Concat(
                parts
                    .iter()
                    .map(|p| self.expand_aliases(p, depth))
                    .collect::<Result<Vec<_>>>()?,
            ),
            Item::Set(entries) => Item::Set(
                entries
                    .iter()
                    .map(|p| self.expand_set_entry(p, depth))
                    .collect::<Result<Vec<_>>>()?,
            ),
            other => other.clone(),
        })
    }

    /// Set entries expand too, but a set-of-characters stays compact:
    /// entries that resolve to Sets/Strs keep their charset semantics.
    fn expand_set_entry(&self, entry: &Item, depth: usize) -> Result<Item> {
        match entry {
            Item::Alias { .. } => self.expand_aliases(entry, depth),
            Item::Concat(parts) => Ok(Item::Concat(
                parts
                    .iter()
                    .map(|p| self.expand_set_entry(p, depth))
                    .collect::<Result<Vec<_>>>()?,
            )),
            Item::Set(entries) => Ok(Item::Set(
                entries
                    .iter()
                    .map(|p| self.expand_set_entry(p, depth))
                    .collect::<Result<Vec<_>>>()?,
            )),
            Item::Maybe(inner) => Ok(Item::Maybe(Box::new(self.expand_set_entry(inner, depth)?))),
            Item::Some(inner) => Ok(Item::Some(Box::new(self.expand_set_entry(inner, depth)?))),
            other => Ok(other.clone()),
        }
    }

    // ------------------------------------------------------------------
    // Item materialization (Transform rules)
    // ------------------------------------------------------------------

    fn materialize(&self, fragment: &TNode) -> Result<Item> {
        match fragment {
            TNode::Hash(_) => {
                if fragment.has_key("concatenation") {
                    let parts = fragment.get("concatenation").unwrap();
                    let items = parts
                        .ruby_array()
                        .iter()
                        .map(|p| self.convert_fragment(p))
                        .collect::<Result<Vec<_>>>()?;
                    Ok(Item::concat_from_parts(items))
                } else {
                    let item = self.convert_fragment(fragment)?;
                    Ok(Item::concat_from_parts(vec![item]))
                }
            }
            TNode::Array(parts) => {
                let items = parts
                    .iter()
                    .map(|p| self.convert_fragment(p))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Item::concat_from_parts(items))
            }
            TNode::Nil => Ok(Item::None),
            TNode::Str(_) => Ok(Item::Str(fragment.as_str().unwrap_or_default().to_string())),
        }
    }

    /// Transform one keyed capture fragment into an Item.
    fn convert_fragment(&self, node: &TNode) -> Result<Item> {
        if let TNode::Hash(pairs) = node {
            let (key, value) = (&pairs[0].0, &pairs[0].1);
            match key.as_str() {
                "string" => return Ok(Item::Str(decode_string_value(value))),
                "none" => return Ok(Item::None),
                "any_char" => {
                    return Ok(Item::Alias {
                        map: None,
                        name: "any_character".to_string(),
                    })
                }
                "primitive" => {
                    let name = leaf_text(value);
                    return Prim::from_name(&name)
                        .map(Item::Prim)
                        .ok_or_else(|| Error::MapLogic(format!("unknown primitive {name:?}")));
                }
                "function" => {
                    let name = leaf_text(value);
                    return match name.as_str() {
                        "upcase" => Ok(Item::Func(FuncName::Upcase)),
                        "downcase" => Ok(Item::Func(FuncName::Downcase)),
                        // title_case/reverse/strip/swapcase as items are
                        // not usable as `to` targets (Ruby adapter
                        // raises); they can only appear inside any(...),
                        // which the corpus does not exercise.
                        other => Err(Error::MapLogic(format!(
                            "function item {other:?} is not supported in this position"
                        ))),
                    };
                }
                "alias" => {
                    // Transform parity: {identifier:, qualified_name:} is
                    // a qualified dep alias (map.name); {identifier:}
                    // alone is a BARE alias name (the Ruby
                    // `rule(alias: simple(:n))` branch).
                    return Ok(match value.get("qualified_name") {
                        Some(q) => Item::Alias {
                            map: value.get("identifier").map(ident),
                            name: ident(q),
                        },
                        None => Item::Alias {
                            map: None,
                            name: match value.get("identifier") {
                                Some(id) => ident(id),
                                None => leaf_text(value),
                            },
                        },
                    });
                }
                "alias_ref" => {
                    return Ok(Item::Alias {
                        map: None,
                        name: match value.get("identifier") {
                            Some(id) => ident(id),
                            None => leaf_text(value),
                        },
                    });
                }
                "ref" => {
                    let digit = leaf_text(value);
                    return Ok(Item::CaptureRef(digit.parse().map_err(|_| {
                        Error::MapLogic(format!("bad ref({digit:?})"))
                    })?));
                }
                "capture_inner" => {
                    return Ok(Item::CaptureGroup {
                        id: 0,
                        inner: Box::new(self.materialize(value)?),
                    })
                }
                "maybe_inner" => return Ok(Item::Maybe(Box::new(self.materialize(value)?))),
                "some_inner" => return Ok(Item::Some(Box::new(self.materialize(value)?))),
                "any" => return self.convert_any_arg(value),
                "concatenation" => return self.materialize(node),
                _ => {}
            }
        }
        self.convert_other(node)
    }

    /// The `any(...)` argument: range | single-string set | list | alias
    /// (kept as a one-entry set) | nested item.
    fn convert_any_arg(&self, value: &TNode) -> Result<Item> {
        if let Some(hash) = value.ruby_array_hash() {
            let keys: Vec<&str> = hash.iter().map(|(k, _)| k.as_str()).collect();
            let get = |name: &str| pget(hash, name);
            if keys.contains(&"lo") && keys.contains(&"hi") {
                let lo = unquote(get("lo").unwrap());
                let hi = unquote(get("hi").unwrap());
                return match (chars_of(&lo).next(), chars_of(&hi).next()) {
                    (Some(l), Some(h)) => Ok(Item::Range(l, h)),
                    _ => Err(Error::MapLogic("empty any() range bounds".into())),
                };
            }
            if keys.contains(&"single") {
                let s = unquote(get("single").unwrap());
                return Ok(Item::Set(
                    chars_of(&s).map(|c| Item::Str(c.to_string())).collect(),
                ));
            }
            if keys.contains(&"list") {
                let items = get("list")
                    .unwrap()
                    .ruby_array()
                    .iter()
                    .map(|e| self.convert_set_entry(e))
                    .collect::<Result<Vec<_>>>()?;
                return Ok(Item::Set(items));
            }
            if keys.contains(&"alias_ref") || keys.contains(&"alias") {
                // A single alias argument keeps set semantics
                // (Transform rule(any: ...)).
                let alias = self.convert_set_entry(value)?;
                return Ok(Item::Set(vec![alias]));
            }
            if keys.contains(&"any_item") {
                return self.convert_any_arg(get("any_item").unwrap());
            }
            if keys.contains(&"any_char") {
                return Ok(Item::Set(vec![Item::Alias {
                    map: None,
                    name: "any_character".to_string(),
                }]));
            }
            if keys.contains(&"primitive") {
                return Ok(Item::Set(vec![self.convert_set_entry(value)?]));
            }
            if keys.contains(&"string") {
                let s = decode_string_value(get("string").unwrap());
                return Ok(Item::Set(
                    chars_of(&s).map(|c| Item::Str(c.to_string())).collect(),
                ));
            }
            if keys.contains(&"concatenation") {
                let item = self.materialize(value)?;
                return Ok(Item::Set(vec![item]));
            }
        }
        self.convert_set_entry(value)
    }

    /// Entries of an any([...]) list (NodeAdapter#convert_set).
    fn convert_set_entry(&self, entry: &TNode) -> Result<Item> {
        if let TNode::Hash(pairs) = entry {
            let (key, value) = (&pairs[0].0, &pairs[0].1);
            match key.as_str() {
                "string" => return Ok(Item::Str(decode_string_value(value))),
                "primitive" => {
                    let name = leaf_text(value);
                    return Prim::from_name(&name)
                        .map(Item::Prim)
                        .ok_or_else(|| Error::MapLogic(format!("unknown primitive {name:?}")));
                }
                "alias_ref" | "alias" => return self.convert_class_alias(value),
                "concatenation" => return self.materialize(entry),
                "any" => return self.convert_any_arg(value),
                _ => {}
            }
        }
        self.materialize(entry)
    }

    /// Alias in CLASS context (NodeAdapter#resolve_charset): stdlib
    /// -> primitive, imported charset -> Any, local charset alias ->
    /// Any/String, unresolvable -> vacuous "".
    fn convert_class_alias(&self, value: &TNode) -> Result<Item> {
        let (map, name) = match value.get("qualified_name") {
            Some(q) => (value.get("identifier").map(ident), ident(q)),
            None => (None, match value.get("identifier") {
                Some(id) => ident(id),
                None => leaf_text(value),
            }),
        };
        // Qualified refs resolve through the DEP document's alias (word
        // context at runtime); NodeAdapter's class context resolves by
        // bare name only, so keep qualified refs runtime-resolvable.
        if map.is_some() {
            return Ok(Item::Alias { map, name });
        }
        if let Some(prim) = Prim::from_name(&name) {
            return Ok(Item::Prim(prim));
        }
        // Imported library charset: Any(String) charsets become sets.
        if let Some(data) = self.imported.get(&name) {
            return match data {
                Item::Set(entries) => Ok(Item::Set(entries.clone())),
                Item::Range(lo, hi) => Ok(Item::Range(*lo, *hi)),
                Item::Str(s) => Ok(Item::Set(
                    chars_of(s).map(|c| Item::Str(c.to_string())).collect(),
                )),
                _ => Ok(vacuous()),
            };
        }
        // Local alias composed of charset parts (grek_latn_any = ...).
        // Any-valued conversions qualify — Set, Range and plain Str —
        // exactly `converted.is_a?(Any) || converted.is_a?(String)` in
        // NodeAdapter#resolve_charset.
        if let Some((_, raw)) = self.local_raw.iter().find(|(n, _)| *n == name) {
            let converted = self.materialize(raw)?;
            return Ok(match converted {
                Item::Set(_) | Item::Range(_, _) | Item::Str(_) => converted,
                _ => vacuous(),
            });
        }
        Ok(vacuous())
    }

    /// Constraint-position items (class context).
    fn materialize_class(&self, fragment: &TNode) -> Result<Item> {
        match fragment {
            TNode::Hash(_) if fragment.has_key("concatenation") => {
                let parts = fragment.get("concatenation").unwrap();
                let items = parts
                    .ruby_array()
                    .iter()
                    .map(|p| self.convert_class_fragment(p))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Item::concat_from_parts(items))
            }
            TNode::Hash(_) => {
                let item = self.convert_class_fragment(fragment)?;
                Ok(Item::concat_from_parts(vec![item]))
            }
            TNode::Array(parts) => {
                let items = parts
                    .iter()
                    .map(|p| self.convert_class_fragment(p))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Item::concat_from_parts(items))
            }
            TNode::Nil => Ok(Item::None),
            TNode::Str(_) => Ok(Item::Str(fragment.as_str().unwrap_or_default().to_string())),
        }
    }

    fn convert_class_fragment(&self, node: &TNode) -> Result<Item> {
        // Class context only differs for alias refs (charset
        // resolution); everything else converts identically.
        if let TNode::Hash(pairs) = node {
            if let Some((key, value)) = pairs.first() {
                if key == "alias_ref" || key == "alias" {
                    return self.convert_class_alias(value);
                }
            }
        }
        self.convert_fragment(node)
    }

    fn convert_other(&self, node: &TNode) -> Result<Item> {
        let _ = self;
        if let TNode::Hash(pairs) = node {
            let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
            let get = |name: &str| pget(pairs, name);
            if keys.contains(&"lo") && keys.contains(&"hi") {
                let lo = unquote(get("lo").unwrap());
                let hi = unquote(get("hi").unwrap());
                return match (chars_of(&lo).next(), chars_of(&hi).next()) {
                    (Some(l), Some(h)) => Ok(Item::Range(l, h)),
                    _ => Err(Error::MapLogic("empty range bounds".into())),
                };
            }
        }
        Ok(Item::Str(leaf_text(node)))
    }
}

// ----------------------------------------------------------------------
// ISC-WF-1 §4.2 — use/override merge (W2)
// ----------------------------------------------------------------------

/// Merge on a flat rule list (the collection-context expansion):
/// partition R by exact from-equality against O; effective expansion =
/// R_kept (in R's order) followed by O (in O's order). Multiple R
/// rules sharing an overridden from ALL drop (an override is strictly
/// stronger, §4.2 rule 1); override rows matching nothing are simply
/// added (§4.2 rule 4). An override row is complete — it carries its
/// own to/constraints/when (§4.2 rule 3), so no field merging.
fn merge_override_rules(rules: Vec<Sub>, overrides: Vec<Sub>) -> Vec<Sub> {
    let replaced: BTreeSet<String> = overrides.iter().map(|o| o.from_key()).collect();
    let mut kept: Vec<Sub> = rules
        .into_iter()
        .filter(|r| !replaced.contains(&r.from_key()))
        .collect();
    kept.extend(overrides);
    kept
}

/// Merge over a STRUCTURAL expansion (the stage-position use, §8.1):
/// matching rules drop from their collections while kept rules stay in
/// order (§4.2 rules 1-2). The override rows land at the expansion
/// tail per the amended §4.2.6 (ruby reference PR #834): appended INTO
/// the final parallel block when the expansion ends with one — so the
/// child rows share that block's single longest-match pass (§4.2.5) —
/// otherwise appended as trailing stage-level rows. An expansion with
/// no rules at all takes O as one trailing sequential item.
fn merge_override_items(
    mut items: Vec<StageItem>,
    overrides: Vec<Sub>,
) -> Result<Vec<StageItem>> {
    let replaced: BTreeSet<String> = overrides.iter().map(|o| o.from_key()).collect();
    for item in items.iter_mut() {
        if let StageItem::Parallel(list) | StageItem::Sequential(list) = item {
            list.retain(|r| !replaced.contains(&r.from_key()));
        }
    }
    match items.last_mut() {
        Some(StageItem::Parallel(list)) => list.extend(overrides),
        _ => {
            if !overrides.is_empty() {
                items.push(StageItem::Sequential(overrides));
            }
        }
    }
    Ok(items)
}

/// Flatten a structural expansion into the ordered rule list of an
/// enclosing pass (`use` inside a parallel/sequence block).
fn flatten_rules(items: &[StageItem], out: &mut Vec<Sub>) -> Result<()> {
    for item in items {
        match item {
            StageItem::Parallel(list) | StageItem::Sequential(list) => {
                out.extend(list.iter().cloned());
            }
            _ => {
                return Err(Error::MapLogic(
                    "use inside a parallel/sequence block: the referenced \
                     ruleset carries non-rule items (funcalls/stage runs) \
                     that cannot join the pass"
                        .into(),
                ))
            }
        }
    }
    Ok(())
}

fn vacuous() -> Item {
    Item::Str(String::new())
}

// ----------------------------------------------------------------------
// Free helpers
// ----------------------------------------------------------------------

fn is_none_capture(node: Option<&TNode>) -> bool {
    match node {
        None => true,
        Some(TNode::Nil) => true,
        Some(n) => n.has_key("none"),
    }
}

/// Apply the string-decode fold to a `string`-keyed capture value.
pub fn decode_string_value(value: &TNode) -> String {
    match value {
        TNode::Hash(_) => decode_part(value),
        TNode::Array(parts) => parts.iter().map(decode_part).collect(),
        TNode::Str(s) => s.clone(),
        TNode::Nil => String::new(),
    }
}

fn decode_part(part: &TNode) -> String {
    if let Some(pairs) = part.ruby_array_hash() {
        if let Some((key, value)) = pairs.first() {
            return match key.as_str() {
                "run" => leaf_text(value),
                "newline" => "\n".to_string(),
                "carriage_return" => "\r".to_string(),
                "tab" => "\t".to_string(),
                "dquote" => "\"".to_string(),
                "backslash" => "\\".to_string(),
                "u_lone" => "U".to_string(),
                "unicode" => {
                    let hex = leaf_text(value);
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(c) => c.to_string(),
                        // Ruby rescue fallback: the raw hex text.
                        None => hex,
                    }
                }
                _ => leaf_text(part),
            };
        }
        return String::new();
    }
    match part {
        TNode::Str(s) => s.clone(),
        _ => leaf_text(part),
    }
}

/// Normalizer.scalar for quoted strings / fragments.
pub fn unquote(fragment: &TNode) -> String {
    match fragment {
        TNode::Nil => String::new(),
        TNode::Str(s) => s.clone(),
        TNode::Hash(pairs) => {
            if let Some((_, v)) = pairs.iter().find(|(k2, _)| k2 == "string") {
                decode_string_value(v)
            } else {
                leaf_text(fragment)
            }
        }
        TNode::Array(_) => leaf_text(fragment),
    }
}

/// Normalizer.scalar for identifiers.
pub fn ident(fragment: &TNode) -> String {
    match fragment {
        TNode::Nil => String::new(),
        TNode::Str(s) => s.clone(),
        _ => match fragment.get("identifier") {
            Some(id) => leaf_text(id),
            None => leaf_text(fragment),
        },
    }
}

fn chars_of(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars()
}

/// Render a collapsed item as the plain string for funcall kwargs
/// (separator values are NodeAdapter StringValue#value).
fn render_str(item: &Item) -> String {
    match item {
        Item::Str(s) => s.clone(),
        Item::None => String::new(),
        Item::Concat(parts) => parts.iter().map(render_str).collect(),
        _ => String::new(),
    }
}

/// Extract `name: "value"` kwarg pairs (DocumentBuilder#extract_kwargs).
pub fn extract_kwargs(node: &TNode) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let entries: Vec<&TNode> = match node.get("kwarg") {
        Some(kw) => kw.ruby_array().iter().collect(),
        None => vec![node],
    };
    for entry in entries {
        if let (Some(name), Some(value)) = (entry.get("kwarg_name"), entry.get("kwarg_value")) {
            out.push((ident(name), unquote(value)));
        }
    }
    out
}

/// DocumentBuilder#unescape_braces.
pub fn unescape_braces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(n @ ('{' | '}' | '\\')) => out.push(n),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// DocumentBuilder#normalize_heredoc: YAML-style dedent with
/// paragraph preservation.
pub fn normalize_heredoc(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let content: Vec<&&str> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if content.is_empty() {
        return String::new();
    }
    if content.len() == 1 {
        return content[0].trim().to_string();
    }
    let min_indent = content
        .iter()
        .skip(1)
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let mut result: Vec<String> = Vec::new();
    for line in &lines {
        if line.trim().is_empty() {
            result.push(String::new());
        } else {
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            if indent >= min_indent {
                result.push(line[min_indent..].to_string());
            } else {
                result.push(line.trim().to_string());
            }
        }
    }
    let joined = result.join("\n");
    let trimmed = joined.trim_end().to_string();
    if text.ends_with('\n') && !text.ends_with("\n\n") {
        trimmed + "\n"
    } else {
        trimmed
    }
}

/// Metadata is inert for transliteration; best-effort text extraction.
fn extract_metadata(arr: &TNode, out: &mut Vec<(String, String)>) {
    for field in arr.ruby_array() {
        let Some(hash) = field.ruby_array_hash() else { continue };
        if let Some(v) = pget(hash, "field_name") {
            let name = ident(v);
            let val = if let Some(b) = pget(hash, "field_block") {
                normalize_heredoc(&unescape_braces(&leaf_text(b)))
            } else if let Some(fv) = pget(hash, "field_value") {
                match fv {
                    TNode::Hash(_) if fv.has_key("string") => unquote(fv),
                    _ => fv.as_str().unwrap_or_default().trim().to_string(),
                }
            } else {
                String::new()
            };
            out.push((name, val));
        } else {
            for (key, val) in hash.iter() {
                if matches!(val, TNode::Nil) {
                    continue;
                }
                let text = match val {
                    TNode::Hash(_) if val.has_key("string") => unquote(val),
                    _ => leaf_text(val),
                };
                out.push((key.clone(), text));
            }
        }
    }
}

/// Assign 1-based capture group ids in regex order: before,
/// not_before, from, not_after, after (the interpreter's lookaround
/// assembly order). Returns the total group count.
pub fn assign_group_ids(sub: &mut Sub) -> usize {
    let mut counter = 0usize;
    for item in [&mut sub.before, &mut sub.not_before] {
        if let Some(i) = item {
            assign_in(i, &mut counter);
        }
    }
    assign_in(&mut sub.from, &mut counter);
    for item in [&mut sub.not_after, &mut sub.after] {
        if let Some(i) = item {
            assign_in(i, &mut counter);
        }
    }
    counter
}

fn assign_in(item: &mut Item, counter: &mut usize) {
    match item {
        Item::CaptureGroup { id, inner } => {
            *counter += 1;
            *id = *counter;
            assign_in(inner, counter);
        }
        Item::Maybe(inner) | Item::Some(inner) => assign_in(inner, counter),
        Item::Concat(parts) => parts.iter_mut().for_each(|p| assign_in(p, counter)),
        Item::Set(entries) => entries.iter_mut().for_each(|e| assign_in(e, counter)),
        _ => {}
    }
}
