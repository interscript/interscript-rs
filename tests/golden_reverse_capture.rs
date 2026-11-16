//! WO20 reverse-mode golden additivity gate.
//!
//! Mirrors `golden_capture.rs` for the reverse golden
//! (tests/golden/isc-reverse.jsonl): re-runs every captured row against
//! the CURRENT runtime — load the map, reverse the document
//! (`Document#reverse` parity), execute the reversed main stage on the
//! row's src — and requires byte-identical output. The capture is
//! RS-only (`iscrs capture --reverse`); every other runtime replays
//! this exact file, so a delta here is a reverse-parity regression.
//!
//! An execution error replays as `null`, the canonical marker the
//! capture records for errors (and the marker the ruby/ts/py replay
//! gates map their own errors to).
//!
//! Skipped when the golden file or the sibling maps checkout is
//! missing.

use std::fs;
use std::path::PathBuf;

const GOLDEN: &str = "tests/golden/isc-reverse.jsonl";

#[test]
fn golden_reverse_capture_additivity() {
    let Ok(golden) = fs::read_to_string(GOLDEN) else {
        eprintln!("skip: no reverse golden capture at {GOLDEN}");
        return;
    };
    let maps_dir = PathBuf::from("../maps/maps");
    if !maps_dir.is_dir() {
        eprintln!("skip: no sibling maps checkout");
        return;
    }

    // The capture's load path: corpus first, then this repo's reverse
    // fixtures (a corpus map shadows a same-named fixture exactly like
    // `locate` — the capture enumerates in this order and dedupes).
    let rt = interscript_rs::Runtime::new(vec![
        PathBuf::from("../maps/maps"),
        PathBuf::from("../maps/libs"),
        PathBuf::from("tests/reverse_maps"),
    ]);

    let mut checked = 0usize;
    let mut deltas: Vec<String> = Vec::new();
    let mut per_map_fail: std::collections::BTreeSet<String> = Default::default();

    for line in golden.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("golden line not json: {e}: {line}"));
        let map = v["map"].as_str().expect("map field").to_string();
        let src = v["src"].as_str().expect("src field").to_string();
        let golden_out = v["out"].as_str().expect("out field").to_string();

        let got = (|| -> interscript_rs::Result<String> {
            let doc = rt.load(&map)?;
            let rdoc = interscript_rs::reverse::reverse_document(&doc)?;
            interscript_rs::interpreter::execute_stage_pub(&rdoc, "main", &src, None)
        })()
        .unwrap_or_else(|_| "null".to_string());
        checked += 1;
        if got == golden_out {
            continue;
        }
        per_map_fail.insert(map.clone());
        if deltas.len() < 10 {
            deltas.push(format!("{map}: {src:?} -> {got:?} (golden {golden_out:?})"));
        }
    }

    assert!(
        per_map_fail.is_empty(),
        "reverse additivity violated on {} map(s): {:?}\nfirst deltas:\n{}",
        per_map_fail.len(),
        per_map_fail,
        deltas.join("\n")
    );
    eprintln!("reverse additivity: {checked} rows byte-identical");
}
