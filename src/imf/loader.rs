//! IMF v1 zip loading: manifest parse + sha256 member verification
//! (`interscript-models/runtime/src/interscript_ml/loader.py` parity,
//! SPEC.md §3).
//!
//! Conformance points implemented here:
//! * C3 — every `.onnx` member is verified against the manifest sha256
//!   map on load; members not covered by the manifest MUST NOT load.
//! * §3 — rejects any other `format`, any `tokenizer` other than
//!   `bytes`, and missing required members (`metadata.yaml`,
//!   `encoder.onnx`, `decoder.onnx`).
//! * Zip-level integrity is never trusted (the manifest decides).

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// `decoder: plain | kv` — kv iff `decoder-kv.onnx` is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderMode {
    Plain,
    Kv,
}

/// `metadata.yaml` as parsed from the artifact (Manifest dataclass
/// parity).
#[derive(Debug, Clone)]
pub struct Manifest {
    pub id: String,
    pub task: String,
    pub decoder: DecoderMode,
    pub precision: String,
    pub opset: u32,
    /// Member name -> recorded hex digest. Every `.onnx` member MUST be
    /// covered.
    pub sha256: BTreeMap<String, String>,
}

/// A loaded, checksum-verified artifact: the manifest plus the
/// verified ONNX graphs keyed by member name.
#[derive(Debug)]
pub struct ArtifactGraphs {
    pub manifest: Manifest,
    /// Verified `.onnx` member bytes (`encoder.onnx`, `decoder.onnx`,
    /// and `decoder-kv.onnx` when present).
    pub graphs: BTreeMap<String, Vec<u8>>,
}

impl ArtifactGraphs {
    /// The member the decoder session must load: `decoder-kv.onnx` for
    /// kv artifacts that carry it, else `decoder.onnx` (model.py
    /// decoder_name parity).
    pub fn decoder_member(&self) -> Result<&str> {
        match self.manifest.decoder {
            DecoderMode::Kv if self.graphs.contains_key("decoder-kv.onnx") => Ok("decoder-kv.onnx"),
            DecoderMode::Kv => Ok("decoder.onnx"),
            DecoderMode::Plain => Ok("decoder.onnx"),
        }
    }
}

fn model_err(what: impl Into<String>) -> Error {
    Error::Model(what.into())
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    to_hex(&hasher.finalize())
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Hex-normalize for comparison: the manifest digests are lowercase hex
/// in released metadata; accept either case but nothing else.
fn normalize_digest(s: &str) -> Result<String> {
    let lower = s.trim().to_lowercase();
    if lower.len() != 64 || !lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(model_err(format!("malformed sha256 digest {s:?}")));
    }
    Ok(lower)
}

/// Load and verify every `.onnx` member of an IMF v1 zip.
pub fn verify_and_read(zip_path: &Path) -> Result<ArtifactGraphs> {
    let manifest = load_manifest(zip_path)?;
    let file = std::fs::File::open(zip_path).map_err(|e| model_err(format!(
        "cannot open artifact {}: {e}",
        zip_path.display()
    )))?;
    let mut zf = zip::ZipArchive::new(file)
        .map_err(|e| model_err(format!("{} is not a readable zip: {e}", zip_path.display())))?;

    let names: Vec<String> = (0..zf.len())
        .filter_map(|i| {
            zf.by_index(i)
                .ok()
                .and_then(|f| f.name().ok().map(|n| n.into_owned()))
        })
        .collect();

    let mut graphs = BTreeMap::new();
    for name in names.iter().filter(|n| n.ends_with(".onnx")) {
        let mut member = zf.by_name(name).map_err(|e| model_err(format!(
            "member {name:?} unreadable: {e}"
        )))?;
        let mut bytes = Vec::with_capacity(member.size() as usize);
        member
            .read_to_end(&mut bytes)
            .map_err(|e| model_err(format!("member {name:?} read failed: {e}")))?;

        // C3: uncovered members MUST NOT load.
        let recorded = manifest.sha256.get(name).ok_or_else(|| {
            model_err(format!("{name} is not covered by metadata sha256"))
        })?;
        let actual = sha256_hex(&bytes);
        if actual != normalize_digest(recorded)? {
            return Err(model_err(format!(
                "{name} sha256 mismatch: zip has {actual}, metadata says {recorded}"
            )));
        }
        graphs.insert(name.clone(), bytes);
    }

    if !graphs.contains_key("encoder.onnx") || !graphs.contains_key("decoder.onnx") {
        return Err(model_err(format!(
            "{} is missing verified encoder/decoder members",
            zip_path.display()
        )));
    }
    Ok(ArtifactGraphs { manifest, graphs })
}

