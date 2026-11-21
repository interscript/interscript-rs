#!/usr/bin/env node
// ts-bench — benchmark runner for the interscript-ts runtime.
// Uniform interface shared by the four runtime runners
// (see interscript-rs docs/benchmarks/PROTOCOL.md). One JSON object on stdout.
//
//   node ts-bench.mjs --phase load|throughput|peak --inputs bench-inputs.json \
//     --ts-repo ../interscript-ts --ir-dir /path/to/json-ir/maps \
//     [--ids-file system-map-ids.txt] [--loops 5] [--commit <sha>] [--maps-commit <sha>]
//
// Registry path: filesystemStrategy over the compiled JSON-IR artifact
// (built from the pinned maps commit by the maps repo's canonical builder).
// --ids-file restricts the load phase to the corpus SYSTEM maps so the
// cold-load count matches the other runtimes (the flat artifact also
// contains dependency libraries).
import { readFileSync, readdirSync } from "node:fs"
import { pathToFileURL } from "node:url"
import { resolve } from "node:path"

function arg(name, def) {
  const i = process.argv.indexOf(`--${name}`)
  if (i >= 0 && i + 1 < process.argv.length) return process.argv[i + 1]
  if (def !== undefined) return def
  console.error(`missing --${name}`)
  process.exit(2)
}

const phase = arg("phase")
const inputsPath = arg("inputs")
const tsRepo = resolve(arg("ts-repo"))
const irDir = resolve(arg("ir-dir"))
const loops = parseInt(arg("loops", "5"), 10)
const commit = arg("commit", "unknown")
const mapsCommit = arg("maps-commit", "unknown")

const { filesystemStrategy } = await import(
  pathToFileURL(resolve(tsRepo, "dist/loaders.node.js"))
)
const isx = await import(pathToFileURL(resolve(tsRepo, "dist/index.js")))

function inputDocs() {
  const doc = JSON.parse(readFileSync(inputsPath, "utf8"))
  return doc.inputs.map((d) => ({ map: d.map, src: d.src, chars: d.chars }))
}

const base = {
  runtime: "typescript",
  version: JSON.parse(readFileSync(resolve(tsRepo, "package.json"), "utf8")).version,
  commit,
  phase,
  maps_commit: mapsCommit,
}

function emit(extra) {
  console.log(JSON.stringify({ ...base, ...extra }))
}

if (phase === "load") {
  // fresh runtime per process: configure() replaces the default runtime
  isx.configure({ strategies: [filesystemStrategy(irDir)] })
  let ids = readdirSync(resolve(irDir))
    .filter((f) => f.endsWith(".json"))
    .sort()
  if (process.argv.includes("--ids-file")) {
    ids = readFileSync(arg("ids-file"), "utf8")
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean)
  }
  const t0 = performance.now()
  const loaded = []
  const failed = []
  for (const id of ids) {
    const code = id.replace(/\.json$/, "")
    try {
      isx.loadMap(code)
      loaded.push(code)
    } catch (e) {
      failed.push(`${code}: ${e.message}`)
    }
  }
  emit({
    attempted: ids.length,
    loaded: loaded.length,
    failed,
    cold_load_ms: Math.round(performance.now() - t0),
  })
} else if (phase === "throughput") {
  isx.configure({ strategies: [filesystemStrategy(irDir)] })
  const docs = inputDocs()
  const loadFailed = []
  for (const d of docs) {
    try {
      isx.loadMap(d.map)
    } catch (e) {
      loadFailed.push(`${d.map}: ${e.message}`)
    }
  }
  // warm pass (untimed)
  let warmErrors = 0
  for (const d of docs) {
    try {
      isx.transliterate(d.map, d.src)
    } catch {
      warmErrors++
    }
  }
  const totalChars = docs.reduce((a, d) => a + d.chars, 0)
  const t0 = performance.now()
  let errors = 0
  for (let i = 0; i < loops; i++) {
    for (const d of docs) {
      try {
        isx.transliterate(d.map, d.src)
      } catch {
        errors++
      }
    }
  }
  const wallS = (performance.now() - t0) / 1000
  const calls = docs.length * loops
  emit({
    docs: docs.length,
    loops,
    total_chars: totalChars,
    load_failed: loadFailed,
    warm_errors: warmErrors,
    wall_s: wallS,
    docs_per_sec: calls / wallS,
    chars_per_sec: (totalChars * loops) / wallS,
    errors,
  })
} else if (phase === "peak") {
  isx.configure({ strategies: [filesystemStrategy(irDir)] })
  const docs = inputDocs()
  const big = docs.reduce((a, d) => (d.chars > a.chars ? d : a))
  const t0 = performance.now()
  let ok = true
  try {
    isx.loadMap(big.map)
    isx.transliterate(big.map, big.src)
  } catch {
    ok = false
  }
  emit({
    map: big.map,
    chars: big.chars,
    ok,
    wall_ms: Math.round(performance.now() - t0),
  })
} else {
  console.error(`unknown phase ${phase}`)
  process.exit(2)
}
