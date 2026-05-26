//! Qwen3-ASR engine sidecar: pure-Rust inference via the `qwen3-asr` crate.

use anyhow::{anyhow, Result};
use qwen3_asr::{AsrInference, StreamingOptions};
use sidecar_core::Transcriber;
use std::path::Path;

// HuggingFace repo. `from_pretrained` downloads config.json + model.safetensors
// and reconstructs tokenizer.json from vocab.json/merges.txt/tokenizer_config.json
// (the repo ships no tokenizer.json), caching under
// `<cache_dir>/Qwen--Qwen3-ASR-0.6B/` with a `.complete` marker.
const MODEL_ID: &str = "Qwen/Qwen3-ASR-0.6B";

struct QwenTranscriber {
    engine: AsrInference,
    language: Option<String>,
}

impl QwenTranscriber {
    fn load(cache_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(cache_dir)?;
        let device = qwen3_asr::best_device();
        let engine = AsrInference::from_pretrained(MODEL_ID, cache_dir, device)
            .map_err(|e| anyhow!("qwen load: {e}"))?;
        let language = forced_language();
        match &language {
            Some(l) => eprintln!("[sidecar] qwen: forcing language {l:?}"),
            None => eprintln!("[sidecar] qwen: language auto-detect"),
        }
        Ok(Self { engine, language })
    }
}

impl Transcriber for QwenTranscriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        // The VAD already segmented this utterance, so feed it all at once and
        // let `finish_streaming` decode the tail into the final transcription.
        let mut opts = StreamingOptions::default();
        if let Some(lang) = &self.language {
            opts = opts.with_language(lang.clone());
        }
        let mut state = self.engine.init_streaming(opts);
        self.engine
            .feed_audio(&mut state, samples)
            .map_err(|e| anyhow!("qwen feed_audio: {e}"))?;
        let result = self
            .engine
            .finish_streaming(&mut state)
            .map_err(|e| anyhow!("qwen finish_streaming: {e}"))?;
        Ok(strip_markers(&result.text))
    }
}

/// In forced-language mode the crate returns the raw generation, which still
/// contains the `<asr_text>` separator (and any residual special tokens) that
/// the auto-detect path would have stripped. Keep only the text after it.
fn strip_markers(text: &str) -> String {
    let body = match text.rfind("<asr_text>") {
        Some(i) => &text[i + "<asr_text>".len()..],
        None => text,
    };
    body.replace("<|im_end|>", "")
        .replace("<|endoftext|>", "")
        .trim()
        .to_string()
}

/// Language to force on the decoder. Defaults to English; override with the
/// `QWEN_LANGUAGE` env var (a lowercase language name like "spanish", or
/// "auto"/empty to enable Qwen's automatic language detection).
fn forced_language() -> Option<String> {
    match std::env::var("QWEN_LANGUAGE") {
        Ok(v) => {
            let v = v.trim().to_lowercase();
            if v.is_empty() || v == "auto" {
                None
            } else {
                Some(v)
            }
        }
        Err(_) => Some("english".to_string()),
    }
}

fn main() -> Result<()> {
    // Surface the crate's `log` output (model download progress) on stderr,
    // which the Talon plugin forwards to talon.log.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    sidecar_core::run(|| {
        // Cache models under sidecar-rs/models/ (gitignored), shared layout
        // with the parakeet engine.
        let cache_dir = sidecar_core::sidecar_root()?.join("models");
        let t: Box<dyn Transcriber> = Box::new(QwenTranscriber::load(&cache_dir)?);
        Ok(t)
    })
}
