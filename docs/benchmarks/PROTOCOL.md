# Interscript 4-Runtime Benchmark Protocol

Status: protocol + first baselines (2026-11). Optimizations are explicitly
out of scope for this lane; this document fixes **how** the four runtimes are
measured so any future number is comparable.

## Runtimes under test

| Runtime | Repo | Registry path measured |
| --- | --- | --- |
| Rust (`interscript` crate) | interscript/interscript-rs | `Runtime::new([maps/, libs/])` + `Runtime::load` (parsanol-backed compiler) |
| TypeScript (`interscript` npm) | interscript/interscript-ts | `configure({ strategies: [filesystemStrategy(dir)] })` + `loadMap` over the JSON-IR artifact |
| Ruby (`interscript` gem) | interscript/interscript-ruby | `Interscript.load_path` default `["."]` against a flat dir (systems + libs symlinked side by side) → `Interscript.load` (parsanol-backed interpreter). The module exposes no `load_path=` writer, so the fixture dir is the cwd; the locate/parse/compile path is unchanged. |
| Python (`interscript` PyPI) | interscript/interscript-py | `add_load_path([maps/, libs/])` + `load_map` |

All four runtimes read the **same maps corpus at the same commit** so
registry contents are identical across runs.

## Corpus pinning

- Maps corpus: `interscript/maps`, commit
  `9cfbf4cf2221910bb68492e736c07ac9cdc1a38f` (457 system maps + libs).
- The corpus checkout is used two ways:
  - `.isc` sources directly (rs / ruby / py: their native load path);
  - compiled JSON-IR artifact for ts, built from the same commit with
    `maps/scripts/build-json-ir-artifact.mjs` (the canonical builder).
- Benchmark input set: 200 documents extracted deterministically from
  `interscript-rs/tests/golden/isc-wf0.jsonl` by
  [`extract_bench_inputs.py`](./extract_bench_inputs.py) (seed `42`,
  map-id-hash order, round-robin 1 doc/map/pass → 200 docs over 200 distinct
  maps, 5,478 chars total). The generated `bench-inputs.json` is a local
  fixture — it is not committed to any repository.

## Measurements

Per runtime, three process invocations (phases), each emitting one JSON
object on stdout (`runners/<runtime>-bench.*`, uniform interface:

    <runner> --phase load|throughput|peak --inputs bench-inputs.json [--loops 5]

1. **Cold load (a)** — fresh process, empty registry, load **all** corpus
   system maps (`maps/*.isc`, 457) through the runtime's standard registry
   path, dependency closure included. Reports `attempted`, `loaded`,
   `failed[]` (map ids that error on load) and `cold_load_ms` (monotonic
   wall clock around the whole load loop). Libraries load on demand as
   dependencies and are not counted in `attempted`.
2. **Warm throughput (b)** — fresh process, load only the maps referenced by
   the input set (plus dependency closure), then transliterate the 200 docs
   `N=5` loops over the pre-warmed registry (map load cost amortised out).
   Reports `docs_per_sec` (200*5 / wall) and `chars_per_sec`
   (total source chars * 5 / wall), plus `errors` (transliteration calls
   that raised).
3. **Single-largest-doc peak (c)** — fresh process, load one map (the input
   set's largest document: `un-ell-Grek-Latn-1987-ts`, 701 chars) and
   transliterate it once. Reports `wall_ms`.

**Peak RSS (d)** is captured for every phase externally: the runner is
executed under `/usr/bin/time -l` and `maximum resident set size` (bytes on
macOS) is recorded per phase. This measures the whole interpreter process —
interpreter baseline + loaded registry + working set — consistently across
all four runtimes. Report peak RSS as **MB (2^20 bytes)**, one decimal.

Every runner also reports `commit` (its own repo's HEAD sha) and
`version` (crate / package / gem / pyproject version), so numbers are
traceable.

## Recorded environment (fixed context)

- Machine: Apple MacBook Pro, **Apple M1 Max**, 10 cores, **64 GB RAM**,
  macOS 14.1.1 (Darwin 23.1.0). Single run, foreground, no other benchmark
  load; the machine was otherwise idle.
- Toolchains at measurement time:
  - rustc/cargo 1.98.0 (Homebrew), crate built `--release`.
  - node v24.14.0 / npm 11.9.0; ts lib built with `tsc` (`npm run build`).
  - ruby 4.0.7 (2026-09-15) +PRISM arm64-darwin23; run via
    `ruby -Ilib <runner>` against the repo tree (no bundler).
  - python 3.10.5; run via `python3 <runner>` with `sys.path` pointing at
    the repo `src/` tree (no install, no venv).

## Reproducing

From a workspace with the five repos cloned side by side
(`interscript-rs`, `interscript-ts`, `interscript-ruby`, `interscript-py`,
`maps`):

1. `python3 docs/benchmarks/extract_bench_inputs.py \
     ../interscript-rs/tests/golden/isc-wf0.jsonl bench-inputs.json 200 42`
2. Build each runtime (rs: `cargo build --release --example rs-bench`;
   ts: `npm ci && npm run build`, then the JSON-IR artifact with the maps
   repo's canonical builder exactly as its CI runs it —
   `npx -y tsx scripts/build-json-ir-artifact.mjs <interscript-ts-dir> <out-dir>`;
   ruby: none — prepare the flat load dir by symlinking every
   `maps/*.isc` and `libs/*.isc` side by side into one fixture directory).
3. For each runtime and phase (`load`, `throughput`, `peak`):
   `/usr/bin/time -l <runner-cmd> 2> rss.txt` — parse the runner's stdout
   JSON and `maximum resident set size` from `rss.txt`.
4. Tabulate: cold-load s / peak-RSS MB (load phase) / docs-per-sec /
   chars-per-sec / peak-RSS MB (peak phase).

## Threats to validity / notes

- Cold-load wall time includes dependency closure: maps that `use` libraries
  pull them in on first load; the count of attempted/loaded is recorded so
  cross-runtime deltas in load-failure sets stay visible.
- The ts registry path measures the JSON-IR artifact (parse happened at
  artifact build time); rs/ruby/py parse `.isc` at load time. This is each
  runtime's **standard** registry path — the asymmetry is inherent to the
  current pipelines and is recorded, not papered over.
- py/ruby/rust interpreter baselines differ; RSS deltas across runtimes
  include that baseline. Cross-runtime RSS comparisons are indicative only;
  per-runtime **trends over time** are the primary signal.
- Transliteration `errors` are counted, not retried; per-map failures are
  listed in the load phase so a runtime silently skipping maps shows up.
