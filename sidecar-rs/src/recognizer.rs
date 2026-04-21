use anyhow::Result;
use crossbeam_channel::Receiver;
use parakeet_rs::{ExecutionConfig, ExecutionProvider, ParakeetTDT, Transcriber};
use std::path::Path;
use std::time::Instant;

use crate::protocol::Event;

// Chunks arrive from the audio thread at 160 ms each (2560 f32 @ 16 kHz).
const VAD_ENERGY_THRESHOLD: f32 = 0.005;
const VAD_START_CHUNKS: u32 = 1;
const VAD_END_CHUNKS: u32 = 5; // ~800 ms trailing silence
const MIN_CHUNKS: u32 = 2; //   ~320 ms minimum phrase length
const MAX_CHUNKS: u32 = 63; //  ~10 s maximum phrase

pub fn run_recognizer<F: Fn(Event)>(
    model_dir: &Path,
    rx: Receiver<Vec<f32>>,
    emit: F,
) -> Result<()> {
    let config = ExecutionConfig::new().with_execution_provider(ExecutionProvider::Cpu);
    let mut parakeet = ParakeetTDT::from_pretrained(model_dir, Some(config))?;

    let mut buf: Vec<f32> = Vec::with_capacity(16_000 * 11);
    let mut speaking = false;
    let mut above_streak: u32 = 0;
    let mut below_streak: u32 = 0;
    let mut chunks_in_phrase: u32 = 0;

    while let Ok(chunk) = rx.recv() {
        let r = rms(&chunk);
        let loud = r >= VAD_ENERGY_THRESHOLD;

        if !speaking {
            if loud {
                above_streak += 1;
                if above_streak >= VAD_START_CHUNKS {
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

        let ended = below_streak >= VAD_END_CHUNKS || chunks_in_phrase >= MAX_CHUNKS;
        if !ended {
            continue;
        }

        // End of utterance: hand the buffer to TDT.
        speaking = false;
        let count = chunks_in_phrase;
        chunks_in_phrase = 0;
        above_streak = 0;
        below_streak = 0;

        if count < MIN_CHUNKS {
            buf.clear();
            continue;
        }

        let audio = std::mem::take(&mut buf);
        let dur_ms = audio.len() as u32 / 16; // 16 samples per ms at 16 kHz
        let t0 = Instant::now();
        match parakeet.transcribe_samples(audio, 16_000, 1, None) {
            Ok(result) => {
                let ms = t0.elapsed().as_millis();
                let text = result.text.trim().to_string();
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