/// Parse `metadata.yaml` from the artifact zip and enforce the §3
/// format contract (load_manifest parity).
pub fn load_manifest(zip_path: &Path) -> Result<Manifest> {
    let file = std::fs::File::open(zip_path).map_err(|e| model_err(format!(
        "cannot open artifact {}: {e}",
        zip_path.display()
    )))?;
    let mut zf = zip::ZipArchive::new(file)
        .map_err(|e| model_err(format!("{} is not a readable zip: {e}", zip_path.display())))?;

    for required in ["metadata.yaml", "encoder.onnx", "decoder.onnx"] {
        if zf.by_name(required).is_err() {
            return Err(model_err(format!("missing required file: {required}")));
        }
    }

    let mut meta = zf.by_name("metadata.yaml").map_err(|e| {
        model_err(format!("metadata.yaml unreadable: {e}"))
    })?;
    let mut raw = String::with_capacity(meta.size() as usize);
    meta.read_to_string(&mut raw)
        .map_err(|e| model_err(format!("metadata.yaml read failed: {e}")))?;

    #[derive(serde::Deserialize)]
    struct MetadataYaml {
        format: String,
        id: String,
        task: String,
        tokenizer: String,
        #[serde(default = "default_decoder")]
        decoder: String,
        #[serde(default = "default_precision")]
        precision: String,
        #[serde(default = "default_opset")]
        opset: u32,
        #[serde(default)]
        sha256: BTreeMap<String, String>,
    }
    fn default_decoder() -> String {
        "plain".into()
    }
    fn default_precision() -> String {
        "fp32".into()
    }
    fn default_opset() -> u32 {
        14
    }

    let raw: MetadataYaml = serde_yaml::from_str(&raw)
        .map_err(|e| model_err(format!("metadata.yaml parse failed: {e}")))?;

    // §3: any other format / tokenizer MUST be rejected.
    if raw.format != "imf-v1" {
        return Err(model_err(format!("unsupported format: {:?}", raw.format)));
    }
    if raw.tokenizer != "bytes" {
        return Err(model_err(format!(
            "tokenizer {:?}: this runtime is byte-level only",
            raw.tokenizer
        )));
    }
    let decoder = match raw.decoder.as_str() {
        "plain" => DecoderMode::Plain,
        "kv" => DecoderMode::Kv,
        other => return Err(model_err(format!("unknown decoder mode {other:?}"))),
    };

    let mut sha256 = BTreeMap::new();
    for (k, v) in raw.sha256 {
        sha256.insert(k, normalize_digest(&v)?);
    }

    Ok(Manifest {
        id: raw.id,
        task: raw.task,
        decoder,
        precision: raw.precision,
        opset: raw.opset,
        sha256,
    })
}

