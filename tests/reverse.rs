//! `Document::reverse` parity tests — every expectation below was
//! generated with `interscript-ruby` origin/main (fresh clone,
//! `Interscript::Interpreter.parse_isc` + `Document#reverse` +
//! `Interpreter.call(rdoc)`), the conformance-proven runtime. Where
//! Ruby and this implementation disagree, Ruby wins.
//!
//! Ground-truth generation (2026-11-04, ruby 3.4.8):
//! ```ruby
//! doc = Interscript::Interpreter.parse_isc(map)
//! rdoc = doc.reverse
//! Interscript::Interpreter.call(rdoc).call(input)
//! ```

use interscript_rs::interpreter::{execute_stage_pub, Runtime};
use std::path::PathBuf;

fn reverse_maps() -> Vec<PathBuf> {
    vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reverse_maps")]
}

/// Reverse a map's main stage and transliterate `input` through it.
fn reverse_run(map_id: &str, input: &str) -> String {
    let rt = Runtime::new(reverse_maps());
    let doc = rt.load(map_id).expect("map loads");
    let reversed = interscript_rs::reverse::reverse_document(&doc).expect("reverse");
    execute_stage_pub(&reversed, "main", input, None).expect("reverse stage executes")
}

#[test]
fn reverse_name_swaps_source_and_target() {
    use interscript_rs::reverse::reverse_name;
    // Document.reverse_name: >= 4 dash-parts swaps [2]/[3].
    assert_eq!(reverse_name("alalc-bul-Cyrl-Latn-1997"), "alalc-bul-Latn-Cyrl-1997");
    // the swap is an involution: swapping back reproduces the input.
    assert_eq!(reverse_name("alalc-bul-Latn-Cyrl-1997"), "alalc-bul-Cyrl-Latn-1997");
    // shorter names strip -reverse, else append it.
    assert_eq!(reverse_name("some-map-reverse"), "some-map");
    assert_eq!(reverse_name("short-x"), "short-x-reverse");
    assert_eq!(reverse_name("a-b-c"), "a-b-c-reverse");
}

#[test]
fn parallel_rules_swap_and_reverse_order() {
    // Ruby: "phth" -> "phthh", "x" -> "x", "fats" -> "phaths".
    // (reversed rules ks->x, t->th, f->ph; "fats" hits t then f)
    assert_eq!(reverse_run("rev-parallel", "phth"), "phthh");
    assert_eq!(reverse_run("rev-parallel", "x"), "x");
    assert_eq!(reverse_run("rev-parallel", "fats"), "phaths");
}

#[test]
fn sequential_children_reverse_order() {
    // forward: ab->X then X->Q; reversed: Q->X then X->ab.
    assert_eq!(reverse_run("rev-sequence", "Q"), "ab");
    assert_eq!(reverse_run("rev-sequence", "X"), "ab");
    assert_eq!(reverse_run("rev-sequence", "ab"), "ab");
    assert_eq!(reverse_run("rev-sequence", "abq"), "abq");
}

#[test]
fn case_swap_downcases_the_from_tree() {
    // forward: sub "abc" to upcase; downcase.
    // reversed: title_case, then sub "abc" to downcase.
    // Ruby: "abc" -> "Abc", "abc ABC" -> "Abc ABC".
    assert_eq!(reverse_run("rev-case", "abc"), "Abc");
    assert_eq!(reverse_run("rev-case", "abc ABC"), "Abc ABC");
}

#[test]
fn boundary_constraints_pass_through_unchanged() {
    // forward: sub "x" "y" after boundary; sub "q" "r" before line_end.
    // Ruby reverse keeps the constraints on the same side: "r y" -> "r x"
    // (y->x fires after the boundary before end-of-string).
    assert_eq!(reverse_run("rev-boundary", "r y"), "r x");
    assert_eq!(reverse_run("rev-boundary", "yq"), "yq");
    assert_eq!(reverse_run("rev-boundary", "qr"), "qr");
}

#[test]
fn capture_and_ref_exchange_sides() {
    // forward: from "a" + capture("b" + maybe("c")) + "d" to "X" + ref(1)
    // reversed: from "X" + capture("b" + maybe("c")) to "a" + ref(1).
    assert_eq!(reverse_run("rev-capture", "Xbc"), "abcd");
    assert_eq!(reverse_run("rev-capture", "Xd"), "Xd");
    assert_eq!(reverse_run("rev-capture", "abcd"), "abcd");
}

#[test]
fn funcalls_invert_through_the_reverse_function_map() {
    // forward: separate separator "-"; compose.
    // reversed: decompose, then unseparate separator "-".
    assert_eq!(reverse_run("rev-funcall", "a-b-c"), "abc");
    assert_eq!(reverse_run("rev-funcall", "abc"), "abc");
}

#[test]
fn dependency_documents_reverse_with_the_parent() {
    // dep main: n->p, e->f. forward parent: m->n then run dep.
    // reversed parent: run reversed-dep (p->n, f->e) then n->m.
    // Ruby: "men" -> "mem", "pem" -> "mem".
    assert_eq!(reverse_run("rev-dep", "men"), "mem");
    assert_eq!(reverse_run("rev-dep", "pem"), "mem");
}

#[test]
fn real_corpus_map_reverses_like_ruby() {
    // Vendored alalc-bul-Cyrl-Latn-1997.isc; Ruby ground truth:
    assert_eq!(reverse_run("alalc-bul-Cyrl-Latn-1997", "Kirillicata"), "Кириллиcата");
    assert_eq!(reverse_run("alalc-bul-Cyrl-Latn-1997", "zapad"), "запад");
    assert_eq!(reverse_run("alalc-bul-Cyrl-Latn-1997", "Sofiya"), "Софиyа");
}

#[test]
fn empty_reversed_from_disables_the_rule() {
    // sub "h" none (deletion): the reversed from is empty -> Ruby
    // marks it reverse_run: true so the executor skips it; a
    // forward-deletion map must not infinite-expand on reverse.
    // Ruby ground truth: "hh" -> "hh", "hi" -> "hi".
    assert_eq!(reverse_run("rev-delete", "hh"), "hh");
    assert_eq!(reverse_run("rev-delete", "hi"), "hi");
}

#[test]
fn upcase_funcall_reverse_is_unsupported_like_ruby() {
    // The Ruby reverse_function map has no :upcase entry — reversing a
    // funcall upcase crashes there (nil.to_sym TypeError when the
    // reversed funcall executes). We raise cleanly at reverse time.
    let rt = Runtime::new(reverse_maps());
    let doc = rt.load("rev-upcase-funcall").expect("map loads");
    let err = match interscript_rs::reverse::reverse_document(&doc) {
        Err(e) => e,
        Ok(_) => panic!("upcase funcall reverse must be rejected"),
    };
    assert!(err.to_string().contains("upcase"), "{err}");
}
