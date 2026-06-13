use anyhow::Result;
use crossbeam_channel::Receiver;
use std::collections::VecDeque;
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
// Sub-window for loudness detection: 32 ms @ 16 kHz, 5 windows per chunk.
// A chunk counts as loud if ANY window crosses the threshold, so a word
// starting mid-chunk isn't averaged below the floor by the silence before it.
const LOUD_WIN: usize = 512;

/// VAD parameters, tunable at startup via env vars.
struct VadParams {
    energy_threshold: f32,
    start_chunks: u32,
    end_chunks: u32,
    min_chunks: u32,
    max_chunks: u32,
    preroll_chunks: u32,
}

impl VadParams {
    fn from_env() -> Self {
        let threshold = env_f32("PARAKEET_VAD_THRESHOLD", 0.005);
        let start_ms = env_u32("PARAKEET_VAD_START_MS", 160);
        let end_ms = env_u32("PARAKEET_VAD_END_MS", 800);
        let min_ms = env_u32("PARAKEET_VAD_MIN_MS", 320);
        let max_ms = env_u32("PARAKEET_VAD_MAX_MS", 10_000);
        let preroll_ms = env_u32("PARAKEET_VAD_PREROLL_MS", 320);
        let start_chunks = ms_to_chunks(start_ms).max(1);
        let p = Self {
            energy_threshold: threshold,
            start_chunks,
            end_chunks: ms_to_chunks(end_ms).max(1),
            min_chunks: ms_to_chunks(min_ms).max(1),
            max_chunks: ms_to_chunks(max_ms).max(1),
            // The pre-roll must hold at least the start_chunks-1 loud chunks
            // that precede the trigger, or they would be dropped from the phrase.
            preroll_chunks: ms_to_chunks(preroll_ms).max(start_chunks - 1),
        };
        eprintln!(
            "[sidecar] vad: threshold={} start={} ms end={} ms min={} ms max={} ms preroll={} ms",
            p.energy_threshold,
            p.start_chunks * CHUNK_MS,
            p.end_chunks * CHUNK_MS,
            p.min_chunks * CHUNK_MS,
            p.max_chunks * CHUNK_MS,
            p.preroll_chunks * CHUNK_MS,
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
    transcriber: Box<dyn Transcriber>,
    rx: Receiver<Vec<f32>>,
    emit: F,
) -> Result<()> {
    run_with_params(VadParams::from_env(), transcriber, rx, emit)
}

fn run_with_params<F: Fn(Event)>(
    vad: VadParams,
    mut transcriber: Box<dyn Transcriber>,
    rx: Receiver<Vec<f32>>,
    emit: F,
) -> Result<()> {
    let mut buf: Vec<f32> = Vec::with_capacity(16_000 * 11);
    // While idle, the last few chunks are kept so the phrase can start before
    // the trigger: word onsets (quiet consonants, speech starting mid-chunk)
    // live there, and they double as leading silence padding for the model.
    let mut preroll: VecDeque<Vec<f32>> = VecDeque::with_capacity(vad.preroll_chunks as usize + 1);
    let mut speaking = false;
    let mut above_streak: u32 = 0;
    let mut below_streak: u32 = 0;
    let mut chunks_in_phrase: u32 = 0;

    while let Ok(chunk) = rx.recv() {
        let loud = is_loud(&chunk, vad.energy_threshold);

        if !speaking {
            if loud {
                above_streak += 1;
                if above_streak >= vad.start_chunks {
                    speaking = true;
                    buf.clear();
                    for past in preroll.drain(..) {
                        buf.extend_from_slice(&past);
                    }
                    buf.extend_from_slice(&chunk);
                    chunks_in_phrase = 1;
                    above_streak = 0;
                    below_streak = 0;
                    continue;
                }
            } else {
                above_streak = 0;
            }
            if vad.preroll_chunks > 0 {
                if preroll.len() as u32 == vad.preroll_chunks {
                    preroll.pop_front();
                }
                preroll.push_back(chunk);
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
        // Length check on the speech itself, excluding the trailing-silence
        // run — otherwise end_chunks alone pushes every blip past the minimum.
        let speech_chunks = chunks_in_phrase - below_streak;
        chunks_in_phrase = 0;
        above_streak = 0;
        below_streak = 0;

        if speech_chunks < vad.min_chunks {
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

fn is_loud(chunk: &[f32], threshold: f32) -> bool {
    chunk.chunks(LOUD_WIN).any(|w| rms(w) >= threshold)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|x| x * x).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use std::sync::{Arc, Mutex};

    const CHUNK_LEN: usize = crate::audio::CHUNK_LEN;

    fn params() -> VadParams {
        VadParams {
            energy_threshold: 0.005,
            start_chunks: 1,
            end_chunks: 5,
            min_chunks: 2,
            max_chunks: 62,
            preroll_chunks: 2,
        }
    }

    /// Records the sample count of every utterance it receives.
    struct LenRecorder(Arc<Mutex<Vec<usize>>>);

    impl Transcriber for LenRecorder {
        fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
            self.0.lock().unwrap().push(samples.len());
            Ok("ok".into())
        }
    }

    fn run_vad(vad: VadParams, chunks: Vec<Vec<f32>>) -> Vec<usize> {
        let lens = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded();
        for c in chunks {
            tx.send(c).unwrap();
        }
        drop(tx);
        run_with_params(vad, Box::new(LenRecorder(lens.clone())), rx, |_| {}).unwrap();
        let out = lens.lock().unwrap().clone();
        out
    }

    fn silence() -> Vec<f32> {
        vec![0.0; CHUNK_LEN]
    }

    fn loud() -> Vec<f32> {
        vec![0.1; CHUNK_LEN]
    }

    #[test]
    fn preroll_is_prepended_to_phrase() {
        let mut chunks = vec![silence(); 4];
        chunks.extend(vec![loud(); 3]);
        chunks.extend(vec![silence(); 5]);
        let lens = run_vad(params(), chunks);
        // 2 pre-roll + 3 speech + 5 trailing silence
        assert_eq!(lens, vec![10 * CHUNK_LEN]);
    }

    #[test]
    fn single_chunk_blip_is_discarded() {
        let mut chunks = vec![silence(); 4];
        chunks.push(loud());
        chunks.extend(vec![silence(); 6]);
        let lens = run_vad(params(), chunks);
        // 1 speech chunk < min_chunks=2, dropped despite trailing silence
        assert!(lens.is_empty());
    }

    #[test]
    fn onset_mid_chunk_still_triggers() {
        // Speech in only the last 32 ms window: whole-chunk RMS is diluted
        // below the threshold, but the window RMS is not.
        let mut chunk = vec![0.0; CHUNK_LEN];
        for s in &mut chunk[CHUNK_LEN - LOUD_WIN..] {
            *s = 0.008;
        }
        assert!(rms(&chunk) < 0.005);
        assert!(is_loud(&chunk, 0.005));
    }

    #[test]
    fn loud_chunks_before_trigger_are_kept() {
        // start_chunks=3: the two loud chunks before the trigger chunk must
        // come back out of the pre-roll instead of being dropped.
        let vad = VadParams {
            start_chunks: 3,
            ..params()
        };
        let mut chunks = vec![silence(); 4];
        chunks.extend(vec![loud(); 4]);
        chunks.extend(vec![silence(); 5]);
        let lens = run_vad(vad, chunks);
        // pre-roll holds 2 chunks (the loud ones), trigger at 3rd loud chunk:
        // 2 pre-roll + 2 speech after trigger + 5 trailing silence
        assert_eq!(lens, vec![9 * CHUNK_LEN]);
    }
}
