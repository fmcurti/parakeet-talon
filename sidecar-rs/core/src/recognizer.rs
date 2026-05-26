use anyhow::Result;
use crossbeam_channel::Receiver;
use std::time::Instant;

use crate::protocol::Event;

/// A speech-to-text backend. Implemented per engine binary (parakeet, qwen).
/// `transcribe` receives one VAD-segmented utterance as mono f32 @ 16 kHz and
/// returns the recognized text (empty string if nothing was recognized).
pub trait Transcriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}

// Chunks arrive from the audio thread at 160 ms each (2560 f32 @ 16 kHz).
const CHUNK_MS: u32 = 160;

/// VAD parameters, tunable at startup via env vars.
struct VadParams {
    energy_threshold: f32,
    start_chunks: u32,
    end_chunks: u32,
    min_chunks: u32,
    max_chunks: u32,
}

impl VadParams {
    fn from_env() -> Self {
        let threshold = env_f32("PARAKEET_VAD_THRESHOLD", 0.005);
        let start_ms = env_u32("PARAKEET_VAD_START_MS", 160);
        let end_ms = env_u32("PARAKEET_VAD_END_MS", 800);
        let min_ms = env_u32("PARAKEET_VAD_MIN_MS", 320);
        let max_ms = env_u32("PARAKEET_VAD_MAX_MS", 10_000);
        let p = Self {
            energy_threshold: threshold,
            start_chunks: ms_to_chunks(start_ms).max(1),
            end_chunks: ms_to_chunks(end_ms).max(1),
            min_chunks: ms_to_chunks(min_ms).max(1),
            max_chunks: ms_to_chunks(max_ms).max(1),
        };
        eprintln!(
            "[sidecar] vad: threshold={} start={} ms end={} ms min={} ms max={} ms",
            p.energy_threshold,
            p.start_chunks * CHUNK_MS,
            p.end_chunks * CHUNK_MS,
            p.min_chunks * CHUNK_MS,
            p.max_chunks * CHUNK_MS,
        );
        p
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn ms_to_chunks(ms: u32) -> u32 {
    ms.div_ceil(CHUNK_MS)
}

/// Drive VAD over incoming audio chunks, handing each detected utterance to the
/// transcriber and emitting the resulting phrase.
pub fn run_recognizer<F: Fn(Event)>(
    mut transcriber: Box<dyn Transcriber>,
    rx: Receiver<Vec<f32>>,
    emit: F,
) -> Result<()> {
    let vad = VadParams::from_env();

    let mut buf: Vec<f32> = Vec::with_capacity(16_000 * 11);
    let mut speaking = false;
    let mut above_streak: u32 = 0;
    let mut below_streak: u32 = 0;
    let mut chunks_in_phrase: u32 = 0;

    while let Ok(chunk) = rx.recv() {
        let r = rms(&chunk);
        let loud = r >= vad.energy_threshold;

        if !speaking {
            if loud {
                above_streak += 1;
                if above_streak >= vad.start_chunks {
                    speaking = true;
                    buf.clear();
                    buf.extend_from_slice(&chunk);
                    chunks_in_phrase = 1;
                    above_streak = 0;
                    below_streak = 0;
                }
            } else {
                above_streak = 0;
            }
            continue;
        }

        // In an utterance.
        buf.extend_from_slice(&chunk);
        chunks_in_phrase += 1;
        if loud {
            below_streak = 0;
        } else {
            below_streak += 1;
        }

        let ended = below_streak >= vad.end_chunks || chunks_in_phrase >= vad.max_chunks;
        if !ended {
            continue;
        }

        // End of utterance: hand the buffer to the transcriber.
        speaking = false;
        let count = chunks_in_phrase;
        chunks_in_phrase = 0;
        above_streak = 0;
        below_streak = 0;

        if count < vad.min_chunks {
            buf.clear();
            continue;
        }

        let audio = std::mem::take(&mut buf);
        let dur_ms = audio.len() as u32 / 16; // 16 samples per ms at 16 kHz
        let t0 = Instant::now();
        match transcriber.transcribe(&audio) {
            Ok(text) => {
                let ms = t0.elapsed().as_millis();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    eprintln!("[sidecar] decoded {dur_ms} ms audio in {ms} ms -> {text:?}");
                    emit(Event::Phrase { text });
                } else {
                    eprintln!("[sidecar] empty decode for {dur_ms} ms audio ({ms} ms)");
                }
            }
            Err(e) => emit(Event::Error {
                msg: format!("transcribe: {e}"),
            }),
        }
        buf.reserve(16_000 * 11);
    }

    Ok(())
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|x| x * x).sum();
    (sum_sq / samples.len() as f32).sqrt()
}
