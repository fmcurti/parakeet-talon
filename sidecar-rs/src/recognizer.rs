use anyhow::Result;
use crossbeam_channel::Receiver;
use parakeet_rs::{ExecutionConfig, ExecutionProvider, ParakeetEOU};
use std::path::Path;
use std::time::Instant;

use crate::protocol::Event;

const EOU_MARKER: &str = "[EOU]";
const CHUNK_MS: f64 = 160.0;
/// Flush the accumulator after this many consecutive empty fragments
/// if the [EOU] marker never arrived.
const SILENCE_FLUSH_CHUNKS: u32 = 4; // ~640 ms

pub fn run_recognizer<F: Fn(Event)>(
    model_dir: &Path,
    rx: Receiver<Vec<f32>>,
    emit: F,
) -> Result<()> {
    let config = ExecutionConfig::new().with_execution_provider(ExecutionProvider::Cpu);
    let mut parakeet = ParakeetEOU::from_pretrained(model_dir, Some(config))?;

    let mut acc = String::new();
    let mut empty_streak: u32 = 0;
    let mut slow_warns: u32 = 0;
    let mut chunk_count: u64 = 0;
    let mut total_ms: f64 = 0.0;

    let flush = |acc: &mut String, emit: &F| {
        let phrase = sp_to_text(acc);
        acc.clear();
        if !phrase.is_empty() {
            emit(Event::Phrase { text: phrase });
        }
    };

    while let Ok(chunk) = rx.recv() {
        let t0 = Instant::now();
        let fragment = match parakeet.transcribe(&chunk, true) {
            Ok(t) => t,
            Err(e) => {
                emit(Event::Error {
                    msg: format!("transcribe: {e}"),
                });
                continue;
            }
        };
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        total_ms += elapsed_ms;
        chunk_count += 1;
        if elapsed_ms > CHUNK_MS && slow_warns < 10 {
            eprintln!(
                "[sidecar] slow chunk: {elapsed_ms:.1} ms (budget {CHUNK_MS:.0} ms, rtf={:.2})",
                elapsed_ms / CHUNK_MS
            );
            slow_warns += 1;
        }
        if chunk_count.is_multiple_of(100) {
            eprintln!(
                "[sidecar] avg inference: {:.1} ms over {chunk_count} chunks (rtf={:.2})",
                total_ms / chunk_count as f64,
                (total_ms / chunk_count as f64) / CHUNK_MS
            );
        }

        if fragment.is_empty() {
            empty_streak = empty_streak.saturating_add(1);
            if !acc.is_empty() && empty_streak >= SILENCE_FLUSH_CHUNKS {
                flush(&mut acc, &emit);
                empty_streak = 0;
            }
            continue;
        }
        empty_streak = 0;
        acc.push_str(&fragment);

        // Flush complete phrases terminated by [EOU].
        while let Some(idx) = acc.find(EOU_MARKER) {
            let head: String = acc.drain(..idx).collect();
            acc.drain(..EOU_MARKER.len()); // remove the marker itself
            let phrase = sp_to_text(&head);
            if !phrase.is_empty() {
                emit(Event::Phrase { text: phrase });
            }
        }
    }

    // Channel closed: flush any trailing phrase.
    flush(&mut acc, &emit);

    Ok(())
}

/// Decode SentencePiece-style text: `▁` (U+2581) is a word boundary marker.
fn sp_to_text(raw: &str) -> String {
    let mut out = raw.replace('\u{2581}', " ");
    // Collapse whitespace and trim.
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out
}
