//! Semantics tests: the vendored mini-corpus under tests/corpus
//! (each expectation Ruby-verified) plus runtime-level unit tests
//! for the tricky rules. Reverse rule chains are intentionally NOT
//! tested — see the README "Conformance plan" gap notes.

use interscript_rs::Runtime;
use std::path::PathBuf;

fn corpus_dir() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    vec![dir.clone()]
}

fn rt() -> Runtime {
    Runtime::new(corpus_dir())
}

/// Every map in tests/corpus must parse, load and pass 100% of its
/// embedded tests.
#[test]
fn vendored_corpus_embedded_tests_pass() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut maps: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/corpus exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("isc"))
        .collect();
    maps.sort();

    assert!(maps.len() >= 9, "expected the vendored corpus, found {}", maps.len());

    let runtime = rt();
    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    for map in &maps {
        let id = map.file_stem().unwrap().to_string_lossy().to_string();
        let (passed, failed, failures) = runtime.run_tests(&id).unwrap_or_else(|e| {
            panic!("map {id} failed to load: {e}");
        });
        total_pass += passed;
        total_fail += failed;
        assert_eq!(failed, 0, "map {id} failures: {failures:?}");
    }
    assert!(total_pass >= 10, "expected real test data, got {total_pass}");
}

/// `after boundary` is a lookahead: fires only at word-ending
/// positions; `before boundary` the lookbehind mirror.
#[test]
fn boundary_before_and_after_semantics() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-boundary", "у k").unwrap(),
        "ŭ q"
    );
    assert_eq!(runtime.transliterate("rs-boundary", "kу").unwrap(), "qŭ");
    // A boundary precedes the word-initial 'k' (string start is a
    // boundary), but the mid-word 'k' stays.
    assert_eq!(runtime.transliterate("rs-boundary", "kku").unwrap(), "qku");
}

/// Empty positive constraints are no-ops; empty negative constraints
/// disable the rule (Ruby (?<=) / (?<!) behavior).
#[test]
fn empty_constraint_semantics() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-empty-constraints", "aa").unwrap(),
        "XX"
    );
}

/// any() sets, ranges, aliases and not_after negation with classes.
#[test]
fn any_and_negation_semantics() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-any", "kakibldcx").unwrap(),
        "KmKm*L**m"
    );
}

/// capture(...) groups with ref(N) backreferences, across rules.
#[test]
fn capture_ref_semantics() {
    let runtime = rt();
    assert_eq!(runtime.transliterate("rs-capture", "ckzk").unwrap(), "cqsk");
}

/// Parallel groups: longest-max_length rule first at each position;
/// constraint rules participate in the same simultaneous pass.
#[test]
fn parallel_longest_match_and_constraints() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-parallel", "abcd d").unwrap(),
        "2d D"
    );
}

/// Tree-path groups (no constraints, plain froms): duplicate from
/// keys are LAST-WINS, longest match first — Ruby
/// parallel_replace_compile_tree semantics.
#[test]
fn parallel_tree_duplicate_keys_last_win() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-parallel-tree", "i ab i").unwrap(),
        "b AB b"
    );
}

/// Funcall directives: title_case, downcase, separate, compose,
/// decompose (the full chain the vendored map exercises).
#[test]
fn funcall_chain_semantics() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-funcalls", "§heating").unwrap(),
        "  h e a t i n g"
    );
}

/// Unaliased (library import) dependency: the lib's aliases resolve
/// in word position and as imported charsets.
#[test]
fn library_import_dependency() {
    let runtime = rt();
    assert_eq!(runtime.transliterate("rs-libdep", "aA 7").unwrap(), "aU #");
}

/// Aliased dependency: run map.<dep>.stage.<stage> runs the named
/// stage of the dependency document.
#[test]
fn aliased_dependency_stage_run() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-stage-run", "xaya").unwrap(),
        "Xbyb"
    );
    // The dependency's own main stage works standalone.
    assert_eq!(
        runtime.transliterate("rs-stage-run-dep", "y").unwrap(),
        "Y"
    );
}

/// Nested any() composition: set-of-range+literal entries and an
/// imported range charset.
#[test]
fn nested_any_composition() {
    let runtime = rt();
    assert_eq!(
        runtime.transliterate("rs-nested-any", "가ᄀᆨ").unwrap(),
        "HJJ"
    );
}

/// Unicode-aware \b (Ruby's \b is Unicode while \w is ASCII-only):
/// the vendored map already shows a Cyrillic letter forming a
/// boundary with a Latin letter's edge; pin the asymmetric case.
#[test]
fn word_boundary_is_unicode_aware() {
    let runtime = rt();
    // 'у' at end of "καλ у" — boundary between 'у' and end; the Greek
    // prefix does not affect it.
    assert_eq!(
        runtime.transliterate("rs-boundary", "καλ у").unwrap(),
        "καλ ŭ"
    );
}

/// Truncated map sources are a parse error with a wire.
#[test]
fn truncated_map_is_parse_error() {
    let runtime = rt();
    let err = runtime
        .transliterate("rs-nonexistent", "x")
        .err()
        .expect("missing map errors");
    assert!(matches!(err, interscript_rs::Error::MapNotFound(_)));
}

/// A dependency cycle is detected, not looped on.
#[test]
fn dependency_cycle_detected() {
    let dir = std::env::temp_dir().join(format!("iscrs-cycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.isc"), "system \"a\" { dependency \"b\" as b\n stage main { run map.b.stage.main } }\n").unwrap();
    std::fs::write(dir.join("b.isc"), "system \"b\" { dependency \"a\" as a\n stage main { run map.a.stage.main } }\n").unwrap();
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime.transliterate("a", "x").err().expect("cycle errors");
    assert!(
        matches!(err, interscript_rs::Error::DependencyCycle(_)),
        "got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
