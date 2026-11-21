# Interscript Runtime Benchmarks — First Baselines (2026-11)

Protocol: [PROTOCOL.md](./PROTOCOL.md) (measure definitions, corpus pinning,
runner interface, RSS method). This file records the **first baseline run** —
harness + numbers only; optimizations are a follow-up lane and none are
claimed or attempted here.

## Run identity

| | |
| --- | --- |
| Date | 2026-11-21 |
| Machine | Apple MacBook Pro — Apple M1 Max, 10 cores, 64 GB RAM, macOS 14.1.1 |
| Maps corpus | interscript/maps @ `9cfbf4cf2221910bb68492e736c07ac9cdc1a38f` (457 system maps + libs) — same commit for all four runtimes |
| Input set | 200 docs, 200 distinct maps, 5,478 chars — deterministic extraction (seed 42) from `tests/golden/isc-wf0.jsonl` (16,322 rows, 453 maps) via [extract_bench_inputs.py](./extract_bench_inputs.py) |
| Rust | interscript-rs @ `e3d7388b` — crate 0.3.5 — rustc/cargo 1.98.0, `--release` |
| TypeScript | interscript-ts @ `8323201c` — npm 5.15.1 — node v24.14.0 |
| Ruby | interscript-ruby @ `dd5708b5` — gem 3.7.4 — ruby 4.0.7 (PRISM), `ruby -Ilib`, parsanol gem 1.5.13 |
| Python | interscript-py @ `6d29cb7c` — 0.2.1 — python 3.10.5, repo `src/` tree |
| Single run, sequential | phases executed one runtime at a time on an otherwise idle machine; numbers are one-shot baselines, not medians |

## Baseline table

| Runtime | Cold load (s) | Loaded | RSS @ load (MB) | Docs/sec | Chars/sec | RSS @ throughput (MB) | Peak doc (ms) | RSS @ peak (MB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust (interscript-rs) | 9.55 | 456/457 | 342.7 | 145.1 | 3,975 | 406.2 | 318 | 16.2 |
| TypeScript (interscript-ts) | 0.28 | 457/457 | 202.7 | 1,475.3 | 40,408 | 613.2 | 22 | 49.6 |
| Ruby (interscript-ruby) | 137.59 | 457/457 | 974.2 | 1,976.3 | 54,130 | 594.3 | 863 | 33.4 |
| Python (interscript-py) | 3.59 | 435/457 | 273.0 | 52.1 | 1,426 | 241.3 | 42 | 19.4 |

Definitions (see PROTOCOL.md): **cold load** = fresh process loading all 457
system maps through the runtime's standard registry path; **docs/sec /
chars/sec** = warm throughput over the 200-doc set x 5 loops (map load cost
amortised); **peak doc (ms)** = fresh-process load + single transliteration of
the largest input doc (`un-ell-Grek-Latn-1987-ts`, 701 chars); RSS = peak
resident set of the whole runner process per phase (`/usr/bin/time -l`).

## Reading the numbers (as-measured caveats — no fixes in this lane)

- **Ruby: slowest cold, fastest warm.** 137.6 s / 974 MB to compile the
  corpus through parsanol, then the highest warm throughput of the four
  (1,976 docs/sec). The cold cost is per-map compile; the warm cost is
  execution over compiled maps.
- **TypeScript: fastest cold by construction.** It loads the pre-compiled
  JSON-IR artifact (275 ms), so its cold-load number measures deserialise +
  link, not ISC parsing — the parse cost lives in the artifact build (the
  canonical tsx builder). This registry-path asymmetry is inherent to the
  current pipelines and recorded, not normalized away.
- **Rust: mid cold load, mid warm throughput.** Parses `.isc` at load
  (9.6 s / 456 maps). One map cannot load:
  `var-ara-Arab-Arab-rababa` (funcall `rababa` needs external ML engines,
  unsupported in the current slice — by design, not a regression).
- **Python: 22 corpus maps fail to load.** Genuine ISC-parser gaps in the
  py runtime: `languages` blocks (the `iso-*-15919-2026` family),
  `parallel` stage blocks (`alalc-tha-Thai-Latn-2011`,
  `bgnpcgn-tha-Thai-Latn-2002`), and free-text `note` parsing
  (`din-urd-Arab-Latn-31635-2011`). 7 of the 200 sampled docs are affected:
  their warm throughput contributes `errors` (7 load failures x (1 warm + 5
  timed) = 42 failed calls, consistent with `warm_errors: 7` +
  `errors: 35`). Load coverage 435/457 is the honest cold-load number.
- **RSS baselines are process-level** (interpreter + registry + working
  set). Cross-runtime deltas include each interpreter's baseline; per-runtime
  trends over time are the primary signal.
- Peak-doc wall times include the fresh single-map load (protocol phase (c));
  the four runtimes' load-cost profiles above explain most of the spread
  (ruby 863 ms vs ts 22 ms).

## Reproducing

`PROTOCOL.md` §Reproducing has the exact commands. The runners:
[`rs-bench.rs`](../../examples/rs-bench.rs) (cargo example),
[`ts-bench.mjs`](./runners/ts-bench.mjs),
[`ruby-bench.rb`](./runners/ruby-bench.rb),
[`py-bench.py`](./runners/py-bench.py); aggregation:
[`aggregate.py`](./aggregate.py) over a results directory of
`<runtime>-<phase>.json` + `.rss` files. The generated `bench-inputs.json`
and the per-phase result files are local bench fixtures and are not
committed to any repository.
