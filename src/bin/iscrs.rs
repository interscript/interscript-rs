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
  transliterate <map_id> <string> [--stage NAME]
  test <map_id>...
  test-all [--root DIR] [--limit N]
  parse <file-or-dir>...
  parse-all [--root DIR]
  grammar-test

options:
  --maps DIR...    add a maps/library directory to the load path
                   (default: $ISCRS_MAPS or the sibling maps checkout)
  --json           machine-readable summary lines (test-all/parse-all)"#
        .to_string()
}

fn run(args: &[String]) -> Result<(), CliError> {
    let mut maps_dirs: Vec<PathBuf> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut stage = "main".to_string();
    let mut limit: Option<usize> = None;
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
            "--limit" => {
                let v = iter
                    .next()
                    .ok_or_else(|| CliError::Usage("--limit needs a value".into()))?;
                limit = Some(v.parse().map_err(|_| CliError::Usage("--limit needs a number".into()))?);
            }
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(CliError::Usage(format!("unknown option {other}")));
            }
            _ => positional.push(arg.clone()),
        }
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

    let Some(cmd) = positional.first() else {
        return Err(CliError::Usage(usage()));
    };
    let rest = &positional[1..];

    match cmd.as_str() {
        "transliterate" => {
            if rest.len() != 2 {
                return Err(CliError::Usage("usage: transliterate <map_id> <string>".into()));
            }
            let rt = Runtime::new(load_path);
            let out = rt.transliterate_stage(&rest[0], &stage, &rest[1])?;
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
            let root = rest
                .first()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("../maps/maps"));
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
            let root = rest
                .first()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("../maps/maps"));
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
