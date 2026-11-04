//! ONNX inference for IMF v1 artifacts: the greedy single-row decode
//! loop (the artifact contract per CONFORMANCE.md) over a mockable
//! `InferenceEngine` trait, plus the `ort`-backed implementation.
//!
//! Byte-exactness contract (mirror of
//! `pipeline.conformance.ort_single_row_decoder`, the reference the
//! fixtures were generated with):
//! * encode: ids = UTF-8 byte + 3, one trailing EOS; int64, one row;
//! * encoder: one row per call, no padding, no attention mask;
//! * decoder: greedy argmax until EOS, 512-step cap, prefix
//!   `[PAD]` then growing;
//! * output: `max(0, t - 3)` per id, lossy UTF-8.

use crate::error::{Error, Result};
use crate::imf::loader::ArtifactGraphs;
use crate::imf::tokens;

fn model_err(what: impl Into<String>) -> Error {
    Error::Model(what.into())
}

/// The engine abstraction: one encoder pass + one decoder step at a
/// time. `ort` sessions implement it; tests drive the loop with a
/// deterministic mock.
pub trait InferenceEngine {
    /// Encoder: single-row `input_ids` -> hidden states, shape
    /// (1, seq, d) flattened, row-major.
    fn encode(&mut self, input_ids: &[i64]) -> Result<Vec<f32>>;
    /// Hidden-state tensor dims (1, seq, d) — needed to build the
    /// decoder's `encoder_hidden_states` input.
    fn hidden_dims(&self, seq_len: usize) -> Vec<i64>;
    /// Decoder step: full prefix `decoder_ids` -> logits of the last
    /// position (vocab-sized).
    fn decode_step(&mut self, decoder_ids: &[i64], hidden: &[f32]) -> Result<Vec<f32>>;
}

/// The greedy single-row decode loop (model.generate + tokens.decode
/// parity). Empty input (only the trailing EOS) short-circuits to no
/// tokens, exactly like the reference loader.
pub fn greedy_generate(
    engine: &mut dyn InferenceEngine,
    text: &str,
    max_len: usize,
) -> Result<Vec<i64>> {
    let ids = tokens::encode(text);
    if ids.len() == 1 {
        // only the trailing EOS: empty input
        return Ok(Vec::new());
    }
    let hidden = engine.encode(&ids)?;
    let mut decoder_ids = vec![tokens::PAD_ID];
    let mut generated = Vec::new();
    for _ in 0..max_len {
        let logits = engine.decode_step(&decoder_ids, &hidden)?;
        // numpy argmax parity: first index of the maximum wins.
        let token = argmax(&logits).ok_or_else(|| model_err("decoder produced empty logits"))?;
        if token == tokens::EOS_ID {
            break;
        }
        generated.push(token);
        decoder_ids.push(token);
    }
    Ok(generated)
}

/// `translate(text)`: greedy ids -> text. Conformance fixtures were
/// generated with the `max(0, t - 3)` byte rule (see
/// `ids_to_fixture_bytes`); `tokens::decode` implements the SPEC §4
/// table (skip pad/unk, mod 256). This function uses the SPEC table.
pub fn translate(engine: &mut dyn InferenceEngine, text: &str, max_len: usize) -> Result<String> {
    let ids = greedy_generate(engine, text, max_len)?;
    Ok(tokens::decode(&ids))
}

/// The fixture-generation byte rule: `max(0, t - 3)` per id (no
/// pad/unk skipping — a PAD argmax yields byte 0x00 there), then
/// lossy UTF-8. Used by the conformance harness so frozen vectors are
/// compared byte-exactly under the rule that produced them.
pub fn ids_to_fixture_bytes(ids: &[i64]) -> Vec<u8> {
    ids.iter()
        .map(|&t| std::cmp::max(0, t - tokens::BYTE_OFFSET) as u8)
        .collect()
}

pub fn ids_to_fixture_string(ids: &[i64]) -> String {
    String::from_utf8_lossy(&ids_to_fixture_bytes(ids)).into_owned()
}

fn argmax(data: &[f32]) -> Option<i64> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &v) in data.iter().enumerate() {
        match best {
            Some((_, bv)) if bv >= v => {}
            _ => best = Some((i, v)),
        }
    }
    best.map(|(i, _)| i as i64)
}

/// Decode-step step cap. CONFORMANCE.md: "greedy argmax until EOS,
/// 512-step cap".
pub const MAX_DECODE_STEPS: usize = 512;

// ----------------------------------------------------------------------
// ort-backed engine
// ----------------------------------------------------------------------

/// An `ort`-loaded IMF artifact: encoder + decoder sessions plus the
/// ONNX Runtime environment they share.
pub struct OrtEngine {
    encoder: ort::session::Session,
    decoder: ort::session::Session,
    encoder_input: String,
    decoder_inputs: [String; 2],
    hidden_seq: usize,
    hidden_dim: usize,
}

