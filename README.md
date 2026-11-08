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
- ISC-WF-1 workflow extension (docs/isc-wf-1.md): W1 named rulesets
  (`ruleset` / `use`, load-time recursive expansion, cycles are load
  errors), W2 `use <ident> [alias.<ruleset>] override { ... }`
  (child-wins on exact from-equality, kept-in-order then overrides),
  and W3 language conditioning (`languages` declaration, `when`
  guards, `transliterate_lang` / CLI `--language`; guards inert
  without a selection, guard-applicable beats unguarded same-from),
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
tests/corpus/         vendored semantics maps (Ruby-verified expectations;
                      rs-wf-*.isc pin the ISC-WF-1 constructs)
tests/semantics.rs    semantics suite
tests/wf1.rs          ISC-WF-1 suite (expansion/merge/guard semantics)
tests/corpus_sweep.rs corpus integration (sibling maps checkout)
```

Dependencies: `parsanol` (path pin — the same local checkout the
interscript-ruby `tools/isc-parg-check` harness uses, until a
crates.io release exists), `unicode-normalization` (compose/decompose
= NFC/NFD, 79 corpus maps), `unicode-general-category` (Onigmo's
`\b` word-char table: Word = Alphabetic|Mark|Nd|Pc — `std` exposes no
general categories). Each is justified by a runtime-semantics
requirement, not convenience. Slice 2 adds the IMF v1 layer:
`sha2` (SPEC.md C2/C3 digests), `zip` + `serde`/`serde_yaml` +
`serde_json` (artifact zips, metadata/models.yaml, the frozen JSON
vectors), `ureq` (SPEC §2 download channel; rustls), and `ort`
(ONNX Runtime 2.0.0-rc.15, prebuilt ms@1.31.0 binaries; macOS needs
the `.cargo/config.toml` -U link workaround for CoreML EP symbols —
see that file).

## Grammar provenance

`grammar/isc.parg` is copied verbatim from
`interscript/interscript-ruby@34bd8829` `grammar/isc.parg` (WO13
commit `873caf2`, blob `c9d4ffc8`), then extended in place with the
ISC-WF-1 constructs (ruleset / use / override / languages / when,
spec `docs/isc-wf-1.md` in the maps repo). The extension is
parse-additive: no corpus map uses the four new keywords at bare
identifier positions, verified against the 404-map corpus. The
embedded `test isc { accept ... reject ... }`
suite runs under `iscrs grammar-test` and in `cargo test`.

## CLI

```
iscrs transliterate <map_id> <string> [--stage NAME] [--language LANG]
                                                       # Interscript.transliterate parity
iscrs reverse <map_id> <string> [--stage NAME]         # run the reversed document (Document#reverse)
iscrs test <map_id>...                                 # run embedded tests
iscrs test-all [--root DIR] [--limit N] [--json]       # corpus sweep
iscrs parse <file-or-dir>...                           # parse-only
iscrs parse-all [--root DIR] [--json]                  # parse sweep
iscrs grammar-test                                     # the .parg embedded suite
iscrs infer <artifact_zip_or_model_id> <string>        # IMF v1 greedy decode (ort)
iscrs conformance <fixture_file_or_dir> [<artifact_dir>]  # frozen vectors, byte-exact
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

## Status (slice 2, `feat/interscript-rs-slice2`)

- Slice 1 holds: `grammar-test` 8/8; `parse-all ../maps/maps` 401/401;
  `test-all` 10 772 passed / 29 failed (the same 9 maps fail the same
  tests under interscript-ruby).
- **IMF v1 loader** (`src/imf/loader.rs`): zip member verification per
  SPEC.md §3/C3 — format/tokenizer rejection, every `.onnx` member
  checked against the manifest sha256 map, uncovered members refused.
  `src/imf/registry.rs` implements §1/§2/C1/C2: `models.yaml` index
  (`version: 1`, `SECRYST_INDEX`), WO87 rename aliases, cache hits
  re-verified, downloads (single URL, `file://` mirrors, split parts
  verified as they land) installed atomically into `SECRYST_CACHE`.
- **Inference** (`src/imf/infer.rs`): the single-row greedy contract
  (CONFORMANCE.md) — byte+3 ids with trailing EOS, one-row encoder
  calls, greedy argmax until EOS with the 512-step cap — over the
  mockable `InferenceEngine` trait; `ort` sessions implement it.
- **Conformance harness** (`iscrs conformance`): the frozen fixtures
  in `interscript-models/conformance/` run byte-exact:
  - `khm-phonemic-1.0`: **12/12**
  - `tha-phonemic-1.0`: **12/12**
  - `tha-g2p-lex-1.0`: **12/12** (pre-rename id; same zip, resolved by
    its metadata id)
  fas/urd fixtures reference fp16/larger artifacts not in the local
  cache; the same command resolves them via the index + cache (and
  would download per §2) — not run here.
- **Reverse chains** (`src/reverse.rs`): `Document#reverse` parity for
  the slice-1 subset — stage/rule order reversal, `to upcase`/`to
  downcase` from-tree case swap, boundary-alias hoisting into `to`,
  capture/ref exchange, kor_maybedash, funcall inversion via
  `reverse_function`, `reverse_name`, empty-from rules marked
  reverse_run and skipped by the executor. Every expectation in
  `tests/reverse.rs` was generated with interscript-ruby origin/main
  (fresh clone); Ruby wins. `upcase` funcall reverse is a clean
  Unsupported (the Ruby map omits it and crashes).

## Conformance plan

Slice 1 pinned semantics against the Ruby interpreter directly (the
conformance-proven runtime): every vendored test expectation in
`tests/corpus/` was verified against `interscript-ruby` before being
committed, and the corpus sweeps run the maps corpus's own embedded
tests.

Slice 2 binds the runtime to BOTH fixture layers: the frozen ISC-map
semantics (slice 1, `tests/corpus/`) and the frozen IMF model vectors
(`interscript-models/conformance/`, byte-exact through the ort
inference path — counts above). Reviewer commands:

```
cargo run --release -- conformance \
  ../interscript-models/conformance/khm-phonemic-vectors.json \
  ~/.cache/secryst/models
cargo run --release -- conformance \
  ../interscript-models/conformance/tha-phonemic-vectors.json \
  ~/.cache/secryst/models
cargo run --release -- infer \
  ~/.cache/secryst/models/khm-phonemic-1.0/khm-phonemic-1.0.zip "ភាសាវិទូ"
```

Artifacts are never committed; they resolve from `SECRYST_CACHE`
(`~/.cache/secryst`, the shared layout) or an explicit artifact dir,
sha256-verified against `models.yaml` before use.

## Known gaps (slice 2)

- **Batched/beam decode and KV-cache decode are not implemented** —
  the artifact contract is single-row greedy (WO51), and both fixture
  families are `decoder: plain`; `decoder-kv.onnx` members load but
  decode through the plain graph.
- fp16 artifacts load and verify but the greedy loop reads logits as
  f32; fp16 graph outputs cast on extraction — fas/urd fp16 families
  unrun locally (artifacts not cached).
- `iscrs infer` decodes with the SPEC §4 token table; the conformance
  harness uses the fixture generator's byte rule (`max(0, t-3)`); on
  the proven families the two agree (no pad/unk argmax observed).
- Reverse: sub-rule `priority:` is not modeled (the slice-1 Sub has no
  priority field); reverse of `repeat` items in to-upcase/downcase
  subs and of boundary hoisting that consumes a whole from-group
  raise Unsupported where Ruby crashes (NoMethodError).
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
