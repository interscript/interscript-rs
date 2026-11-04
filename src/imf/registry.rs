//! models.yaml index resolution + cached downloads (SPEC.md §1/§2,
//! `interscript_ml/registry.py` parity).
//!
//! Conformance points implemented here:
//! * C1 — model ids resolve against a `models.yaml` index of
//!   `version: 1`, honoring `SECRYST_INDEX`.
//! * C2 — the whole-artifact sha256 is verified before installation
//!   AND on every cache hit; a mismatch fails loudly.
//! * §2 — resolution order: verified cache hit -> download (single URL
//!   or parts in index order, each part verified as it lands) ->
//!   whole-artifact verify -> atomic rename into place.
//! * `renames:` — WO87 target-accurate rename aliases (old id -> new
//!   id); old ids stay resolvable.

use crate::error::{Error, Result};
use crate::imf::loader::sha256_file;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const DEFAULT_INDEX_URL: &str = "https://github.com/interscript/interscript-models/releases/download/index-v12/models-index.yaml";
const ENV_INDEX: &str = "SECRYST_INDEX";
const ENV_CACHE: &str = "SECRYST_CACHE";

fn model_err(what: impl Into<String>) -> Error {
    Error::Model(what.into())
}

#[derive(Debug, Clone)]
pub struct Part {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub id: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub precision: String,
    pub task: String,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Default)]
pub struct Index {
    pub models: BTreeMap<String, IndexEntry>,
    /// WO87 renames: old id -> current id (old ids stay resolvable).
    pub renames: BTreeMap<String, String>,
}

impl Index {
    /// Resolve a possibly-renamed id to its current entry id.
    pub fn canonical_id(&self, id: &str) -> String {
        let mut cur = id.to_string();
        // Follow the rename chain (bounded to reject cycles).
        for _ in 0..8 {
            match self.renames.get(&cur) {
                Some(next) => cur = next.clone(),
                None => break,
            }
        }
        cur
    }
}

/// `SECRYST_CACHE` or `~/.cache/secryst` (the shared cache layout).
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_CACHE) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("secryst")
}

/// The index source: `SECRYST_INDEX`, then the default index-v12 URL.
fn index_source(explicit: Option<&str>) -> String {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var(ENV_INDEX).ok())
        .unwrap_or_else(|| DEFAULT_INDEX_URL.to_string())
}