/// Whole-artifact sha256 (streaming; SPEC.md C2 — cache hits are
/// re-verified, never trusted blindly).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| model_err(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| model_err(format!("read failed on {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal IMF v1 zip for loader tests. `decoder_digest`
    /// overrides the recorded decoder digest source (to fabricate a
    /// mismatch: record the digest of DIFFERENT bytes than the member).
    fn write_test_zip(path: &Path, encoder: &[u8], decoder: &[u8], decoder_digest: Option<&[u8]>) {
        let file = std::fs::File::create(path).unwrap();
        let mut zf = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        let digest = match decoder_digest {
            Some(data) => sha256_hex(data),
            None => sha256_hex(decoder),
        };
        let metadata = format!(
            "format: imf-v1\nid: test-1.0\ntask: g2p\ntokenizer: bytes\ndecoder: plain\nopset: 14\nsha256:\n  encoder.onnx: {}\n  decoder.onnx: {}\n",
            sha256_hex(encoder),
            digest
        );
        zf.start_file("metadata.yaml", opts).unwrap();
        zf.write_all(metadata.as_bytes()).unwrap();
        zf.start_file("encoder.onnx", opts).unwrap();
        zf.write_all(encoder).unwrap();
        zf.start_file("decoder.onnx", opts).unwrap();
        zf.write_all(decoder).unwrap();
        zf.finish().unwrap();
    }

    #[test]
    fn verifies_members_and_manifest() {
        let dir = std::env::temp_dir().join("iscrs-loader-test");
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("ok.zip");
        write_test_zip(&zip_path, b"encoder-bytes", b"decoder-bytes", None);

        let art = verify_and_read(&zip_path).unwrap();
        assert_eq!(art.manifest.id, "test-1.0");
        assert_eq!(art.manifest.decoder, DecoderMode::Plain);
        assert_eq!(art.graphs["encoder.onnx"], b"encoder-bytes");
    }

    #[test]
    fn rejects_uncovered_and_mismatched_members() {
        let dir = std::env::temp_dir().join("iscrs-loader-test");
        std::fs::create_dir_all(&dir).unwrap();

        // member carries "other" while the manifest records the digest
        // of "decoder-bytes" -> C3 mismatch.
        let bad = dir.join("bad.zip");
        write_test_zip(&bad, b"encoder-bytes", b"other", Some(b"decoder-bytes"));
        match verify_and_read(&bad) {
            Err(Error::Model(msg)) => assert!(msg.contains("sha256 mismatch"), "{msg}"),
            other => panic!("expected sha256 mismatch, got {other:?}"),
        }

        // an .onnx member NOT covered by the manifest (decoder-kv.onnx
        // with no sha256 entry) -> MUST NOT load.
        let uncovered = dir.join("uncovered.zip");
        let file = std::fs::File::create(&uncovered).unwrap();
        let mut zf = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        let metadata = format!(
            "format: imf-v1\nid: test-1.0\ntask: g2p\ntokenizer: bytes\nsha256:\n  encoder.onnx: {}\n  decoder.onnx: {}\n",
            sha256_hex(b"e"),
            sha256_hex(b"d")
        );
        zf.start_file("metadata.yaml", opts).unwrap();
        zf.write_all(metadata.as_bytes()).unwrap();
        zf.start_file("encoder.onnx", opts).unwrap();
        zf.write_all(b"e").unwrap();
        zf.start_file("decoder.onnx", opts).unwrap();
        zf.write_all(b"d").unwrap();
        zf.start_file("decoder-kv.onnx", opts).unwrap();
        zf.write_all(b"kv").unwrap();
        zf.finish().unwrap();
        match verify_and_read(&uncovered) {
            Err(Error::Model(msg)) => assert!(msg.contains("not covered"), "{msg}"),
            other => panic!("expected uncovered-member error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_imf_format_and_non_bytes_tokenizer() {
        let dir = std::env::temp_dir().join("iscrs-loader-test");
        std::fs::create_dir_all(&dir).unwrap();

        let file = std::fs::File::create(dir.join("fmt.zip")).unwrap();
        let mut zf = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zf.start_file("metadata.yaml", opts).unwrap();
        zf.write_all(b"format: imf-v2\nid: x\ntask: t\ntokenizer: bytes\n").unwrap();
        zf.start_file("encoder.onnx", opts).unwrap();
        zf.write_all(b"e").unwrap();
        zf.start_file("decoder.onnx", opts).unwrap();
        zf.write_all(b"d").unwrap();
        zf.finish().unwrap();

        match load_manifest(&dir.join("fmt.zip")) {
            Err(Error::Model(msg)) => assert!(msg.contains("unsupported format"), "{msg}"),
            other => panic!("expected format rejection, got {other:?}"),
        }
    }
}
