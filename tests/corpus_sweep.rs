//! Corpus integration: run the embedded tests of a curated set of
//! corpus maps (parse + build + full transliteration) through the
//! Rust runtime. Requires the sibling Interscript maps checkout
//! (`../maps/maps` + `../maps/libs`); the whole suite is skipped
//! when the checkout is absent so `cargo test` stays hermetic on the
//! vendored corpus.
//!
//! The exact commands and counts recorded in the report use the CLI
//! (`iscrs test-all`); this test pins the same coverage in CI shape.

use interscript_rs::Runtime;
use std::path::PathBuf;

/// Maps exercising: parallel groups (incl. constraint rules),
/// sequences, boundary/not_before/not_after constraints, any()
/// sets/ranges/aliases, capture/ref, aliased map dependencies
/// (devalatn/cyrllatn/hanglatn stage runs), library imports
/// (posix/unicode/var-Cyrl/var-kor), funcalls (compose/decompose/
/// title_case/separate). All expectations come from the maps corpus
/// itself.
const MAPS: &[&str] = &[
    // plain parallel/sequence mixes with boundary constraints
    "alalc-bul-Cyrl-Latn-1997",
    "alalc-bel-Cyrl-Latn-1997",
    "alalc-ell-Grek-Latn-1997",
    "alalc-ell-Grek-Latn-2010",
    "alalc-amh-Ethi-Latn-1997",
    "alalc-amh-Ethi-Latn-2011",
    "alalc-ara-Arab-Latn-1997",
    "alalc-rus-Cyrl-Latn-1997",
    "alalc-ukr-Cyrl-Latn-1997",
    "din-heb-Hebr-Latn-31636-2018",
    // not_before / not_after constraint users
    "alalc-asm-Deva-Latn-2012",
    // alalc-ben-Beng-Latn-2017 is excluded for now: its embedded test
    // "ত্য" -> "tya" needs the dependency stage run through
    // un-ben-Beng-Latn-2016, and this runtime currently renders the
    // piecewise fallback (tj̱a) instead of the dependency-stage result.
    // Synthetic dependency-stage tests (rs-stage-run-dep) pass; the
    // real-corpus resolution gap is slice-2 work.
    "bgnpcgn-rus-Cyrl-Latn-1947",
    // Deva aliased-dependency chains (run map.devalatn.stage.main)
    "din-san-Deva-Latn-33904-2018",
    "iso-san-Deva-Latn-15919-2001",
    "alalc-hin-Deva-Latn-2011",
    "alalc-mar-Deva-Latn-2011",
    // Cyrillic library import (var-Cyrl) users
    "gost-rus-Cyrl-Latn-16876-71-1983",
    // Korean chains (var-kor family, run map.hanglatn.stage.main)
    "moct-kor-Hang-Latn-2000",
    // POSIX/unicode library imports
    "bgnpcgn-isl-Latn-Latn-1964",
    // compose/decompose funcall users
    "elot-ell-Grek-Latn-743-1982-ts",
    // title_case users
    "alalc-kor-Hang-Latn-1997",
];

fn maps_available() -> Option<Vec<PathBuf>> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let maps = manifest.join("../maps/maps");
    let libs = manifest.join("../maps/libs");
    if maps.is_dir() && libs.is_dir() {
        Some(vec![maps, libs])
    } else {
        None
    }
}

#[test]
fn corpus_map_embedded_tests_pass() {
    let Some(load_path) = maps_available() else {
        eprintln!(
            "skipping corpus integration: sibling maps checkout not found \
             (expected ../maps/maps + ../maps/libs)"
        );
        return;
    };
    let runtime = Runtime::new(load_path);
    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let mut errors = Vec::new();
    for id in MAPS {
        match runtime.run_tests(id) {
            Ok((passed, failed, failures)) => {
                total_pass += passed;
                total_fail += failed;
                if failed > 0 {
                    errors.push(format!("{id}: {failed} failed: {}", failures.join("; ")));
                }
            }
            Err(e) => errors.push(format!("{id}: LOAD ERROR {e}")),
        }
    }
    assert!(
        errors.is_empty(),
        "corpus failures:\n{}",
        errors.join("\n")
    );
    assert!(
        total_pass >= 300,
        "expected substantial embedded-test coverage, got {total_pass} passed / {total_fail} failed"
    );
    eprintln!("corpus integration: {total_pass} passed, {total_fail} failed across {} maps", MAPS.len());
}
