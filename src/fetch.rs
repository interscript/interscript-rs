//! Fetch + unpack the raw maps corpus from the interscript/maps GitHub
//! Releases data channel.
//!
//! The `interscript-maps-isc-<version>.tar.gz` asset (maps/ + libs/ at
//! the tagged commit) and its `.sha256` sidecar download into a local
//! directory usable directly as an `iscrs` load path:
//!
//! ```text
//! iscrs maps-fetch 3.0.0 --dest ./maps-corpus
//! iscrs test-all --root ./maps-corpus/maps
//! ```

use crate::error::{Error, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const RELEASE_BASE: &str = "https://github.com/interscript/maps/releases/download";

fn asset_url(version: &str, name: &str) -> String {
    format!("{RELEASE_BASE}/v{version}/{name}")
}

/// sha256 of a file, streamed.
fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65_536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download `url` into `dest`, streaming.
fn download(url: &str, dest: &Path) -> Result<()> {
    let response = ureq::get(url).call().map_err(|e| {
        Error::MapLogic(format!("download {url}: {e}"))
    })?;
    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(())
}

/// Fetch the corpus archive for `version` into `dest`, verify its sha256
/// against the release sidecar, and unpack maps/ + libs/ under it.
///
/// Returns the unpacked load-path directory (`dest/maps`).
pub fn fetch_corpus(version: &str, dest: &Path) -> Result<PathBuf> {
    let name = format!("interscript-maps-isc-{version}.tar.gz");
    fs::create_dir_all(dest)?;

    let archive_path = dest.join(&name);
    if !archive_path.exists() {
        download(&asset_url(version, &name), &archive_path)?;
    }

    // sidecar verify
    let sidecar_path = dest.join(format!("{name}.sha256"));
    if !sidecar_path.exists() {
        download(&asset_url(version, &format!("{name}.sha256")), &sidecar_path)?;
    }
    let expected = fs::read_to_string(&sidecar_path)?
        .split_whitespace()
        .next()
        .ok_or_else(|| Error::MapLogic(format!("{sidecar_path:?}: empty sidecar")))?
        .to_string();
    let actual = sha256_file(&archive_path)?;
    if actual != expected {
        return Err(Error::MapLogic(format!(
            "{name}: sha256 mismatch (expected {expected}, got {actual})"
        )));
    }

    // unpack
    let gz = fs::File::open(&archive_path)?;
    let gz = flate2::read::GzDecoder::new(gz);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest).map_err(|e| {
        Error::MapLogic(format!("unpack {name}: {e}"))
    })?;

    let maps_dir = dest.join("maps");
    if !maps_dir.is_dir() {
        return Err(Error::MapLogic(format!(
            "{name}: unpacked but no maps/ directory found"
        )));
    }
    Ok(maps_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_mismatch_is_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "iscrs-fetch-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let name = "interscript-maps-isc-9.9.9.tar.gz";
        let archive = dir.join(name);
        fs::write(&archive, b"not a real archive").unwrap();
        fs::write(dir.join(format!("{name}.sha256")), format!("{}  {name}\n", "0".repeat(64)))
            .unwrap();

        match fetch_corpus("9.9.9", &dir) {
            Err(Error::MapLogic(msg)) => {
                assert!(msg.contains("sha256 mismatch"), "{msg}");
            }
            other => panic!("expected sha256 mismatch error, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }
}
