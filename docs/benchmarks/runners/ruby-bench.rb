#!/usr/bin/env ruby
# frozen_string_literal: true

# ruby-bench — benchmark runner for the interscript-ruby runtime.
# Uniform interface shared by the four runtime runners
# (see interscript-rs docs/benchmarks/PROTOCOL.md). One JSON object on stdout.
#
#   ruby -I< interscript-ruby >/lib ruby-bench.rb --phase load|throughput|peak \
#     --inputs bench-inputs.json --load-dir <flat .isc dir> \
#     [--ids-file system-map-ids.txt] [--loops 5] [--commit <sha>] [--maps-commit <sha>]
#
# --load-dir is a flat directory (systems + libs symlinked side by side)
# against Interscript.load_path's default ["."] entry: the standard
# locate -> DSL parse -> Interpreter compile registry path, no gems.
# --ids-file restricts the load phase to the corpus SYSTEM maps so the
# cold-load count matches the other runtimes (the fixture also holds libs).

require "json"
require "optparse"

opts = { loops: 5, commit: "unknown", maps_commit: "unknown" }
OptionParser.new do |o|
  o.on("--phase PHASE") { |v| opts[:phase] = v }
  o.on("--inputs PATH") { |v| opts[:inputs] = v }
  o.on("--load-dir DIR") { |v| opts[:load_dir] = v }
  o.on("--ids-file PATH") { |v| opts[:ids_file] = v }
  o.on("--loops N") { |v| opts[:loops] = v.to_i }
  o.on("--commit SHA") { |v| opts[:commit] = v }
  o.on("--maps-commit SHA") { |v| opts[:maps_commit] = v }
end.parse!

phase = opts[:phase] or abort "missing --phase"
inputs_path = opts[:inputs] or abort "missing --inputs"
load_dir = opts[:load_dir] or abort "missing --load-dir"

require "interscript"
# The registry memoizes load_path on first locate; chdir into the flat
# load dir BEFORE any Interscript call resolves it (load_path default ["."]).
Dir.chdir(load_dir)

def input_docs(path)
  JSON.parse(File.read(path))["inputs"].map do |d|
    { map: d["map"], src: d["src"], chars: d["chars"] }
  end
end

def system_map_ids(opts)
  return File.readlines(opts[:ids_file]).map(&:strip).reject(&:empty?) if opts[:ids_file]

  Dir["*.isc"].map { |f| File.basename(f, ".isc") }.sort
end

def emit(base, extra)
  puts JSON.generate(base.merge(extra))
  $stdout.flush
end

base = {
  "runtime" => "ruby",
  "version" => Interscript::VERSION,
  "commit" => opts[:commit],
  "phase" => phase,
  "maps_commit" => opts[:maps_commit],
}

monotonic = defined?(Process::CLOCK_MONOTONIC) ? Process::CLOCK_MONOTONIC : nil

def now_ms
  Process.clock_gettime(Process::CLOCK_MONOTONIC, :millisecond)
end

case phase
when "load"
  ids = system_map_ids(opts)
  loaded = 0
  failed = []
  t0 = now_ms
  ids.each do |id|
    begin
      Interscript.load(id)
      loaded += 1
    rescue StandardError => e
      failed << "#{id}: #{e.class}: #{e.message.to_s[0, 120]}"
    end
  end
  emit(base, "attempted" => ids.size, "loaded" => loaded,
             "failed" => failed, "cold_load_ms" => now_ms - t0)
when "throughput"
  docs = input_docs(inputs_path)
  load_failed = []
  docs.each do |d|
    begin
      Interscript.load(d[:map])
    rescue StandardError => e
      load_failed << "#{d[:map]}: #{e.class}"
    end
  end
  # warm pass (untimed)
  warm_errors = 0
  docs.each do |d|
    begin
      Interscript.transliterate(d[:map], d[:src])
    rescue StandardError
      warm_errors += 1
    end
  end
  total_chars = docs.sum { |d| d[:chars] }
  loops = opts[:loops]
  t0 = now_ms
  errors = 0
  loops.times do
    docs.each do |d|
      begin
        Interscript.transliterate(d[:map], d[:src])
      rescue StandardError
        errors += 1
      end
    end
  end
  wall_s = (now_ms - t0) / 1000.0
  calls = docs.size * loops
  emit(base, "docs" => docs.size, "loops" => loops,
             "total_chars" => total_chars, "load_failed" => load_failed,
             "warm_errors" => warm_errors, "wall_s" => wall_s,
             "docs_per_sec" => calls / wall_s,
             "chars_per_sec" => total_chars * loops / wall_s,
             "errors" => errors)
when "peak"
  docs = input_docs(inputs_path)
  big = docs.max_by { |d| d[:chars] }
  t0 = now_ms
  ok = begin
    Interscript.load(big[:map])
    Interscript.transliterate(big[:map], big[:src])
    true
  rescue StandardError
    false
  end
  emit(base, "map" => big[:map], "chars" => big[:chars],
             "ok" => ok, "wall_ms" => now_ms - t0)
else
  abort "unknown phase #{phase}"
end
