//! `iscrs` — the interscript-rs CLI. Mirrors the Ruby runtime's
//! entry points:
//!
//!   iscrs transliterate <map_id> <string>   # Interscript.transliterate
//!   iscrs test <map_id>...                  # run a map's embedded tests
//!   iscrs test-all                          # sweep the corpus
//!   iscrs parse <file|dir>...               # parse-only check
//!   iscrs parse-all                         # parse sweep of a maps dir
//!   iscrs grammar-test                      # the .parg embedded suite

use interscript_rs::{Error, Runtime};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(msg)) => {
            eprintln!("{msg}\n\n{}", usage());
            ExitCode::from(2)
        }
        Err(CliError::Runtime(e)) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
        Err(CliError::Failures) => ExitCode::FAILURE,
    }
}

enum CliError {
    Usage(String),
    Runtime(Error),
    Failures,
}

impl From<Error> for CliError {
    fn from(e: Error) -> Self {
        CliError::Runtime(e)
    }
}

fn usage() -> String {
    r#"commands:
  transliterate <map_id> <string> [--stage NAME] [--language LANG]
  reverse <map_id> <string> [--stage NAME] [--language LANG]
                                          # run the reversed document
  test <map_id>...
  test-all [--root DIR] [--limit N]
  parse <file-or-dir>...
  parse-all [--root DIR]
  maps-fetch <version> [--dest DIR]   # fetch + verify + unpack the maps corpus release
  capture [-o FILE] [--root DIR]      # golden transliteration capture (jsonl)
  grammar-test
  infer <artifact_zip_or_model_id> <string> [--max-len N]
  conformance <fixture_file_or_dir> [<artifact_dir>]

options:
  --maps DIR...    add a maps/library directory to the load path
                   (default: $ISCRS_MAPS or the sibling maps checkout)
  --language LANG  ISC-WF-1 W3 language selection for transliterate
                   (guards inert when the map does not declare LANG)
  --json           machine-readable summary lines (test-all/parse-all)

environment (IMF commands):
  SECRYST_INDEX    models.yaml index URL or path (default: index-v12)
  SECRYST_CACHE    artifact cache dir (default: ~/.cache/secryst)"#
        .to_string()
}

