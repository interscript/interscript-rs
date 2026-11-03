//! Map resolution over the load path — `Interscript.locate` parity:
//! for every directory, try `.isc` (ISC maps), then `.iml`
//! (character-set libraries), then `.imp` (legacy Ruby DSL; not
//! supported here).

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

pub fn locate(load_path: &[PathBuf], map_name: &str) -> Result<PathBuf> {
    for dir in load_path {
        for ext in ["isc", "iml", "imp"] {
            let candidate = dir.join(format!("{map_name}.{ext}"));
            if candidate.is_file() {
                if ext == "imp" {
                    return Err(Error::Unsupported(format!(
                        "legacy .imp map {map_name:?} (Ruby DSL); only .isc/.iml run in slice 1"
                    )));
                }
                return Ok(candidate);
            }
        }
    }
    Err(Error::MapNotFound(map_name.to_string()))
}

pub fn read_map(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))
}

/// Default load path: $ISCRS_MAPS (colon-separated), falling back to
/// the sibling Interscript maps checkout (`../maps/maps` +
/// `../maps/libs`, relative to the working directory).
pub fn default_load_path() -> Vec<PathBuf> {
    if let Ok(env) = std::env::var("ISCRS_MAPS") {
        return std::env::split_paths(&env).collect();
    }
    let mut out = Vec::new();
    for rel in ["maps/maps", "maps/libs"] {
        for base in [".", "..", "../.."] {
            let candidate = Path::new(base).join(rel);
            if candidate.is_dir() {
                out.push(candidate);
                break;
            }
        }
    }
    out
}
