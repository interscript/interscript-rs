#!/usr/bin/env python3
"""Extract the benchmark input set from the golden corpus (one script, one place).

Usage:
  python3 extract_bench_inputs.py <golden.jsonl> <out.json> [N] [SEED]

Deterministic sample of N documents (default 200) from the ISC-WF-0 golden
corpus (interscript-rs tests/golden/isc-wf0.jsonl), diversified across maps:

  1. group unique (map, src) rows by map id;
  2. order map ids by sha256("<map-id>:<seed>") — deterministic pseudo-random
     order keyed on the map id itself (seed 42 upstream, recorded in output);
  3. round-robin passes over that order, taking each map's next unused row in
     file order, until N docs are collected.

With >= N distinct maps this yields exactly one document per map for the first
N map ids in the hashed order (maximises map coverage).

Output JSON schema:
  { "seed", "count", "source", "notes",
    "inputs": [ { "map", "src", "out", "chars" }, ... ] }

The generated file is a local bench fixture — it is NOT committed to any
repository (data rides releases, not code).
"""
import hashlib
import json
import sys
from collections import defaultdict


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    golden_path, out_path = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 200
    seed = sys.argv[4] if len(sys.argv) > 4 else "42"

    by_map: dict[str, list[dict]] = defaultdict(list)
    seen: set[tuple[str, str]] = set()
    total_rows = 0
    with open(golden_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            row = json.loads(line)
            total_rows += 1
            key = (row["map"], row["src"])
            if key in seen:
                continue
            seen.add(key)
            by_map[row["map"]].append(row)

    # Deterministic map-id-hash order (seeded).
    map_ids = sorted(by_map, key=lambda m: hashlib.sha256(f"{m}:{seed}".encode()).hexdigest())

    inputs: list[dict] = []
    cursors = defaultdict(int)
    while len(inputs) < n:
        progressed = False
        for map_id in map_ids:
            if len(inputs) >= n:
                break
            cur = cursors[map_id]
            rows = by_map[map_id]
            if cur < len(rows):
                row = rows[cur]
                cursors[map_id] = cur + 1
                inputs.append(
                    {
                        "map": row["map"],
                        "src": row["src"],
                        "out": row["out"],
                        "chars": len(row["src"]),
                    }
                )
                progressed = True
        if not progressed:
            break  # corpus exhausted below N

    doc = {
        "seed": seed,
        "count": len(inputs),
        "source": "tests/golden/isc-wf0.jsonl (interscript-rs)",
        "notes": "deterministic map-id-hash order (sha256(map:seed)), round-robin 1 doc/map/pass",
        "total_rows": total_rows,
        "distinct_maps_in_corpus": len(map_ids),
        "distinct_maps_sampled": len({i["map"] for i in inputs}),
        "inputs": inputs,
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False)
        f.write("\n")
    print(
        f"extract_bench_inputs: {total_rows} rows -> {len(inputs)} docs, "
        f"{doc['distinct_maps_sampled']} distinct maps, seed={seed}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
