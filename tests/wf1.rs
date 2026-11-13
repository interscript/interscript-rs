//! ISC-WF-1 tests — the workflow-extension layer (W1 named rulesets,
//! W2 use-with-override, W3 language conditioning), per docs/isc-wf-1.md
//! (maps repo, PR #259). Fixtures live in tests/corpus/rs-wf-*.isc;
//! load-error cases build synthetic maps in a temp dir (they must not
//! load, so they cannot sit in the vendored corpus).
//!
//! Semantics pinned here:
//! * W1 §3: expansion is textual-equivalent and recursive; cycles are
//!   a load error; pass structure is inherited; a ruleset never
//!   executes on its own.
//! * W2 §4: child-wins on exact from-equality; effective expansion =
//!   kept-in-order then overrides; an override row is complete and may
//!   carry its own `when`.
//! * W3 §5: guards are inert without a selection (or with an
//!   undeclared selection); guard-applicable beats unguarded
//!   same-from; sequence guards skip inert rows.

use interscript_rs::Error;
use interscript_rs::Runtime;
use std::path::PathBuf;

fn corpus_dir() -> Vec<PathBuf> {
    vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")]
}

fn rt() -> Runtime {
    Runtime::new(corpus_dir())
}

// ----------------------------------------------------------------------
// W1 — named rulesets
// ----------------------------------------------------------------------

/// Recursive expansion: `use vowels_plus` expands vowels_plus, whose
/// own `use vowels` expands vowels — all in position, document order.
#[test]
fn ruleset_expansion_is_recursive_and_in_position() {
    let runtime = rt();
    // a→A, e→E, i→I (use-expanded), then ONE parallel pass (k→K),
    // then the bare x→X.
    assert_eq!(
        runtime.transliterate("rs-wf-ruleset", "aeikx").unwrap(),
        "AEIKX"
    );
}

/// Pass-structure inheritance: a ruleset's parallel block contributes
/// ONE pass at the use position (so "kk" longest-matches before "k").
#[test]
fn ruleset_parallel_block_is_one_pass() {
    let runtime = rt();
    assert_eq!(runtime.transliterate("rs-wf-ruleset", "kk").unwrap(), "KK");
}

/// A ruleset body may carry funcalls; they execute at the expansion
/// position (`fun` = parallel s→S then upcase).
#[test]
fn ruleset_funcall_passes_through() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate_stage("rs-wf-ruleset", "fun", "sis", None).unwrap(),
        "SIS"
    );
}

/// `use` inside a parallel block joins the enclosing pass (§3.2):
/// the expansion flattens into the block's rule list.
#[test]
fn use_inside_parallel_joins_the_pass() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_stage("rs-wf-ruleset", "flat", "kpk kp kk", None)
            .unwrap(),
        "KPK KP KK"
    );
}

/// A ruleset is never a stage: it cannot be run directly (§3.2).
#[test]
fn ruleset_is_not_a_stage() {
    let runtime = rt();
    let err = runtime
        .transliterate_stage("rs-wf-ruleset", "vowels", "a", None)
        .err()
        .expect("ruleset must not execute as a stage");
    assert!(matches!(err, Error::MapLogic(_)), "got {err:?}");
    assert!(err.to_string().contains("not found"));
}

/// The Document carries the expanded ruleset table (for dependents to
/// splice) and the declared languages; stages stay flat.
#[test]
fn document_carries_expanded_rulesets_and_languages() {
    let runtime = rt();
    let doc = runtime.load("rs-wf-ruleset").unwrap();
    assert_eq!(doc.rulesets.len(), 4);
    let plus = &doc.rulesets["vowels_plus"];
    // vowels_plus = use vowels (2 items) + 1 own sub = 3 expanded items.
    assert_eq!(plus.items.len(), 3);
    // Languages: declaration order, deduped.
    let lang_doc = runtime.load("rs-wf-languages").unwrap();
    assert_eq!(lang_doc.languages, vec!["kur".to_string(), "chv".to_string()]);
}