fn run(args: &[String]) -> Result<(), CliError> {
    let mut maps_dirs: Vec<PathBuf> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut stage = "main".to_string();
    let mut language: Option<String> = None;
    let mut limit: Option<usize> = None;
    let mut max_len = interscript_rs::imf::infer::MAX_DECODE_STEPS;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--maps" => {
                let dir = iter.next().ok_or_else(|| CliError::Usage("--maps needs a value".into()))?;
                maps_dirs.push(PathBuf::from(dir));
            }
            "--stage" => {
                stage = iter
                    .next()
                    .ok_or_else(|| CliError::Usage("--stage needs a value".into()))?
                    .clone();
            }
            "--language" => {
                language = Some(
                    iter.next()
                        .ok_or_else(|| CliError::Usage("--language needs a value".into()))?
                        .clone(),
                );
            }
            "--limit" => {
                let v = iter
                    .next()
                    .ok_or_else(|| CliError::Usage("--limit needs a value".into()))?;
                limit = Some(v.parse().map_err(|_| CliError::Usage("--limit needs a number".into()))?);
            }
            "--max-len" => {
                let v = iter
                    .next()
                    .ok_or_else(|| CliError::Usage("--max-len needs a value".into()))?;
                max_len = v
                    .parse()
                    .map_err(|_| CliError::Usage("--max-len needs a number".into()))?;
            }
            "--json" => json = true,
            // maps-fetch takes its own --dest; pass it through to the
            // subcommand parser as positionals.
            "--dest" => positional.push("--dest".into()),
            // capture takes its own -o/--root; pass through to the
            // subcommand parser as positionals.
            "-o" | "--root" => positional.push(arg.clone()),
            other if other.starts_with("--") => {
                return Err(CliError::Usage(format!("unknown option {other}")));
            }
            _ => positional.push(arg.clone()),
        }
    }

    let Some(cmd) = positional.first() else {
        return Err(CliError::Usage(usage()));
    };
    let rest = &positional[1..];

    // The IMF commands do not need a maps load path.
    match cmd.as_str() {
        "infer" => return cmd_infer(rest, max_len).map_err(CliError::from),
        "conformance" => return cmd_conformance(rest).map_err(CliError::from),
        "maps-fetch" => return cmd_maps_fetch(rest).map_err(CliError::Usage),
        "capture" => return cmd_capture(rest).map_err(CliError::Usage),
        _ => {}
    }

    let load_path = if maps_dirs.is_empty() {
        interscript_rs::locate::default_load_path()
    } else {
        maps_dirs
    };
    if load_path.is_empty() {
        return Err(CliError::Usage(
            "no maps directory found; pass --maps DIR or set ISCRS_MAPS".into(),
        ));
    }

    match cmd.as_str() {
        "transliterate" => {
            if rest.len() != 2 {
                return Err(CliError::Usage("usage: transliterate <map_id> <string>".into()));
            }
            let rt = Runtime::new(load_path);
            let out = rt.transliterate_stage(&rest[0], &stage, &rest[1], language.as_deref())?;
            println!("{out}");
        }
        "reverse" => {
            if rest.len() != 2 {
                return Err(CliError::Usage("usage: reverse <map_id> <string>".into()));
            }
            let rt = Runtime::new(load_path);
            let doc = rt.load(&rest[0])?;
            let reversed = interscript_rs::reverse::reverse_document(&doc)?;
            let out = interscript_rs::interpreter::execute_stage_pub(
                &reversed,
                &stage,
                &rest[1],
                language.as_deref(),
            )?;
            println!("{out}");
        }
        "test" => {
            if rest.is_empty() {
                return Err(CliError::Usage("usage: test <map_id>...".into()));
            }
            let rt = Runtime::new(load_path);
            let mut total_pass = 0usize;
            let mut total_fail = 0usize;
            for map in rest {
                let (passed, failed, failures) = rt.run_tests(map)?;
                println!("{map}: {passed} passed, {failed} failed");
                for f in &failures {
                    println!("  {f}");
                }
                total_pass += passed;
                total_fail += failed;
            }
            println!("total: {total_pass} passed, {total_fail} failed");
            if total_fail > 0 {
                return Err(CliError::Failures);
            }
        }
        "test-all" => {
            let root = root_from_rest(rest)?;
            let maps = list_isc(&root);
            let rt = Runtime::new(load_path);
            let mut maps_with_tests = 0usize;
            let mut maps_run = 0usize;
            let mut total_pass = 0usize;
            let mut total_fail = 0usize;
            let mut failing_maps: Vec<String> = Vec::new();
            let mut skipped: Vec<String> = Vec::new();
            for (i, path) in maps.iter().enumerate() {
                if let Some(l) = limit {
                    if maps_run >= l {
                        break;
                    }
                }
                let map_id = path.file_stem().unwrap().to_string_lossy().to_string();
                // A map without tests still proves parse+build+load.
                let has_tests = match rt.run_tests(&map_id) {
                    Ok((passed, failed, failures)) => {
                        if passed + failed == 0 {
                            if !json {
                                println!("SKIP-NO-TESTS {map_id}");
                            }
                            skipped.push(format!("{map_id} (no tests)"));
                            continue;
                        }
                        maps_run += 1;
                        maps_with_tests += 1;
                        total_pass += passed;
                        total_fail += failed;
                        if failed > 0 {
                            println!("FAIL {map_id}: {failed} failed");
                            for f in &failures {
                                println!("  {f}");
                            }
                            failing_maps.push(map_id.clone());
                        } else if !json {
                            println!("OK   {map_id}: {passed} passed");
                        }
                        true
                    }
                    Err(e) => {
                        maps_run += 1;
                        skipped.push(format!("{map_id} ({e})"));
                        if !json {
                            println!("SKIP {map_id}: {e}");
                        }
                        false
                    }
                };
                let _ = has_tests;
                let _ = i;
            }
            if json {
                println!(
                    r#"{{"maps_run": {maps_run}, "maps_with_tests": {maps_with_tests}, "tests_passed": {total_pass}, "tests_failed": {total_fail}, "failing_maps": {:?}}}"#,
                    failing_maps
                );
            } else {
                println!(
                    "maps run: {maps_run} (with tests: {maps_with_tests}); tests: {total_pass} passed, {total_fail} failed"
                );
                println!("skipped: {}", skipped.len());
                for s in &skipped {
                    println!("  {s}");
                }
            }
            if total_fail > 0 {
                return Err(CliError::Failures);
            }
        }
        "parse" => {
            if rest.is_empty() {
                return Err(CliError::Usage("usage: parse <file-or-dir>...".into()));
            }
            let mut files = Vec::new();
            for arg in rest {
                collect_isc(Path::new(arg), &mut files);
            }
            files.sort();
            let mut ok = 0usize;
            let mut fail = 0usize;
            for file in &files {
                let source = interscript_rs::locate::read_map(file).map_err(Error::from)?;
                match interscript_rs::grammar::parse_isc(&source) {
                    Ok(_) => {
                        ok += 1;
                        if !json {
                            println!("OK   {}", file.display());
                        }
                    }
                    Err((offset, expected)) => {
                        fail += 1;
                        println!("FAIL {} (offset {offset}, expected {:?})", file.display(), expected);
                    }
                }
            }
            println!("{}/{}/{} files parsed", ok, ok + fail, ok + fail);
            if fail > 0 {
                return Err(CliError::Failures);
            }
        }
        "parse-all" => {
            let root = root_from_rest(rest)?;
            let mut files = Vec::new();
            collect_isc(&root, &mut files);
            files.sort();
            let mut ok = 0usize;
            let mut fail = 0usize;
            for file in &files {
                let source = interscript_rs::locate::read_map(file).map_err(Error::from)?;
                match interscript_rs::grammar::parse_isc(&source) {
                    Ok(_) => ok += 1,
                    Err((offset, expected)) => {
                        fail += 1;
                        println!("FAIL {} (offset {offset}, expected {:?})", file.display(), expected);
                    }
                }
            }
            if json {
                println!(r#"{{"parsed": {ok}, "failed": {fail}}}"#);
            } else {
                println!("{}/{}/{} files parsed", ok, ok + fail, ok + fail);
            }
            if fail > 0 {
                return Err(CliError::Failures);
            }
        }
        "grammar-test" => {
            let (passed, failed, failures) = interscript_rs::grammar::run_grammar_suite();
            println!("grammar suite: {passed} passed, {failed} failed");
            for f in &failures {
                println!("  {f}");
            }
            if failed > 0 {
                return Err(CliError::Failures);
            }
        }
        other => {
            return Err(CliError::Usage(format!("unknown command {other:?}\n\n{}", usage())));
        }
    }
    Ok(())
}

