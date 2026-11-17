//! WO20 fuzz finding (maps#294): a self-recursive `run stage.<self>`
//! (or a mutual run cycle across stages) had no cycle guard in rs —
//! the recursion ran until the stack overflowed and the process
//! ABORTED (SIGABRT, uncatchable by embedders). The guard mirrors the
//! W1 `use cycle:` named errors: a named `MapLogicError` carrying the
//! run chain, in the run directive's own syntax.

use interscript_rs::{Error, Runtime};

fn write_map(tag: &str, body: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("iscrs-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{tag}.isc")), body).unwrap();
    dir
}

/// `run stage.main` inside `stage main` is a named error, not an abort.
#[test]
fn self_run_cycle_is_a_named_error() {
    let tag = "selfrun";
    let dir = write_map(
        tag,
        "system \"T:fz-selfrun:1\" {\n stage main {\n  parallel { sub \"a\" \"b\" }\n  run stage.main\n }\n}\n",
    );
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime
        .transliterate(tag, "aa")
        .err()
        .expect("self-run must be a named error");
    assert!(
        matches!(err, Error::MapLogic(ref m) if m.contains("run cycle")),
        "got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A two-stage mutual run cycle is caught as well.
#[test]
fn mutual_run_cycle_is_a_named_error() {
    let tag = "mutrun";
    let dir = write_map(
        tag,
        "system \"T:fz-mutrun:1\" {\n stage main {\n  run stage.a\n }\n stage a {\n  sub \"x\" \"y\"\n  run stage.b\n }\n stage b {\n  sub \"y\" \"x\"\n  run stage.a\n }\n}\n",
    );
    let runtime = Runtime::new(vec![dir.clone()]);
    let err = runtime
        .transliterate(tag, "x")
        .err()
        .expect("mutual run cycle must be a named error");
    assert!(
        matches!(err, Error::MapLogic(ref m) if m.contains("run cycle")),
        "got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A stage may still be re-run through a NON-cyclic chain (a -> b -> a
/// is a cycle, but a -> b from two different callers is fine): a run
/// whose target is already COMPLETED on a finished branch must not be
/// rejected. Pin: a diamond (main runs a and b; both run common) works.
#[test]
fn non_cyclic_shared_runs_still_work() {
    let tag = "diamondrun";
    let dir = write_map(
        tag,
        "system \"T:diamond:1\" {\n stage common {\n  sub \"1\" \"2\"\n }\n stage a {\n  sub \"a\" \"A\"\n  run stage.common\n }\n stage b {\n  sub \"b\" \"B\"\n  run stage.common\n }\n stage main {\n  run stage.a\n  run stage.b\n }\n}\n",
    );
    let runtime = Runtime::new(vec![dir.clone()]);
    let out = runtime.transliterate(tag, "ab1").expect("diamond runs");
    assert_eq!(out, "AB2");
    std::fs::remove_dir_all(&dir).ok();
}