/// onnxruntime allows ONE environment per process (CreateEnv twice
/// errors out), and sessions keep their environment alive internally —
/// so it lives in a process-global slot (the Result caches init
/// failures so a broken install fails identically on every call).
fn ort_environment() -> Result<&'static ort::environment::Environment> {
    static ENV: std::sync::OnceLock<std::result::Result<ort::environment::Environment, String>> =
        std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_name("iscrs")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| model_err(format!("onnxruntime environment init failed: {e}")))
}

impl OrtEngine {
    /// Build sessions from verified graphs (`decoder-kv.onnx` is
    /// selected for kv artifacts per the reference loader; this slice
    /// implements the plain greedy loop — kv sessions decode through
    /// the plain graph).
    pub fn from_graphs(graphs: &ArtifactGraphs) -> Result<OrtEngine> {
        let intra_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let intra_threads = std::cmp::max(1, intra_threads / 2);

        let env = ort_environment()?;

        let commit = |bytes: &[u8]| -> Result<ort::session::Session> {
            let builder = ort::session::Session::builder(env)
                .map_err(|e| model_err(format!("session builder failed: {e}")))?;
            let builder = builder
                .with_optimization_level(
                    ort::session::builder::GraphOptimizationLevel::Level3,
                )
                .map_err(|e| model_err(format!("optimization level failed: {e}")))?;
            let builder = builder
                .with_intra_threads(intra_threads)
                .map_err(|e| model_err(format!("intra threads failed: {e}")))?;
            let mut builder = builder;
            builder
                .commit_from_memory(bytes)
                .map_err(|e| model_err(format!("onnx session load failed: {e}")))
        };

        let encoder = commit(&graphs.graphs["encoder.onnx"])?;
        let decoder_name = graphs.decoder_member()?;
        let decoder = commit(&graphs
            .graphs
            .get(decoder_name)
            .ok_or_else(|| model_err(format!("missing verified member {decoder_name}")))?)?;

        let encoder_input = encoder
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or_else(|| model_err("encoder graph has no inputs"))?;
        let dec_inputs: Vec<String> = decoder
            .inputs()
            .iter()
            .take(2)
            .map(|i| i.name().to_string())
            .collect();
        if dec_inputs.len() != 2 {
            return Err(model_err(
                "decoder graph lacks input_ids/encoder_hidden_states inputs",
            ));
        }
        let decoder_inputs = [dec_inputs[0].clone(), dec_inputs[1].clone()];

        Ok(OrtEngine {
            encoder,
            decoder,
            encoder_input,
            decoder_inputs,
            hidden_seq: 0,
            hidden_dim: 0,
        })
    }
}

impl InferenceEngine for OrtEngine {
    fn encode(&mut self, input_ids: &[i64]) -> Result<Vec<f32>> {
        let seq = input_ids.len();
        let tensor = ort::value::Tensor::from_array(([1usize, seq], input_ids.to_vec()))
            .map_err(|e| model_err(format!("input_ids tensor build failed: {e}")))?;
        let key = self
            .encoder
            .outputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or_else(|| model_err("encoder graph has no outputs"))?;
        let outputs = self
            .encoder
            .run(ort::inputs![self.encoder_input.as_str() => tensor])
            .map_err(|e| model_err(format!("encoder run failed: {e}")))?;
        let (shape, data) = outputs
            .get(key.as_str())
            .ok_or_else(|| model_err(format!("encoder output {key:?} missing")))?
            .try_extract_tensor::<f32>()
            .map_err(|e| model_err(format!("encoder output extract failed: {e}")))?;
        // shape = [1, seq, d]
        let dims: Vec<i64> = shape.iter().copied().collect();
        self.hidden_seq = dims.get(1).copied().unwrap_or(seq as i64) as usize;
        self.hidden_dim = dims
            .get(2)
            .copied()
            .unwrap_or((data.len() / seq.max(1)) as i64) as usize;
        Ok(data.to_vec())
    }

    fn hidden_dims(&self, seq_len: usize) -> Vec<i64> {
        let seq = if self.hidden_seq == 0 { seq_len } else { self.hidden_seq };
        let d = if self.hidden_dim == 0 { 1 } else { self.hidden_dim };
        vec![1, seq as i64, d as i64]
    }