fn list_isc(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_isc(root, &mut out);
    out.sort();
    out
}

/// Corpus root for the sweep commands: `--root DIR` (value follows the
/// flag anywhere in rest) or the bare positional fallback, else the
/// documented default.
fn root_from_rest(rest: &[String]) -> Result<PathBuf, CliError> {
    let mut root: Option<PathBuf> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        if arg == "--root" {
            let v = iter
                .next()
                .ok_or_else(|| CliError::Usage("--root needs a value".into()))?;
            root = Some(PathBuf::from(v));
        } else if root.is_none() && !arg.starts_with('-') {
            root = Some(PathBuf::from(arg));
        }
    }
    Ok(root.unwrap_or_else(|| PathBuf::from("../maps/maps")))
}

// ----------------------------------------------------------------------
// IMF v1 commands (infer / conformance)
// ----------------------------------------------------------------------

/// `iscrs infer <artifact_zip_or_model_id> <string>`: resolve (local
/// path or models.yaml id -> verified cache), greedy single-row
/// decode, print the translated string.
fn cmd_infer(rest: &[String], max_len: usize) -> interscript_rs::Result<()> {
    if rest.len() != 2 {
        return Err(interscript_rs::Error::Io(
            "usage: infer <artifact_zip_or_model_id> <string>".into(),
        ));
    }
    let artifact = resolve_artifact_arg(&rest[0])?;
    println!(
        "# artifact: {}",
        artifact.display()
    );
    let graphs = interscript_rs::imf::loader::verify_and_read(&artifact)?;
    println!(
        "# model {} (decoder: {:?}, precision: {}, opset: {}), members sha256-verified",
        graphs.manifest.id,
        graphs.manifest.decoder,
        graphs.manifest.precision,
        graphs.manifest.opset
    );
    let mut engine = interscript_rs::imf::infer::OrtEngine::from_graphs(&graphs)?;
    let out = interscript_rs::imf::infer::translate(&mut engine, &rest[1], max_len)?;
    println!("{out}");
    Ok(())
}

