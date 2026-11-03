//! The embedded ISC grammar: `grammar/isc.parg` (vendored verbatim
//! from interscript-ruby, see README provenance) compiled once through
//! parsanol's portable backend, plus the parse entry point that
//! resolves the engine's arena-backed tree into the owned
//! parslet-shaped value tree (`tree::TNode`).

use crate::tree::{resolve, TNode};
use parsanol::parg::frontend::compile::{compile_entry_mode, Tables};
use parsanol::parg::frontend::parser::parse_grammar_with_mode;
use parsanol::portable::parslet_transform::to_parslet_compatible;
use parsanol::portable::Grammar;
use std::sync::OnceLock;

/// Vendored verbatim from interscript-ruby@34bd8829 `grammar/isc.parg`
/// (introduced in WO13 commit 873caf2; blob c9d4ffc8).
pub const ISC_PARG: &str = include_str!("../grammar/isc.parg");

fn compiled() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        let (parsed, mode) = parse_grammar_with_mode(ISC_PARG)
            .expect("embedded grammar/isc.parg must parse");
        compile_entry_mode(
            &parsed.document,
            "isc",
            &Tables::new(),
            &parsed.atomic_rules,
            mode.as_deref(),
        )
        .expect("embedded grammar/isc.parg must compile")
    })
}

/// Parse an ISC map source into the parslet-shaped value tree the
/// Ruby-side `Interscript::Isc::DocumentBuilder` consumes.
pub fn parse_isc(source: &str) -> Result<TNode, (usize, Vec<String>)> {
    let grammar = compiled();
    let mut arena = parsanol::portable::AstArena::for_input(source.len().max(1 << 16));
    let mut parser = parsanol::portable::PortableParser::new(grammar, source, &mut arena);
    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(_) => {
            let wire = match parser.failure_wire() {
                Some((offset, expected)) => (offset, expected),
                None => (source.len(), Vec::new()),
            };
            return Err(wire);
        }
    };
    let shaped = to_parslet_compatible(&ast, &mut arena, source);
    Ok(resolve(&shaped, &arena, source))
}

/// Run the accept/reject suite embedded in `grammar/isc.parg`
/// (`test isc { accept ... / reject ... }`).
pub fn run_grammar_suite() -> (usize, usize, Vec<String>) {
    let (parsed, mode) = parse_grammar_with_mode(ISC_PARG).expect("grammar parses");
    let grammar = compile_entry_mode(
        &parsed.document,
        "isc",
        &Tables::new(),
        &parsed.atomic_rules,
        mode.as_deref(),
    )
    .expect("grammar compiles");

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures = Vec::new();
    for test in &parsed.document.tests {
        let mut arena = parsanol::portable::AstArena::for_input(test.input.len().max(1 << 12));
        let mut parser = parsanol::portable::PortableParser::new(&grammar, &test.input, &mut arena);
        let ok = parser.parse().is_ok();
        let wanted = test.kind == "accept";
        if ok == wanted {
            passed += 1;
        } else {
            failed += 1;
            failures.push(format!("{} {:?} (parsed={ok})", test.kind, test.input));
        }
    }
    (passed, failed, failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_suite_passes() {
        let (passed, failed, failures) = run_grammar_suite();
        assert_eq!(failed, 0, "failures: {failures:?}");
        assert!(passed >= 8, "expected the parg suite, got {passed} passes");
    }

    #[test]
    fn minimal_map_parses() {
        let src = r#"system "T:a-b:c-d:1" { stage main { parallel { sub "a" "b" } } }"#;
        assert!(parse_isc(src).is_ok());
    }

    #[test]
    fn truncated_map_fails_with_wire() {
        let src = r#"system "T:a-b:c-d:1" {"#;
        let err = parse_isc(src).unwrap_err();
        assert!(err.1.iter().any(|e| e.contains("metadata") || e.contains("stage") || e.contains('"')), "{err:?}");
    }
}