    fn decode_step(&mut self, decoder_ids: &[i64], hidden: &[f32]) -> Result<Vec<f32>> {
        let cur_len = decoder_ids.len();
        let ids = ort::value::Tensor::from_array(([1usize, cur_len], decoder_ids.to_vec()))
            .map_err(|e| model_err(format!("decoder input_ids tensor build failed: {e}")))?;
        let dims = self.hidden_dims(hidden.len());
        let states = ort::value::Tensor::from_array((dims, hidden.to_vec()))
            .map_err(|e| model_err(format!("encoder_hidden_states tensor build failed: {e}")))?;
        let key = self
            .decoder
            .outputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or_else(|| model_err("decoder graph has no outputs"))?;
        let outputs = self
            .decoder
            .run(ort::inputs![
                self.decoder_inputs[0].as_str() => ids,
                self.decoder_inputs[1].as_str() => states,
            ])
            .map_err(|e| model_err(format!("decoder run failed: {e}")))?;
        let (shape, data) = outputs
            .get(key.as_str())
            .ok_or_else(|| model_err(format!("decoder output {key:?} missing")))?
            .try_extract_tensor::<f32>()
            .map_err(|e| model_err(format!("decoder output extract failed: {e}")))?;
        // shape = [1, cur_len, vocab]; the greedy step reads the last
        // position only.
        let dims: Vec<i64> = shape.iter().copied().collect();
        let vocab = dims
            .get(2)
            .copied()
            .unwrap_or((data.len() / cur_len.max(1)) as i64) as usize;
        let start = (cur_len - 1) * vocab;
        let end = (start + vocab).min(data.len());
        if start >= data.len() {
            return Err(model_err("decoder logits too short for prefix length"));
        }
        Ok(data[start..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic mock: emits `aa` for input "x", then EOS. Lets
    /// the loop be tested without onnxruntime.
    struct MockEngine {
        steps: std::cell::RefCell<Vec<Vec<i64>>>,
    }

    impl MockEngine {
        fn new() -> Self {
            MockEngine {
                steps: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl InferenceEngine for MockEngine {
        fn encode(&mut self, input_ids: &[i64]) -> Result<Vec<f32>> {
            assert_eq!(input_ids.last(), Some(&tokens::EOS_ID));
            assert_eq!(input_ids[0], b'x' as i64 + tokens::BYTE_OFFSET);
            Ok(vec![1.0])
        }
        fn hidden_dims(&self, _seq: usize) -> Vec<i64> {
            vec![1, 1, 1]
        }
        fn decode_step(&mut self, decoder_ids: &[i64], _hidden: &[f32]) -> Result<Vec<f32>> {
            self.steps.borrow_mut().push(decoder_ids.to_vec());
            let vocab = 300i64;
            let mut logits = vec![0.0f32; vocab as usize];
            match decoder_ids.len() {
                1 => logits['a' as i64 as usize] = 5.0, // -> "a"
                2 => logits['a' as i64 as usize] = 4.0, // -> "a" (tie-free repeat)
                _ => logits[tokens::EOS_ID as usize] = 3.0, // -> EOS
            }
            Ok(logits)
        }
    }

    #[test]
    fn greedy_loop_stops_at_eos_and_grows_prefix() {
        let mut engine = MockEngine::new();
        let ids = greedy_generate(&mut engine, "x", MAX_DECODE_STEPS).unwrap();
        assert_eq!(ids, vec!['a' as i64, 'a' as i64]);
        // prefix starts at [PAD] and grows with each emitted token
        assert_eq!(engine.steps.borrow()[0], vec![tokens::PAD_ID]);
        assert_eq!(
            engine.steps.borrow()[1],
            vec![tokens::PAD_ID, 'a' as i64]
        );
    }

    #[test]
    fn empty_input_short_circuits() {
        let mut engine = MockEngine::new();
        assert!(greedy_generate(&mut engine, "", MAX_DECODE_STEPS)
            .unwrap()
            .is_empty());
        assert!(engine.steps.borrow().is_empty());
    }

    #[test]
    fn max_len_cap_bounds_the_loop() {
        struct NeverEos;
        impl InferenceEngine for NeverEos {
            fn encode(&mut self, _ids: &[i64]) -> Result<Vec<f32>> {
                Ok(vec![1.0])
            }
            fn hidden_dims(&self, _s: usize) -> Vec<i64> {
                vec![1, 1, 1]
            }
            fn decode_step(&mut self, _ids: &[i64], _h: &[f32]) -> Result<Vec<f32>> {
                let mut logits = vec![0.0f32; 300];
                logits[100] = 1.0;
                Ok(logits)
            }
        }
        let mut engine = NeverEos;
        let ids = greedy_generate(&mut engine, "x", 10).unwrap();
        assert_eq!(ids.len(), 10);
        assert!(ids.iter().all(|&t| t == 100));
    }

    #[test]
    fn fixture_byte_rule_and_argmax_tiebreak() {
        // max(0, t-3): token 97 -> byte 94 ('^'); pad/unk are NOT
        // skipped by the fixture rule (byte 0x00), unlike tokens::decode.
        assert_eq!(ids_to_fixture_bytes(&[97, 2, 0, 101]), b"^\x00\x00b");
        // the SPEC tokens::decode DOES skip them: (97-3)='^', 0/2 skipped
        assert_eq!(crate::imf::tokens::decode(&[97, 2, 0, 101]), "^b");
        assert_eq!(argmax(&[1.0, 3.0, 3.0]), Some(1)); // first max wins
        assert_eq!(argmax(&[]), None);
    }
}
