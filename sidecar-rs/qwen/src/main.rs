//! Qwen3-ASR engine sidecar: ONNX Runtime inference via the `sherpa-onnx` crate.
//!
//! Uses the int8-quantized Qwen3-ASR-0.6B model, which runs fast on CPU across
//! Windows/Linux/macOS (the same ONNX Runtime backend the parakeet engine uses).

use anyhow::{anyhow, Context, Result};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};
use sidecar_core::Transcriber;
use std::path::{Path, PathBuf};

const MODEL_NAME: &str = "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25";
const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2";

struct QwenTranscriber {
    recognizer: OfflineRecognizer,
}

impl QwenTranscriber {
    fn load(models_root: &Path) -> Result<Self> {
        let model_dir = ensure_model(models_root)?;
        let path = |name: &str| model_dir.join(name).to_string_lossy().into_owned();

        // sherpa auto-detects language; Qwen3-ASR has no forced-language option.
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.qwen3_asr.conv_frontend = Some(path("conv_frontend.onnx"));
        config.model_config.qwen3_asr.encoder = Some(path("encoder.int8.onnx"));
        config.model_config.qwen3_asr.decoder = Some(path("decoder.int8.onnx"));
        config.model_config.qwen3_asr.tokenizer = Some(path("tokenizer"));
        config.model_config.num_threads = num_threads();

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("failed to create sherpa-onnx Qwen3-ASR recognizer"))?;
        eprintln!(
            "[sidecar] qwen: sherpa-onnx recognizer ready ({} threads)",
            config.model_config.num_threads
        );
        Ok(Self { recognizer })
    }
}

impl Transcriber for QwenTranscriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(16_000, samples);
        self.recognizer.decode(&stream);
        let text = stream.get_result().map(|r| r.text).unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

fn num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(2)
        .clamp(1, 4)
}

/// Ensure the int8 model is present under `models_root/<MODEL_NAME>/`, fetching
/// and extracting the release tarball on first run.
fn ensure_model(models_root: &Path) -> Result<PathBuf> {
    let model_dir = models_root.join(MODEL_NAME);
    let encoder = model_dir.join("encoder.int8.onnx");
    if encoder.exists() {
        return Ok(model_dir);
    }
    std::fs::create_dir_all(models_root)?;
    let tarball = models_root.join(format!("{MODEL_NAME}.tar.bz2"));
    eprintln!("[sidecar] qwen: downloading model from {MODEL_URL}");
    sidecar_core::download_to(MODEL_URL, &tarball)?;
    eprintln!("[sidecar] qwen: extracting {}", tarball.display());
    extract_tar_bz2(&tarball, models_root)?;
    let _ = std::fs::remove_file(&tarball);
    if !encoder.exists() {
        return Err(anyhow!(
            "model extraction did not produce {}",
            encoder.display()
        ));
    }
    Ok(model_dir)
}

fn extract_tar_bz2(tarball: &Path, dest: &Path) -> Result<()> {
    let file =
        std::fs::File::open(tarball).with_context(|| format!("open {}", tarball.display()))?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(dest)
        .with_context(|| format!("unpack into {}", dest.display()))?;
    Ok(())
}

fn main() -> Result<()> {
    sidecar_core::run(|| {
        let models_root = sidecar_core::sidecar_root()?.join("models");
        let t: Box<dyn Transcriber> = Box::new(QwenTranscriber::load(&models_root)?);
        Ok(t)
    })
}
