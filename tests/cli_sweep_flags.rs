//! CLI sweep-command flag handling: `test-all` / `parse-all` document
//! `--root DIR`, and the dispatcher must honor the flag (and not
//! silently sweep zero maps). Runs the built binary against a tiny
//! fixture corpus so the test stays hermetic.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn iscrs() -> &'static str {
    env!("CARGO_BIN_EXE_iscrs")
}

fn fixture_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("iscrs-cli-root-fixture");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("tst-root-flag.isc"),
        "system \"TST:root-flag:Latn:Latn:2026\" {\nstage main {\n  parallel {\n    sub \"a\" \"b\"\n  }\n}\n\ntests {\n  \"a\" -> \"b\"\n}\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn test_all_honors_root_flag() {
    let dir = fixture_dir();
    let out = Command::new(iscrs())
        .args(["test-all", "--root", dir.to_str().unwrap()])
        .env("ISCRS_MAPS", dir.to_str().unwrap())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("maps run: 1"),
        "expected the fixture map to run; stdout: {stdout} stderr: {stderr}"
    );
    assert!(out.status.success(), "stderr: {stderr}");
}

#[test]
fn parse_all_honors_root_flag() {
    let dir = fixture_dir();
    let out = Command::new(iscrs())
        .args(["parse-all", "--root", dir.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1/1/1") || stdout.contains(" 1 files"),
        "expected the fixture file parsed; stdout: {stdout}"
    );
    assert!(out.status.success());
}

#[test]
fn sweep_with_flag_dir_without_maps_reports_zero_not_zero_maps_found() {
    // empty dir: the sweep must report zero maps RUN (not silently
    // conflate a missing --root value with an empty corpus)
    let dir = std::env::temp_dir().join("iscrs-cli-root-empty");
    fs::create_dir_all(&dir).unwrap();
    let out = Command::new(iscrs())
        .args(["test-all", "--root", dir.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("maps run: 0"), "stdout: {stdout}");
    assert!(out.status.success());
}
