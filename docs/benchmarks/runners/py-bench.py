#!/usr/bin/env python3
"""py-bench — benchmark runner for the interscript-py runtime.

Uniform interface shared by the four runtime runners (see interscript-rs
docs/benchmarks/PROTOCOL.md). One JSON object on stdout.

    python3 py-bench.py --phase load|throughput|peak \
        --inputs bench-inputs.json --py-repo ../interscript-py \
        --maps-repo ../maps [--loops 5] [--commit SHA] [--maps-commit SHA]

Registry path: interscript.add_load_path([maps/, libs/]) + load_map
against the py repo's src/ tree (no install, no venv).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time


def now_ms() -> float:
    return time.monotonic_ns() / 1_000_000


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--phase", required=True, choices=["load", "throughput", "peak"])
    ap.add_argument("--inputs", required=True)
    ap.add_argument("--py-repo", required=True)
    ap.add_argument("--maps-repo", required=True)
    ap.add_argument("--loops", type=int, default=5)
    ap.add_argument("--commit", default="unknown")
    ap.add_argument("--maps-commit", default="unknown")
    args = ap.parse_args()

    src = os.path.join(args.py_repo, "src")
    sys.path.insert(0, src)

    def read_version() -> str:
        toml = os.path.join(args.py_repo, "pyproject.toml")
        with open(toml, encoding="utf-8") as f:
            for line in f:
                if line.startswith("version"):
                    return line.split("=", 1)[1].strip().strip('"')
        return "unknown"

    import interscript  # noqa: E402  (path set above)

    maps_dir = os.path.join(args.maps_repo, "maps")
    libs_dir = os.path.join(args.maps_repo, "libs")
    interscript.add_load_path(maps_dir)
    interscript.add_load_path(libs_dir)

    base = {
        "runtime": "python",
        "version": read_version(),
        "commit": args.commit,
        "phase": args.phase,
        "maps_commit": args.maps_commit,
    }

    def input_docs() -> list[dict]:
        with open(args.inputs, encoding="utf-8") as f:
            return [
                {"map": d["map"], "src": d["src"], "chars": d["chars"]}
                for d in json.load(f)["inputs"]
            ]

    def system_map_ids() -> list[str]:
        return sorted(
            f[: -len(".isc")] for f in os.listdir(maps_dir) if f.endswith(".isc")
        )

    if args.phase == "load":
        ids = system_map_ids()
        loaded, failed = 0, []
        t0 = now_ms()
        for mid in ids:
            try:
                interscript.load_map(mid)
                loaded += 1
            except Exception as e:  # noqa: BLE001 — count, don't crash the sweep
                failed.append(f"{mid}: {type(e).__name__}: {str(e)[:120]}")
        out = {
            "attempted": len(ids),
            "loaded": loaded,
            "failed": failed,
            "cold_load_ms": round(now_ms() - t0),
        }
    elif args.phase == "throughput":
        docs = input_docs()
        load_failed = []
        for d in docs:
            try:
                interscript.load_map(d["map"])
            except Exception as e:  # noqa: BLE001
                load_failed.append(f"{d['map']}: {type(e).__name__}")
        warm_errors = 0
        for d in docs:
            try:
                interscript.transliterate(d["map"], d["src"])
            except Exception:  # noqa: BLE001
                warm_errors += 1
        total_chars = sum(d["chars"] for d in docs)
        t0 = now_ms()
        errors = 0
        for _ in range(args.loops):
            for d in docs:
                try:
                    interscript.transliterate(d["map"], d["src"])
                except Exception:  # noqa: BLE001
                    errors += 1
        wall_s = (now_ms() - t0) / 1000
        calls = len(docs) * args.loops
        out = {
            "docs": len(docs),
            "loops": args.loops,
            "total_chars": total_chars,
            "load_failed": load_failed,
            "warm_errors": warm_errors,
            "wall_s": wall_s,
            "docs_per_sec": calls / wall_s,
            "chars_per_sec": total_chars * args.loops / wall_s,
            "errors": errors,
        }
    else:  # peak
        docs = input_docs()
        big = max(docs, key=lambda d: d["chars"])
        t0 = now_ms()
        ok = True
        try:
            interscript.load_map(big["map"])
            interscript.transliterate(big["map"], big["src"])
        except Exception:  # noqa: BLE001
            ok = False
        out = {
            "map": big["map"],
            "chars": big["chars"],
            "ok": ok,
            "wall_ms": round(now_ms() - t0),
        }

    print(json.dumps({**base, **out}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
