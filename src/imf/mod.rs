//! IMF v1 artifact support (interscript-models SPEC.md, normative):
//! byte tokenizer, checksum-verified zip loading, the models.yaml
//! registry, ONNX greedy single-row inference, and the frozen
//! conformance-fixture harness.

pub mod conformance;
pub mod infer;
pub mod loader;
pub mod registry;
pub mod tokens;