fn resolve_artifact_arg(arg: &str) -> interscript_rs::Result<PathBuf> {
    let path = Path::new(arg);
    if arg.ends_with(".zip") || path.exists() {
        // Model.load parity: an existing path is used directly.
        return Ok(path.to_path_buf());
    }
    // models.yaml id: index (SECRYST_INDEX / default) + cache/download.
    let index = interscript_rs::imf::registry::load_index(None)?;
    interscript_rs::imf::registry::resolve(arg, &index)
}

/// `iscrs conformance <fixture_file_or_dir> [<artifact_dir>]`: load
/// the frozen vectors, resolve each family's artifact, run every row,
/// report byte-exact pass counts. Exit failure on any mismatch.
fn cmd_conformance(rest: &[String]) -> interscript_rs::Result<()> {
    if rest.is_empty() || rest.len() > 2 {
        return Err(interscript_rs::Error::Io(
            "usage: conformance <fixture_file_or_dir> [<artifact_dir>]".into(),
        ));
    }
    let fixture_path = Path::new(&rest[0]);
    let artifact_dir = rest.get(1).map(|d| PathBuf::from(d));

    // One index load shared across families (registry mode).
    let index = if artifact_dir.is_none() {
        Some(interscript_rs::imf::registry::load_index(None)?)
    } else {
        None
    };

    let mut fixture_files: Vec<PathBuf> = Vec::new();
    if fixture_path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(fixture_path)
            .map_err(|e| interscript_rs::Error::Io(format!("{}: {e}", fixture_path.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        entries.sort();
        fixture_files.extend(entries);
    } else {
        fixture_files.push(fixture_path.to_path_buf());
    }

    let mut families = 0usize;
    let mut skipped = 0usize;
    let mut rows_total = 0usize;
    let mut rows_passed = 0usize;
    let mut any_mismatch = false;
    let mut any_skip = false;

    for file in &fixture_files {
        let vectors = match interscript_rs::imf::conformance::Vectors::load(file) {
            Ok(v) => v,
            Err(e) => {
                println!("SKIP {}: {e}", file.display());
                any_skip = true;
                skipped += 1;
                continue;
            }
        };
        let artifact = match interscript_rs::imf::conformance::resolve_artifact(
            &vectors.model,
            artifact_dir.as_deref(),
            index.as_ref(),
        ) {
            Ok(a) => a,
            Err(e) => {
                println!("SKIP {}: {e}", vectors.model);
                any_skip = true;
                skipped += 1;
                continue;
            }
        };
        // C2: the whole artifact is re-verified against the index
        // digest when the index knows the entry.
        if let Some(index) = &index {
            let canonical = index.canonical_id(&vectors.model);
            if let Some(entry) = index.models.get(&canonical) {
                let actual = interscript_rs::imf::loader::sha256_file(&artifact)?;
                if actual != entry.sha256 {
                    println!(
                        "FAIL {}: artifact sha256 mismatch ({} has {actual}, index says {})",
                        vectors.model,
                        artifact.display(),
                        entry.sha256
                    );
                    any_mismatch = true;
                    continue;
                }
            }
        }
        let graphs = interscript_rs::imf::loader::verify_and_read(&artifact)?;
        let mut engine = interscript_rs::imf::infer::OrtEngine::from_graphs(&graphs)?;
        let result = interscript_rs::imf::conformance::run_conformance(&vectors, &mut engine)?;

        let passed = result.checked - result.mismatches.len();
        families += 1;
        rows_total += result.checked;
        rows_passed += passed;
        println!(
            "{}: {passed}/{} byte-exact  (artifact {})",
            vectors.model,
            result.checked,
            artifact.display()
        );
        for m in &result.mismatches {
            println!(
                "  row {} src {:?}\n    expected {:?}\n    got      {:?}",
                m.index, m.src, m.expected, m.got
            );
        }
        if !result.passed() {
            any_mismatch = true;
        }
    }

    println!(
        "conformance: {families} families, {rows_passed}/{rows_total} rows byte-exact{}",
        if any_skip {
            format!(", {skipped} skipped (artifact not resolvable locally)")
        } else {
            String::new()
        }
    );
    if any_mismatch {
        return Err(interscript_rs::Error::Io(
            "conformance reported mismatches above".into(),
        ));
    }
    if any_skip {
        return Err(interscript_rs::Error::Io(
            "conformance incomplete: skipped families above (fetch their artifacts, \
             then re-run)"
                .into(),
        ));
    }
    Ok(())
}

fn collect_isc(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for entry in entries {
            collect_isc(&entry, out);
        }
    } else if path.extension().and_then(|e| e.to_str()) == Some("isc") {
        out.push(path.to_path_buf());
    }
}

/// `iscrs maps-fetch <version> [--dest DIR]` — download + verify + unpack
/// the raw maps corpus release archive into a usable load path.
fn cmd_maps_fetch(rest: &[String]) -> Result<(), String> {
    let mut version: Option<&str> = None;
    let mut dest = std::path::PathBuf::from("maps-corpus");
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dest" => {
                dest = std::path::PathBuf::from(
                    it.next().ok_or("usage: maps-fetch <version> [--dest DIR]")?,
                )
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => version = Some(other),
        }
    }
    let version = version.ok_or("usage: maps-fetch <version> [--dest DIR]")?;
    let maps_dir = interscript_rs::fetch::fetch_corpus(version, &dest)
        .map_err(|e| format!("{e}"))?;
    println!("fetched maps {version} -> {}", maps_dir.display());
    println!("load path: iscrs --maps {} --maps {} ...", maps_dir.display(), dest.join("libs").display());
    Ok(())
}

