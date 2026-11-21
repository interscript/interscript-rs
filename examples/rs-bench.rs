//! rs-bench — benchmark runner for the interscript-rs runtime.
//!
//! Uniform benchmark interface shared by the four runtime runners
//! (see docs/benchmarks/PROTOCOL.md). Emits exactly one JSON object on
//! stdout.
//!
//!   rs-bench --phase load|throughput|peak --inputs bench-inputs.json \
//!           --maps-repo ../maps [--loops 5] [--commit <sha>]
//!
//! Phases:
//!   load       fresh Runtime, load every maps/*.isc (cold registry)
//!   throughput load the input set's maps, then N loops over all docs
//!   peak       load + transliterate the single largest input doc once

use interscript_rs::Runtime;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Args {
    phase: String,
    inputs: PathBuf,
    maps_repo: PathBuf,
    loops: usize,
    commit: String,
    maps_commit: String,
}

fn parse_args() -> Result<Args, String> {
    let mut phase = String::new();
    let mut inputs = PathBuf::new();
    let mut maps_repo = PathBuf::from("../maps");
    let mut loops = 5usize;
    let mut commit = String::from("unknown");
    let mut maps_commit = String::from("unknown");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--phase" => phase = it.next().ok_or("--phase needs a value")?,
            "--inputs" => inputs = PathBuf::from(it.next().ok_or("--inputs needs a value")?),
            "--maps-repo" => maps_repo = PathBuf::from(it.next().ok_or("--maps-repo needs a value")?),
            "--loops" => loops = it.next().ok_or("--loops needs a value")?.parse().map_err(|_| "--loops must be a number")?,
            "--commit" => commit = it.next().ok_or("--commit needs a value")?,
            "--maps-commit" => maps_commit = it.next().ok_or("--maps-commit needs a value")?,
            other => return Err(format!("unknown arg {other}")),
        }
    }
    if phase.is_empty() {
        return Err("missing --phase".into());
    }
    Ok(Args { phase, inputs, maps_repo, loops, commit, maps_commit })
}

fn list_system_maps(maps_dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(maps_dir)
        .expect("maps dir")
        .into_iter()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("isc"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();
    ids.sort();
    ids
}

fn load_inputs(path: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("inputs file");
    serde_json::from_str(&raw).expect("inputs json")
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let maps_dir = args.maps_repo.join("maps");
    let libs_dir = args.maps_repo.join("libs");
    let load_path = vec![maps_dir.clone(), libs_dir.clone()];
    let base = json!({
        "runtime": "rust",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": args.commit,
        "phase": args.phase,
        "maps_commit": args.maps_commit,
    });

    match args.phase.as_str() {
        "load" => {
            let ids = list_system_maps(&maps_dir);
            let rt = Runtime::new(load_path);
            let t0 = Instant::now();
            let mut loaded = 0usize;
            let mut failed: Vec<String> = Vec::new();
            for id in &ids {
                match rt.load(id) {
                    Ok(_) => loaded += 1,
                    Err(e) => failed.push(format!("{id}: {e}")),
                }
            }
            let ms = t0.elapsed().as_millis() as u64;
            println!(
                "{}",
                json!({ "runtime": base["runtime"], "version": base["version"], "commit": args.commit,
                    "phase": "load", "maps_commit": args.maps_commit,
                    "attempted": ids.len(), "loaded": loaded, "failed": failed, "cold_load_ms": ms })
            );
        }
        "throughput" => {
            let inputs = load_inputs(&args.inputs);
            let docs: Vec<(String, String, usize)> = inputs["inputs"]
                .as_array().expect("inputs array")
                .iter()
                .map(|d| (
                    d["map"].as_str().unwrap().to_string(),
                    d["src"].as_str().unwrap().to_string(),
                    d["chars"].as_u64().unwrap() as usize,
                ))
                .collect();
            let rt = Runtime::new(load_path);
            let mut load_failed: Vec<String> = Vec::new();
            for (map, _, _) in &docs {
                if let Err(e) = rt.load(map) {
                    load_failed.push(format!("{map}: {e}"));
                }
            }
            // warm pass (untimed): prime per-map derived state
            let mut warm_err = 0usize;
            for (map, src, _) in &docs {
                if rt.transliterate(map, src).is_err() {
                    warm_err += 1;
                }
            }
            let total_chars: usize = docs.iter().map(|(_, _, c)| c).sum();
            let t0 = Instant::now();
            let mut errors = 0usize;
            for _ in 0..args.loops {
                for (map, src, _) in &docs {
                    if rt.transliterate(map, src).is_err() {
                        errors += 1;
                    }
                }
            }
            let wall_s = t0.elapsed().as_secs_f64();
            let calls = (docs.len() * args.loops) as f64;
            println!(
                "{}",
                json!({ "runtime": base["runtime"], "version": base["version"], "commit": args.commit,
                    "phase": "throughput", "maps_commit": args.maps_commit,
                    "docs": docs.len(), "loops": args.loops, "total_chars": total_chars,
                    "load_failed": load_failed, "warm_errors": warm_err,
                    "wall_s": wall_s, "docs_per_sec": calls / wall_s,
                    "chars_per_sec": (total_chars * args.loops) as f64 / wall_s,
                    "errors": errors })
            );
        }
        "peak" => {
            let inputs = load_inputs(&args.inputs);
            let arr = inputs["inputs"].as_array().expect("inputs array");
            // first max (strict >), matching the other runners' tie-break
            let mut big = &arr[0];
            for d in &arr[1..] {
                if d["chars"].as_u64().unwrap() > big["chars"].as_u64().unwrap() {
                    big = d;
                }
            }
            let map = big["map"].as_str().unwrap();
            let src = big["src"].as_str().unwrap();
            let chars = big["chars"].as_u64().unwrap();
            let rt = Runtime::new(load_path);
            let t0 = Instant::now();
            let load_ok = rt.load(map).is_ok();
            let out = rt.transliterate(map, src);
            let ms = t0.elapsed().as_millis() as u64;
            println!(
                "{}",
                json!({ "runtime": base["runtime"], "version": base["version"], "commit": args.commit,
                    "phase": "peak", "maps_commit": args.maps_commit,
                    "map": map, "chars": chars, "load_ok": load_ok,
                    "ok": out.is_ok(), "wall_ms": ms })
            );
        }
        other => {
            eprintln!("unknown phase {other}");
            std::process::exit(2);
        }
    }
}

