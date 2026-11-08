//! The document model — mirrors `Interscript::Node::Document` as
//! produced by `Interscript::Isc::NodeAdapter`, plus the pieces the
//! interpreter needs (resolved dependency documents, embedded tests).

use crate::item::Item;
use crate::stdlib::Funcall;
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct Sub {
    pub from: Item,
    pub to: SubTo,
    pub before: Option<Item>,
    pub not_before: Option<Item>,
    pub after: Option<Item>,
    pub not_after: Option<Item>,
    /// Number of capture groups in regex order
    /// (before, not_before, from, not_after, after).
    pub group_count: usize,
    /// Ruby `reverse_run:`: `Some(true)` skips the rule in the forward
    /// direction and runs it in the reversed document (and vice versa).
    /// The ISC grammar cannot express it; only `Document::reverse`
    /// produces it.
    pub reverse_run: Option<bool>,
    /// ISC-WF-1 W3: the `when` language guards (§5). Empty = unguarded
    /// (always applicable). A guarded rule applies only when the caller
    /// selects one of these languages.
    pub when: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum SubTo {
    /// `to upcase` — upcase the whole match.
    Upcase,
    /// `to downcase` — downcase the whole match.
    Downcase,
    /// A replacement item.
    Item(Item),
}

impl Sub {
    /// `Interscript::Node::Rule::Sub#max_length`.
    pub fn max_length(&self) -> usize {
        let mut len = self.from.max_length();
        for c in [&self.before, &self.not_before, &self.after, &self.not_after] {
            if let Some(item) = c {
                len += item.max_length();
            }
        }
        len
    }

    /// ISC-WF-1 W2/W3 canonical from-key: the structural rendering of
    /// the EXPANDED from item. For literal froms this is byte-exact
    /// string equality (the corpus convention — no Unicode
    /// normalization); non-literal froms (any()/alias trees) compare
    /// structurally, which is the exact-from generalization the spec's
    /// "from(o) == from(r)" needs on resolved items.
    pub fn from_key(&self) -> String {
        format!("{:?}", self.from)
    }

    /// W3 applicability: unguarded rules always apply; a guarded rule
    /// applies only when the caller selected one of its languages
    /// (§5.1 — no selection, or an undeclared selection, leaves every
    /// guard inert).
    pub fn guard_applies(&self, language: Option<&str>) -> bool {
        self.when.is_empty()
            || language.is_some_and(|l| self.when.iter().any(|w| w == l))
    }
}

#[derive(Debug, Clone)]
pub enum StageItem {
    /// parallel { ... } — single simultaneous pass.
    Parallel(Vec<Sub>),
    /// sequence { ... } rules and bare rules — sequential gsub passes
    /// (the Ruby adapter flattens sequence blocks into the stage).
    Sequential(Vec<Sub>),
    /// run stage.x / run map.<dep>.stage.x
    Run {
        dep: Option<String>,
        stage: String,
        reverse_run: Option<bool>,
    },
    /// A stdlib function application.
    Funcall(Funcall),
}

#[derive(Debug, Clone)]
pub struct Stage {
    pub name: String,
    pub items: Vec<StageItem>,
    /// Ruby `reverse_run:` at stage level (Group#reverse toggles it).
    pub reverse_run: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AliasDef {
    pub name: String,
    pub value: Item,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    /// The located map name (`dependency "x"`).
    pub target: String,
    /// The `as` alias name; None = library import.
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Tests {
    pub data: Vec<(String, String)>,
}

/// A parsed, dependency-resolved map document.
pub struct Document {
    /// The map id (file stem; how `Interscript.transliterate` addresses it).
    pub name: String,
    pub system_code: String,
    pub aliases: BTreeMap<String, Item>,
    pub stages: BTreeMap<String, Stage>,
    /// ISC-WF-1 W1: the map's named rulesets, fully use-expanded at
    /// load time. Never executed directly (§3.2 — a ruleset is not a
    /// stage); only `use` splices their items into a stage.
    pub rulesets: BTreeMap<String, Stage>,
    /// Aliased dependencies (stage-run targets), keyed by alias.
    pub dep_aliases: BTreeMap<String, Rc<Document>>,
    /// Library imports (unaliased dependencies), loaded documents.
    pub imports: Vec<Rc<Document>>,
    pub tests: Tests,
    /// Raw metadata key/value text pairs (inert for transliteration).
    pub metadata: Vec<(String, String)>,
    /// ISC-WF-1 W3: the languages the map declares (`languages a b`).
    pub languages: Vec<String>,
}

impl Document {
    /// `Interscript::Node::Document#imported_aliases`: each import
    /// dependency's own aliases merged UNDER the doc's own aliases.
    pub fn imported_aliases(&self) -> BTreeMap<String, Item> {
        let mut merged: BTreeMap<String, Item> = BTreeMap::new();
        for dep in &self.imports {
            for (k, v) in &dep.aliases {
                merged.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        for (k, v) in &self.aliases {
            merged.insert(k.clone(), v.clone());
        }
        merged
    }

    /// `imported_stages`: import deps' stages merged under own stages.
    pub fn imported_stages(&self) -> BTreeMap<String, Stage> {
        let mut merged: BTreeMap<String, Stage> = BTreeMap::new();
        for dep in &self.imports {
            for (k, v) in &dep.stages {
                merged.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        for (k, v) in &self.stages {
            merged.insert(k.clone(), v.clone());
        }
        merged
    }

    /// Look up an alias through the import chain (word context).
    pub fn lookup_alias(&self, name: &str) -> Option<Item> {
        self.imported_aliases().get(name).cloned()
    }
}
