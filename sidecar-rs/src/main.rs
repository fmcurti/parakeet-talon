mod audio;
mod model;
mod protocol;
mod recognizer;

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Sender};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::thread;

use crate::audio::AudioStream;
use crate::protocol::{Command, Event};

struct Emitter {
    out: Mutex<std::io::Stdout>,
}

impl Emitter {
    fn new() -> Self {
        Self {
            out: Mutex::new(std::io::stdout()),
        }
    }
    fn emit(&self, ev: Event) {
        let line = match serde_json::to_string(&ev) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[sidecar] serialize event: {e}");
                return;
            }
        };
        if let Ok(mut h) = self.out.lock() {
            let _ = writeln!(h, "{line}");
            let _ = h.flush();
        }
    }
}

/// Return the sidecar-rs directory, found by walking up from the binary.
fn sidecar_root() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("current_exe")?;
    let mut cur = exe.as_path();
    while let Some(parent) = cur.parent() {
        if parent.file_name().map(|n| n == "sidecar-rs").unwrap_or(false) {
            return Ok(parent.to_path_buf());
        }
        cur = parent;
    }
    // Fallback: directory of the executable.
    Ok(exe
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf())
}

fn resolve_model_dir() -> Result<PathBuf> {
    Ok(sidecar_root()?.join("models").join("parakeet-tdt-v3"))
}

/// Load variables from a `parakeet.env` file at repo root if present.
/// Existing environment wins over file values so shell overrides still work.
fn load_env_file() {
    let Ok(root) = sidecar_root() else { return };
    let candidates = [
        root.parent().map(|p| p.join("parakeet.env")),
        Some(root.join("parakeet.env")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            match dotenvy::from_path(&candidate) {
                Ok(()) => eprintln!("[sidecar] loaded env from {}", candidate.display()),
                Err(e) => eprintln!("[sidecar] failed to load {}: {e}", candidate.display()),
            }
            return;
        }
    }
}

fn main() -> Result<()> {
    load_env_file();

    let emitter = std::sync::Arc::new(Emitter::new());
    let shutdown = std::sync::Arc::new(AtomicBool::new(false));

    // Resolve + fetch model
    let model_dir = match resolve_model_dir() {
        Ok(p) => p,
        Err(e) => {
            emitter.emit(Event::Error {
                msg: format!("model dir: {e}"),
            });
            return Ok(());
        }
    };
    if let Err(e) = model::ensure_model(&model_dir) {
        emitter.emit(Event::Error {
            msg: format!("model download failed: {e}"),
        });
        return Ok(());
    }

    // Audio → recognizer channel
    let (audio_tx, audio_rx) = bounded::<Vec<f32>>(32);

    // Start default capture; a later set_mic can replace it.
    let audio_stream = std::sync::Arc::new(Mutex::new(None::<AudioStream>));
    match audio::start_capture(None, audio_tx.clone()) {
        Ok(s) => {
            *audio_stream.lock().unwrap() = Some(s);
        }
        Err(e) => emitter.emit(Event::Error {
            msg: format!("open default stream: {e}"),
        }),
    }

    // Recognizer thread
    let emit_rec = emitter.clone();
    let model_dir_rec = model_dir.clone();
    let rec_handle = thread::spawn(move || {
        if let Err(e) = recognizer::run_recognizer(&model_dir_rec, audio_rx, |ev| {
            emit_rec.emit(ev)
        }) {
            emit_rec.emit(Event::Error {
                msg: format!("recognizer: {e}"),
            });
        }
    });

    emitter.emit(Event::Ready);

    // stdin command loop
    let stdin = BufReader::new(std::io::stdin());
    for line in stdin.lines() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Command>(line) {
            Ok(Command::Quit) => {
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
            Ok(Command::SetMic { name }) => {
                let name_ref = name.as_deref();
                match audio::start_capture(name_ref, audio_tx.clone()) {
                    Ok(s) => {
                        let mut g = audio_stream.lock().unwrap();
                        *g = Some(s); // drop old stream on replace
                    }
                    Err(e) => emitter.emit(Event::Error {
                        msg: format!("set_mic: {e}"),
                    }),
                }
            }
            Err(e) => eprintln!("[sidecar] bad command {line:?}: {e}"),
        }
    }

    // Drop audio stream, close channel, let recognizer thread finish.
    {
        let mut g = audio_stream.lock().unwrap();
        *g = None;
    }
    drop(audio_tx);
    let _ = rec_handle.join();
    Ok(())
}

// Silence warnings for unused imports on non-stream code paths.
#[allow(dead_code)]
fn _unused_sender<T>(_: &Sender<T>) {}
