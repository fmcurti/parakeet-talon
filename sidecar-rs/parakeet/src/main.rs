//! Parakeet TDT engine sidecar: ONNX inference via `parakeet-rs`.

use anyhow::Result;
use parakeet_rs::{ExecutionConfig, ExecutionProvider, ParakeetTDT, Transcriber as _};
use sidecar_core::Transcriber;
use std::path::Path;

const HF_REPO: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const FILES: &[&str] = &[
    "config.json",
    "encoder-model.onnx",
    "encoder-model.onnx.data",
    "decoder_joint-model.onnx",
    "nemo128.onnx",
    "vocab.txt",
];

struct ParakeetTranscriber {
    model: ParakeetTDT,
}

impl ParakeetTranscriber {
    fn new(model_dir: &Path) -> Result<Self> {
        let config = ExecutionConfig::new().with_execution_provider(ExecutionProvider::Cpu);
        let model = ParakeetTDT::from_pretrained(model_dir, Some(config))?;
        Ok(Self { model })
    }
}

impl Transcriber for ParakeetTranscriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        let result = self.model.transcribe_samples(samples.to_vec(), 16_000, 1, None)?;
        Ok(result.text)
    }
}

fn main() -> Result<()> {
    sidecar_core::run(|| {
        let dir = sidecar_core::model_dir("parakeet-tdt-v3")?;
        sidecar_core::ensure_files(&dir, HF_REPO, FILES)?;
        let t: Box<dyn Transcriber> = Box::new(ParakeetTranscriber::new(&dir)?);
        Ok(t)
    })
}