#[derive(serde::Deserialize)]
struct IndexYaml {
    version: i32,
    #[serde(default)]
    models: BTreeMap<String, IndexEntryYaml>,
    #[serde(default)]
    renames: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
struct IndexEntryYaml {
    filename: String,
    #[serde(default)]
    url: String,
    sha256: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    precision: String,
    #[serde(default)]
    task: String,
    #[serde(default)]
    parts: Vec<PartYaml>,
}

#[derive(serde::Deserialize)]
struct PartYaml {
    url: String,
    sha256: String,
    #[serde(default)]
    size: u64,
}

/// Load a `models.yaml` index from an http(s) URL or a local path.
pub fn load_index(explicit: Option<&str>) -> Result<Index> {
    let source = index_source(explicit);
    let text = if source.starts_with("http://") || source.starts_with("https://") {
        let mut res = ureq::get(&source)
            .call()
            .map_err(|e| model_err(format!("index fetch failed ({source}): {e}")))?;
        let buf = res
            .body_mut()
            .read_to_string()
            .map_err(|e| model_err(format!("index read failed ({source}): {e}")))?;
        buf
    } else {
        std::fs::read_to_string(&source)
            .map_err(|e| model_err(format!("index read failed ({source}): {e}")))?
    };

    let raw: IndexYaml = serde_yaml::from_str(&text)
        .map_err(|e| model_err(format!("index parse failed ({source}): {e}")))?;
    if raw.version != 1 {
        return Err(model_err(format!(
            "index must be a mapping with version: 1, got {}",
            raw.version
        )));
    }

    let mut models = BTreeMap::new();
    for (model_id, spec) in raw.models {
        models.insert(
            model_id.clone(),
            IndexEntry {
                id: model_id,
                filename: spec.filename,
                url: spec.url,
                sha256: spec.sha256.to_lowercase(),
                size: spec.size,
                precision: if spec.precision.is_empty() {
                    "fp32".into()
                } else {
                    spec.precision
                },
                task: spec.task,
                parts: spec
                    .parts
                    .into_iter()
                    .map(|p| Part {
                        url: p.url,
                        sha256: p.sha256.to_lowercase(),
                        size: p.size,
                    })
                    .collect(),
            },
        );
    }
    Ok(Index {
        models,
        renames: raw.renames,
    })
}

/// §2: return a verified local zip path for `model_id`, downloading
/// into the cache when needed. Never returns an unverified file.
pub fn resolve(model_id: &str, index: &Index) -> Result<PathBuf> {
    let canonical = index.canonical_id(model_id);
    let entry = index.models.get(&canonical).ok_or_else(|| {
        model_err(format!(
            "unknown model id {model_id:?} (known: {} ids; use --artifact-dir or SECRYST_CACHE)",
            index.models.len()
        ))
    })?;

    let target = cache_dir()
        .join("models")
        .join(&canonical)
        .join(&entry.filename);
    // Cache hits are re-verified, never trusted blindly (C2).
    if target.is_file() && sha256_file(&target)? == entry.sha256 {
        return Ok(target);
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            model_err(format!("cache dir {} create failed: {e}", parent.display()))
        })?;
    }
    let tmp = target.with_extension(format!(
        "part-{}",
        std::process::id()
    ));

    let result = (|| -> Result<()> {
        if !entry.parts.is_empty() {
            download_parts(&entry.parts, &tmp)?;
        } else if entry.url.is_empty() {
            return Err(model_err(format!(
                "index entry {canonical:?} has no url and no cached copy"
            )));
        } else if let Some(path) = entry.url.strip_prefix("file://") {
            // file:// is a mirror, not a move.
            std::fs::copy(path, &tmp)
                .map_err(|e| model_err(format!("channel file copy failed ({path}): {e}")))?;
        } else {
            download_to(&entry.url, &tmp)?;
        }
        let actual = sha256_file(&tmp)?;
        if actual != entry.sha256 {
            return Err(model_err(format!(
                "downloaded {} sha256 mismatch: got {actual}, index says {}",
                entry.filename, entry.sha256
            )));
        }
        // C6 (SHOULD): install atomically.
        std::fs::rename(&tmp, &target)
            .map_err(|e| model_err(format!("cache install failed: {e}")))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    Ok(target)
}

fn open_channel(url: &str) -> Result<Box<dyn std::io::Read>> {
    if let Some(path) = url.strip_prefix("file://") {
        let f = std::fs::File::open(path)
            .map_err(|e| model_err(format!("channel file missing ({path}): {e}")))?;
        return Ok(Box::new(f));
    }
    let res = ureq::get(url)
        .call()
        .map_err(|e| model_err(format!("download failed ({url}): {e}")))?;
    Ok(Box::new(
        res.into_body()
            .into_reader(),
    ))
}

fn download_to(url: &str, dest: &Path) -> Result<()> {
    let mut out = std::fs::File::create(dest)
        .map_err(|e| model_err(format!("temp file create failed: {e}")))?;
    let mut stream = open_channel(url)?;
    std::io::copy(&mut stream, &mut out)
        .map_err(|e| model_err(format!("download stream failed ({url}): {e}")))?;
    Ok(())
}

/// §1: parts stream into one file in index order, each verified as it
/// lands; the caller then checks the assembled file as usual.
fn download_parts(parts: &[Part], dest: &Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dest)
        .map_err(|e| model_err(format!("temp file open failed: {e}")))?;
    for (i, part) in parts.iter().enumerate() {
        let mut hasher = Sha256::new();
        let mut stream = open_channel(&part.url)?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = stream
                .read(&mut buf)
                .map_err(|e| model_err(format!("part stream failed: {e}")))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| model_err(format!("part write failed: {e}")))?;
            hasher.update(&buf[..n]);
        }
        let actual = crate::imf::loader::to_hex(&hasher.finalize());
        if actual != part.sha256 {
            return Err(model_err(format!(
                "part {i} sha256 mismatch: got {actual}, index says {}",
                part.sha256
            )));
        }
    }
    Ok(())
}
