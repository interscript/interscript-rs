#!/usr/bin/env python3
"""Aggregate benchmark results into the baselines markdown table.

Usage:
  python3 aggregate.py <results-dir>

Reads the per-phase JSON runner outputs (<runtime>-<phase>.json) and the
/usr/bin/time -l capture files (<runtime>-<phase>.rss) from one results
directory and prints the BASELINES table body to stdout.

RSS convention: /usr/bin/time -l on macOS reports "maximum resident set
size" in BYTES; reported as MB (2^20), one decimal.
"""
from __future__ import annotations

import json
import os
import sys

RUNTIMES = [
    ("rs", "Rust (interscript-rs)"),
    ("ts", "TypeScript (interscript-ts)"),
    ("ruby", "Ruby (interscript-ruby)"),
    ("py", "Python (interscript-py)"),
]


def rss_mb(path: str) -> float | None:
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as f:
        for line in f:
            if "maximum resident set size" in line:
                return int(line.split()[0]) / (1024 * 1024)
    return None


def fmt(v: float | None, unit: str = "", digits: int = 1) -> str:
    if v is None:
        return "n/a"
    if digits == 0:
        return f"{v:,.0f}{unit}"
    return f"{v:,.{digits}f}{unit}"


def load_json(path: str) -> dict | None:
    """Runner JSON, or None when the phase is missing/still running
    (the file exists but is empty until the runner's final print)."""
    if not os.path.exists(path):
        return None
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except json.JSONDecodeError:
        return None


def main() -> int:
    results = sys.argv[1]
    rows = []
    for short, label in RUNTIMES:
        row: dict = {"label": label}
        if (d := load_json(os.path.join(results, f"{short}-load.json"))):
            row["load_s"] = d.get("cold_load_ms", 0) / 1000
            row["loaded"] = f"{d.get('loaded')}/{d.get('attempted')}"
            row["rss_load"] = rss_mb(os.path.join(results, f"{short}-load.rss"))
        if (d := load_json(os.path.join(results, f"{short}-throughput.json"))):
            row["docs_sec"] = d.get("docs_per_sec")
            row["chars_sec"] = d.get("chars_per_sec")
            row["errors"] = d.get("errors", 0)
            row["rss_thr"] = rss_mb(os.path.join(results, f"{short}-throughput.rss"))
        if (d := load_json(os.path.join(results, f"{short}-peak.json"))):
            row["peak_ms"] = d.get("wall_ms")
            row["rss_peak"] = rss_mb(os.path.join(results, f"{short}-peak.rss"))
        rows.append(row)

    print(
        "| Runtime | Cold load (s) | Loaded | RSS @ load (MB) "
        "| Docs/sec | Chars/sec | RSS @ throughput (MB) "
        "| Peak doc (ms) | RSS @ peak (MB) |"
    )
    print("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for r in rows:
        print(
            f"| {r['label']} "
            f"| {fmt(r.get('load_s'), digits=2)} "
            f"| {r.get('loaded', 'n/a')} "
            f"| {fmt(r.get('rss_load'))} "
            f"| {fmt(r.get('docs_sec'), digits=1)} "
            f"| {fmt(r.get('chars_sec'), digits=0)} "
            f"| {fmt(r.get('rss_thr'))} "
            f"| {fmt(r.get('peak_ms'), digits=0)} "
            f"| {fmt(r.get('rss_peak'))} |"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
