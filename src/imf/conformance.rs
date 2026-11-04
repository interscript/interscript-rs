//! The frozen conformance fixtures (interscript-models/conformance/):
//! load the vectors files, resolve the referenced artifact, run every
//! row byte-exactly (`pipeline.conformance.run_conformance` parity).
//!
//! Vectors file schema (validated like `validate_vectors`):
//! `{model, index_version, decode:{mode}, source, vectors:[{src,out}]}`;
//! `decode.mode` must be `single-row-greedy`. Comparison is byte-exact
//! — no normalization, no CER tolerance.

use crate::error::{Error, Result};
use crate::imf::infer::{self, InferenceEngine};
use crate::imf::loader;
use serde::Deserialize;
use std::path::{Path, PathBuf};

fn model_err(what: impl Into<String>) -> Error {
    Error::Model(what.into())
}

#[derive(Deserialize)]
struct VectorsYaml {
    model: String,
    #[serde(default)]
    index_version: Option<u32>,
    #[serde(default)]
    decode: DecodeYaml,
    #[serde(default)]
    source: String,
    vectors: Vec<VectorRow>,
}

#[derive(Deserialize, Default)]
struct DecodeYaml {
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize)]
struct VectorRow {
    src: String,
    out: Option<String>,
}

/// A validated vectors file.
pub struct Vectors {
    pub model: String,
    pub index_version: Option<u32>,
    pub mode: String,
    pub source: String,
    pub rows: Vec<(String, String)>,
}

impl Vectors {
    /// `load_vectors` + `validate_vectors` parity: model id required,
    /// mode must be single-row-greedy, every row needs src and out.
    pub fn load(path: &Path) -> Result<Vectors> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| model_err(format!("vectors read failed ({}): {e}", path.display())))?;
        let parsed: VectorsYaml = serde_json::from_str(&raw)            .map_err(|e| model_err(format!("vectors parse failed ({}): {e}", path.display())))?;

        let mut problems: Vec<String> = Vec::new();
        if parsed.model.is_empty() {
            problems.push("missing 'model' (the models.yaml id)".into());
        }
        let mode = parsed.decode.mode.clone().unwrap_or_default();
        if mode != "single-row-greedy" {
            problems.push(format!(
                "decode.mode must be 'single-row-greedy', got {mode:?}"
            ));
        }
        if parsed.vectors.is_empty() {
            problems.push("'vectors' is empty — a fixture pins at least one row".into());
        }
        for (i, row) in parsed.vectors.iter().enumerate() {
            if row.src.is_empty() {
                problems.push(format!("vectors[{i}]: missing 'src'"));
            }
            if row.out.is_none() {
                problems.push(format!("vectors[{i}]: missing 'out' (expected output)"));
            }
        }
        if !problems.is_empty() {
            return Err(model_err(format!(
                "invalid conformance vectors: {}",
                problems.join("; ")
            )));
        }

        Ok(Vectors {
            model: parsed.model,
            index_version: parsed.index_version,
            mode,
            source: parsed.source,
            rows: parsed
                .vectors
                .into_iter()
                .map(|r| (r.src, r.out.unwrap_or_default()))
                .collect(),
        })
    }
}

/// Per-row outcome for reporting.
pub struct Mismatch {
    pub index: usize,
    pub src: String,
    pub expected: String,
    pub got: String,
}

pub struct ConformanceResult {
    pub model: String,
    pub checked: usize,
    pub mismatches: Vec<Mismatch>,
}

impl ConformanceResult {
    pub fn passed(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// Run every row through the engine; byte-exact comparison (the
/// fixture byte rule from the vector generator: `max(0, t-3)`, lossy
/// UTF-8).
pub fn run_conformance(
    vectors: &Vectors,
    engine: &mut dyn InferenceEngine,
) -> Result<ConformanceResult> {
    let mut mismatches = Vec::new();
    for (i, (src, expected)) in vectors.rows.iter().enumerate() {
        let ids = infer::greedy_generate(engine, src, infer::MAX_DECODE_STEPS)?;
        let got = infer::ids_to_fixture_string(&ids);
        if &got != expected {
            mismatches.push(Mismatch {
                index: i,
                src: src.clone(),
                expected: expected.clone(),
                got,
            });
        }
    }
    Ok(ConformanceResult {
        model: vectors.model.clone(),
        checked: vectors.rows.len(),
        mismatches,
    })
}

/// Artifact resolution for a fixture model id, given an optional
/// artifact directory (flat `<id>.zip` or cache layout
/// `<id>/*.zip`) and the models.yaml index (renames honored).
///
/// Directory lookup matches a zip by file stem OR by the id declared
/// in its `metadata.yaml` — the renamed-release layout (a zip whose
/// metadata still carries the pre-rename id, filed under the new id)
/// resolves under both ids.
pub fn resolve_artifact(
    model_id: &str,
    artifact_dir: Option<&Path>,
    index: Option<&crate::imf::registry::Index>,
) -> Result<PathBuf> {
    if let Some(dir) = artifact_dir {
        let canonical = index.map(|i| i.canonical_id(model_id).to_string());

        // 1. flat layout: <dir>/<id>.zip
        for candidate in [model_id, canonical.as_deref().unwrap_or("")].iter() {
            if candidate.is_empty() {
                continue;
            }
            let p = dir.join(format!("{candidate}.zip"));
            if p.is_file() {
                return Ok(p);
            }
        }
        // 2. cache layout: <dir>/<id>/<filename>
        for candidate in [model_id, canonical.as_deref().unwrap_or("")].iter() {
            if candidate.is_empty() {
                continue;
            }
            let sub = dir.join(candidate);
            if sub.is_dir() {
                if let Some(p) = first_zip(&sub) {
                    return Ok(p);
                }
            }
        }
        // 3. metadata-id scan: any zip in the tree whose metadata
        //    declares the id (covers pre-rename metadata ids).
        let wanted: Vec<&str> = std::iter::once(model_id)
            .chain(canonical.as_deref())
            .collect();
        let mut zips: Vec<PathBuf> = Vec::new();
        collect_zips(dir, &mut zips, 0);
        zips.sort();
        for zip_path in &zips {
            if let Ok(manifest) = loader::load_manifest(zip_path) {
                if wanted.iter().any(|w| *w == manifest.id.as_str()) {
                    return Ok(zip_path.clone());
                }
            }
        }
        return Err(model_err(format!(
            "no artifact for {model_id:?} under {} (looked for <id>.zip, <id>/*.zip, metadata-id match)",
            dir.display()
        )));
    }

    // Registry path: index load + verified cache/download (SPEC §2).
    let index = match index {
        Some(i) => i.clone(),
        None => crate::imf::registry::load_index(None)?,
    };
    crate::imf::registry::resolve(model_id, &index)
}

fn first_zip(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("zip"))
        .collect();
    entries.sort();
    entries.into_iter().next()
}

fn collect_zips(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_zips(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("zip") {
            out.push(path);
        }
    }
}