/// `iscrs capture [-o FILE] [--root DIR]` — golden transliteration
/// capture: one jsonl row per embedded-test input across the corpus,
/// recording the runtime's actual output. The additivity baseline for
/// ISC-WF-1 (WO16): after a runtime change, re-run and diff — zero
/// delta on non-adopting maps is the merge gate.
fn cmd_capture(rest: &[String]) -> Result<(), String> {
    use std::io::Write as _;

    let mut out_path = std::path::PathBuf::from("golden.jsonl");
    let mut root = std::path::PathBuf::from("../maps/maps");
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" => {
                out_path = std::path::PathBuf::from(
                    it.next().ok_or("capture: -o needs a value")?,
                )
            }
            "--root" => {
                root = std::path::PathBuf::from(
                    it.next().ok_or("capture: --root needs a value")?,
                )
            }
            other => return Err(format!("capture: unknown argument {other}")),
        }
    }

    let maps = list_isc(&root);
    let rt = Runtime::new(load_path_for_root(&root));
    let mut rows = 0usize;
    let mut load_errors = 0usize;
    let mut w = std::io::BufWriter::new(
        std::fs::File::create(&out_path).map_err(|e| e.to_string())?,
    );
    for path in &maps {
        let map_id = path.file_stem().unwrap().to_string_lossy().to_string();
        let doc = match rt.load(&map_id) {
            Ok(d) => d,
            Err(e) => {
                // A load error must be visible: silent counting hid the
                // failing map entirely (WO17 follow-up).
                eprintln!("load error: {map_id}: {e}");
                load_errors += 1;
                continue;
            }
        };
        for (input, _) in &doc.tests.data {
            let map_json = serde_json::to_string(&map_id).unwrap_or_else(|_| "null".into());
            let src_json = serde_json::to_string(input).unwrap_or_else(|_| "null".into());
            let out_json = match rt.transliterate(&map_id, input) {
                Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| "null".into()),
                Err(_) => "null".into(),
            };
            let _ = write!(w, "{{\"map\":{map_json},\"src\":{src_json},\"out\":{out_json}}}\n");
            rows += 1;
        }
    }
    w.flush().map_err(|e| e.to_string())?;
    println!(
        "captured {rows} rows from {} maps ({load_errors} load errors) -> {}",
        maps.len(),
        out_path.display()
    );
    Ok(())
}

fn load_path_for_root(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut lp = vec![root.to_path_buf()];
    if let Some(parent) = root.parent() {
        let libs = parent.join("libs");
        if libs.is_dir() {
            lp.push(libs);
        }
    }
    lp
}
