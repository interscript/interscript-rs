# interscript-rs

Rust runtime for [Interscript](https://github.com/interscript) ISC
maps — slice 1 of WO14 (the rust leg of the runtime matrix
rust+ruby+python+ts on parsanol v2).

Parses `.isc` maps through the canonical parsanol grammar
(`grammar/isc.parg`, vendored verbatim from interscript-ruby) and
executes the standard map subset end-to-end:

- parallel stages — both Ruby paths are mirrored: the constraint-free
  tree path (a last-wins char trie, longest match first) and the
  megaregexp fallback (deterministic max-length ordering, first match
  wins) — plus sequential sub rules,
- sub rules with `before` / `after` / `not_before` / `not_after`
  boundary constraints (`before` = lookbehind, `after` = lookahead —
  Ruby `Interscript::Interpreter` parity, including the empty-constraint
  corner: `before ""` is a no-op, `not_before ""` disables the rule),
- `any(...)` classes: string sets, codepoint ranges, nested
  composition, alias references,
- zero-width primitives: `boundary` (Onigmo Unicode-aware `\b`),
  `line_start`, `line_end`, `word_boundary`, plus ASCII `\w`-class
  aliases,
- `capture(...)` groups with `ref(N)` backreferences, numbered in
  regex order after alias expansion,
- alias/dependency resolution: unaliased dependencies are library
  imports (aliases merge into scope); aliased dependencies are
  stage-run targets (`run map.<dep>.stage.<stage>`),
- embedded per-map tests, and the stage funcalls `upcase`, `downcase`,
  `title_case` (with `word_separator:` kwarg), `separate`,
  `compose` / `decompose` (NFC / NFD).

## Layout

```
grammar/isc.parg      vendored grammar (provenance below)
src/grammar.rs        parg compile + parse entry (parsanol portable engine)
src/tree.rs           parslet-shaped value tree (arena resolution)
src/builder.rs        tree -> Document (Normalizer+Transform+DocumentBuilder+NodeAdapter parity)
src/document.rs       Document model (stages, sub rules, dependencies)
src/item.rs           Item model (Node::Item parity)
src/stdlib.rs         Stdlib parity: primitives, boundary, funcalls
src/interpreter.rs    backtracking matcher + stage executor
src/locate.rs         map resolution over the load path
src/bin/iscrs.rs      CLI
tests/corpus/         vendored semantics maps (Ruby-verified expectations)
tests/semantics.rs    semantics suite
tests/corpus_sweep.rs corpus integration (sibling maps checkout)
```

Dependencies: `parsanol` (path pin — the same local checkout the
interscript-ruby `tools/isc-parg-check` harness uses, until a
crates.io release exists), `unicode-normalization` (compose/decompose
= NFC/NFD, 79 corpus maps), `unicode-general-category` (Onigmo's
`\b` word-char table: Word = Alphabetic|Mark|Nd|Pc — `std` exposes no
general categories). Each is justified by a runtime-semantics
requirement, not convenience.

## Grammar provenance

`grammar/isc.parg` is copied verbatim from
`interscript/interscript-ruby@34bd8829` `grammar/isc.parg` (WO13
commit `873caf2`, blob `c9d4ffc8`). It is the canonical grammar the
Ruby runtime ships; do not edit it here — upstream changes re-vendor
from the Ruby repo. The embedded `test isc { accept ... reject ... }`
suite runs under `iscrs grammar-test` and in `cargo test`.

## CLI

```
iscrs transliterate <map_id> <string> [--stage NAME]   # Interscript.transliterate parity
iscrs test <map_id>...                                 # run embedded tests
iscrs test-all [--root DIR] [--limit N] [--json]       # corpus sweep
iscrs parse <file-or-dir>...                           # parse-only
iscrs parse-all [--root DIR] [--json]                  # parse sweep
iscrs grammar-test                                     # the .parg embedded suite
```

Load path: `--maps DIR` (repeatable), else `$ISCRS_MAPS`
(colon-separated), else the sibling Interscript checkout
(`../maps/maps` + `../maps/libs`). Resolution order per directory is
`.isc` then `.iml` then `.imp` — `Interscript.locate` parity;
legacy `.imp` maps are rejected with a clear error.

## Reviewer commands

```
cargo test                                   # vendored suite (hermetic)
cargo run -- grammar-test                    # parg accept/reject suite
cargo run --release -- parse-all ../maps/maps --json
cargo run --release -- test-all  ../maps/maps --json
cargo run --release -- transliterate alalc-bul-Cyrl-Latn-1997 "Кириллица Latin тест"
cargo run --release -- test alalc-ell-Grek-Latn-1997 alalc-ara-Arab-Latn-1997 din-san-Deva-Latn-33904-2018
```

## Status (slice 1, `feat/interscript-rs-slice1`)

- `grammar-test`: 8/8 embedded parg suite cases pass.
- `parse-all ../maps/maps`: **401/401** files parsed — same total as the
  interscript-ruby `tools/isc-parg-check` baseline (401/401 over the
  same corpus with the same grammar). A deterministic 201-file sample
  (`every other file`) parses 201/201 under both.
- `test-all ../maps/maps`: **10 772 embedded tests passed, 29 failed**
  across 396 maps-with-tests. All 9 failing maps (alalc-ban, alalc-bod,
  alalc-khm, alalc-lao, alalc-lif, alalc-mni, alalc-mya, bgnpcgn-tha,
  din-fas) fail the SAME tests under interscript-ruby (verified
  per-map; e.g. the Thai map's comment records the pending WO80
  capture-reorder semantics its tests are written for). 387 maps pass
  100% of their tests.
- The vendored `tests/corpus/` maps (boundary before/after, empty
  constraints, any/negation, capture-ref, parallel longest-match and
  duplicate-key tree semantics, funcall chain, library import, aliased
  stage run, nested any) are all Ruby-verified.

## Conformance plan

Slice 1 pins semantics against the Ruby interpreter directly (the
conformance-proven runtime): every vendored test expectation in
`tests/corpus/` was verified against `interscript-ruby` before being
committed, and the corpus sweeps run the maps corpus's own embedded
tests.

Slice 2 binds the runtime to the frozen conformance fixtures in
`interscript-models/conformance/` (the fixtures DECIDE runtime
equivalence): a fixture loader reads the frozen input/expected pairs
and a `conformance` test runs the whole set byte-identical, driven by
`interscript-models/models.yaml` (index-v12) resolution so map ids and
artifact pins come from the registry rather than a hardcoded list.

Model inference (the WO14 b-leg) lands on top: load IMF v1 artifacts
per `interscript-models/SPEC.md` — `encoder.onnx` +
`decoder-kv.onnx` + `metadata.yaml`, sha256-verified against
`models.yaml` — through the `ort` crate (ONNX Runtime), ByT5 byte+3
tokenizer with trailing EOS (id 1; raw `text.bytes` is the known
silent-garbage trap), greedy + KV-cache decode. Parity gate before
shipping: >= 500 samples single-row greedy vs the Ruby/TS runtimes,
batched decode only for masked-encoder exports. NOT in slice 1 — no
ort code exists in this tree yet.

## Known gaps (slice 1)

- **Reverse chains are not implemented** (no `Document#reverse`,
  no reverse rule execution). Reverse semantics live in the Ruby
  `Node#reverse` layer and are not exercised by the ISC corpus or
  parsanol; guessing them here would violate the "fixtures decide"
  law. Deferred until the conformance fixtures require them.
- `rababa` / `secryst` funcalls need external ML engines: clean
  `Unsupported` errors.
- The `upcase` stage directive parses (the grammar accepts it) but is
  rejected at runtime — the Ruby runtime crashes on it too
  (`Stdlib::Functions` has no `upcase`).
- Metadata is extracted best-effort (it is inert for
  transliteration).
- Legacy `.imp` maps (Ruby-DSL) are out of scope; `.isc`/`.iml` only.
- `--stage` runs a whole named stage; the `each:` enumerator mode of
  the Ruby interpreter (Fiber-based ambiguity enumeration) is not
  implemented.
