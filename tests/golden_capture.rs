//! ISC-WF-1 additivity gate (WO16).
//!
//! Re-runs every golden-capture row against the CURRENT runtime and
//! requires byte-identical outputs for all maps outside the adopting
//! set (tests/golden/adopting-maps.txt). A delta on a non-adopting map
//! fails the build — that is the merge gate for every runtime landing
//! of the workflow-extension layer.
//!
//! Skipped when the golden file or the sibling maps checkout is
//! missing.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const GOLDEN: &str = "tests/golden/isc-wf0.jsonl";
const ADOPTING: &str = "tests/golden/adopting-maps.txt";

#[test]
fn golden_capture_additivity() {
    let Ok(golden) = fs::read_to_string(GOLDEN) else {
        eprintln!("skip: no golden capture at {GOLDEN}");
        return;
    };
    let maps_dir = PathBuf::from("../maps/maps");
    if !maps_dir.is_dir() {
        eprintln!("skip: no sibling maps checkout");
        return;
    }

    let adopting: BTreeSet<String> = fs::read_to_string(ADOPTING)
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect()
        })
        .unwrap_or_default();

    let rt = interscript_rs::Runtime::new(vec![
        PathBuf::from("../maps/maps"),
        PathBuf::from("../maps/libs"),
    ]);

    let mut checked = 0usize;
    let mut deltas: Vec<String> = Vec::new();
    let mut per_map_fail: BTreeSet<String> = BTreeSet::new();

    for line in golden.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("golden line not json: {e}: {line}"));
        let map = v["map"].as_str().expect("map field").to_string();
        let src = v["src"].as_str().expect("src field").to_string();
        let golden_out = v["out"].as_str().expect("out field").to_string();

        let got = match rt.transliterate(&map, &src) {
            Ok(v) => v,
            Err(e) => format!("\u{0}error:{e}"),
        };
        if got == golden_out {
            checked += 1;
            continue;
        }
        // adopting maps may legitimately differ
        if adopting.contains(&map) {
            continue;
        }
        per_map_fail.insert(map.clone());
        if deltas.len() < 10 {
            deltas.push(format!(
                "{map}: {src:?} -> {got:?} (golden {golden_out:?})"
            ));
        }
        checked += 1;
    }

    assert!(
        per_map_fail.is_empty(),
        "additivity violated on {} non-adopting map(s): {:?}\nfirst deltas:\n{}",
        per_map_fail.len(),
        per_map_fail,
        deltas.join("\n")
    );
    eprintln!("additivity: {checked} rows byte-identical; adopting set: {} map(s)", adopting.len());
}