/// Languages + declaration order, deduped; a `ruleset` block head
/// directly after a languages list must parse (the ident-list guard
/// stops at any block head — `ruleset` included).
#[test]
fn ruleset_after_languages_decl_parses() {
    let dir = std::env::temp_dir().join(format!("iscrs-langs-rs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("langrs.isc"),
        "system \"T:langrs:1\" {\n languages kur\n ruleset r1 { sequence { sub \"s\" \"1\" } }\n stage main { use r1 }\n}\n",
    )
    .unwrap();
    let runtime = Runtime::new(vec![dir.clone()]);
    assert_eq!(runtime.transliterate("langrs", "ss").unwrap(), "11");
}

/// Expansion cycle: a bare `use` chain revisiting a ruleset is a load
/// error (§3.2, §6.4).
#[test]
fn use_cycle_is_a_load_error() {
    let dir = std::env::temp_dir().join(format!("iscrs-use-cycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("cycle.isc"),
        "system \"T:cycle:1\" {\n ruleset a { use b }\n ruleset b { use a }\n stage main { use a }\n}\n",
    )
    .unwrap();
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime
        .transliterate("cycle", "x")
        .err()
        .expect("cycle must not load");
    assert!(matches!(err, Error::MapLogic(ref m) if m.contains("use cycle")), "got {err:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Self-reference is a cycle too.
#[test]
fn self_use_cycle_is_a_load_error() {
    let dir = std::env::temp_dir().join(format!("iscrs-self-cycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("selfcyc.isc"),
        "system \"T:selfcyc:1\" {\n ruleset a { use a }\n stage main { use a }\n}\n",
    )
    .unwrap();
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime.transliterate("selfcyc", "x").err().expect("cycle");
    assert!(matches!(err, Error::MapLogic(ref m) if m.contains("use cycle")), "got {err:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A ruleset declared twice is a load error (py is the canon —
/// first-wins and order-dependent merges both silently pick a body).
#[test]
fn duplicate_ruleset_is_a_load_error() {
    let dir = std::env::temp_dir().join(format!("iscrs-dup-rs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("dup.isc"),
        "system \"T:dup:1\" { ruleset a { sub \"x\" \"1\" } ruleset a { sub \"x\" \"2\" } stage main { use a } }\n",
    )
    .unwrap();
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime.transliterate("dup", "x").err().expect("duplicate must not load");
    assert!(
        matches!(err, Error::MapLogic(ref m) if m.contains("ruleset \"a\" declared")),
        "got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `use` of an undeclared ruleset is a load error.
#[test]
fn unknown_ruleset_is_a_load_error() {
    let dir = std::env::temp_dir().join(format!("iscrs-unknown-rs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("unknown.isc"),
        "system \"T:unknown:1\" {\n stage main { use nope }\n}\n",
    )
    .unwrap();
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime.transliterate("unknown", "x").err().expect("error");
    assert!(
        matches!(err, Error::MapLogic(ref m) if m.contains("ruleset \"nope\" not found")),
        "got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `use alias.<ruleset>` through an unknown alias, or for a ruleset
/// the dependency does not declare, is a load error (§3.3: declared
/// dependencies only).
#[test]
fn unknown_dep_ruleset_is_a_load_error() {
    let dir = std::env::temp_dir().join(format!("iscrs-unknown-dep-rs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("badalias.isc"),
        "system \"T:badalias:1\" {\n stage main { use shared.base }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("badrs.isc"),
        "system \"T:badrs:1\" {\n dependency \"rs-wf-override-dep\" as shared\n stage main { use shared.nope }\n}\n",
    )
    .unwrap();
    let dir2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let runtime = Runtime::new(vec![dir.clone(), dir2]);
    let err = runtime.transliterate("badalias", "x").err().expect("error");
    assert!(
        matches!(err, Error::MapLogic(ref m) if m.contains("dep alias \"shared\" not found")),
        "got {err:?}"
    );
    let err = runtime.transliterate("badrs", "x").err().expect("error");
    assert!(
        matches!(err, Error::MapLogic(ref m) if m.contains("ruleset \"nope\" of dep \"shared\" not found")),
        "got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ----------------------------------------------------------------------
// W2 — use with override
// ----------------------------------------------------------------------

/// §8.1 pattern across a dependency: child wins on exact from-equality
/// (k→q replaced by k→k), rows matching nothing are added (c→c), kept
/// rows stay in the base's order (kh→x before t→t).
#[test]
fn override_child_wins_exact_from() {
    let runtime = rt();
    assert_eq!(runtime.transliterate("rs-wf-override", "khtk").unwrap(), "xtk");
    // The added row participates too.
    assert_eq!(runtime.transliterate("rs-wf-override", "kc").unwrap(), "kc");
}

/// §4.2 rule 1: if multiple rules in R share the overridden from, ALL
/// of them are replaced — the ruleset's internal last-wins winner ("2")
/// is superseded too.
#[test]
fn override_replaces_every_same_from_row() {
    let runtime = rt();
    let out = runtime
        .transliterate_stage("rs-wf-override", "dup", "s", None)
        .unwrap();
    assert_eq!(out, "S");
}

/// Amended §4.2.6: override rows join the expansion's TRAILING
/// parallel block (pinned by override_child_wins_exact_from — the
/// appended k→k row must share the single pass); when the expansion
/// does NOT end with one, they append as stage-level rows after it
/// (here after the funcall: the row fires post-downcase).
#[test]
fn override_appends_stage_level_without_trailing_parallel() {
    let runtime = rt();
    let out = runtime
        .transliterate_stage("rs-wf-override", "tailjoin", "qA", None)
        .unwrap();
    // parallel (a→1 only) leaves "qA"; downcase → "qa"; THEN q→Q.
    assert_eq!(out, "Qa");
}

/// §4.2 rule 2 in a sequence context: kept rows keep R's order, the
/// overrides run after them ("ab" no longer catches first).
#[test]
fn override_keeps_order_then_appends() {
    let runtime = rt();
    let out = runtime
        .transliterate_stage("rs-wf-override", "seq", "ab", None)
        .unwrap();
    // kept b→Y fires first (ab -> aY), then ab→AB no longer matches.
    assert_eq!(out, "aY");
}

/// §4.2 rule 3 / §5.3: an override row is complete and its `when`
/// applies to that row alone. With the selection the replacement row
/// fires; without it the row is inert AND the imported row is gone
/// (replaced), so the from passes through untouched.
#[test]
fn override_row_guard_applies_to_that_row_alone() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_stage("rs-wf-override", "lang", "k", Some("kur"))
            .unwrap(),
        "kk"
    );
    assert_eq!(
        runtime
            .transliterate_stage("rs-wf-override", "lang", "k", None)
            .unwrap(),
        "k"
    );
}

// ----------------------------------------------------------------------
// W3 — language conditioning
// ----------------------------------------------------------------------

/// §5.4 with no selection: every guard is inert — the map behaves
/// exactly as if the guarded rows were absent (embedded tests pin this
/// through the full path).
#[test]
fn no_selection_all_guards_inert() {
    let runtime = rt();
    assert_eq!(runtime.transliterate("rs-wf-languages", "қҫ").unwrap(), "ķş");
}

/// §5.4 `--language kur`: the kur guard beats the generic same-from
/// row; the chv guard is inert.
#[test]
fn kur_selection_guard_beats_generic() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_lang("rs-wf-languages", "қҫ", Some("kur"))
            .unwrap(),
        "k̦ş"
    );
}

/// §5.4 `--language chv`: the chv guard applies; the kur guard is
/// inert so the generic қ row applies.
#[test]
fn chv_selection_guard_beats_generic() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_lang("rs-wf-languages", "қҫ", Some("chv"))
            .unwrap(),
        "ķș"
    );
}

/// §5.1: selecting a language the map does not declare behaves like no
/// selection — every guard inert, no partial application.
#[test]
fn undeclared_selection_is_like_no_selection() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_lang("rs-wf-languages", "қҫ", Some("rus"))
            .unwrap(),
        "ķş"
    );
}

/// §5.1: guards on a map with NO `languages` declaration are inert
/// under any selection — nothing is selectable.
#[test]
fn no_declaration_selection_activates_nothing() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_lang("rs-wf-nodecl", "қ", Some("kur"))
            .unwrap(),
        "ķ"
    );
}

/// §5.1: a `when` guard whose language is outside the declared set
/// stays inert even when that exact language is selected; the
/// declared guard still fires.
#[test]
fn guard_outside_declared_set_is_inert() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_lang("rs-wf-subset", "қ", Some("kur"))
            .unwrap(),
        "k̦"
    );
    assert_eq!(
        runtime
            .transliterate_lang("rs-wf-subset", "қ", Some("rus"))
            .unwrap(),
        "ķ"
    );
}

/// §5.2: sequence blocks apply their rules in order; a guarded rule
/// whose guard is inert is skipped entirely (so with no selection the
/// generic row fires, and with kur the guarded row consumes the match
/// first).
#[test]
fn sequence_guards_skip_when_inert() {
    let runtime = rt();
    assert_eq!(
        runtime
            .transliterate_stage("rs-wf-languages", "seqguards", "k", None)
            .unwrap(),
        "Q"
    );
    assert_eq!(
        runtime
            .transliterate_stage("rs-wf-languages", "seqguards", "k", Some("kur"))
            .unwrap(),
        "K"
    );
    assert_eq!(
        runtime
            .transliterate_stage("rs-wf-languages", "seqguards", "k", Some("chv"))
            .unwrap(),
        "Q"
    );
}
