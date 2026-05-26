use anyhow::{Context, Result};
use crossbeam_channel::bounded;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;

use crate::audio::{self, AudioStream};
use crate::protocol::{Command, Event};
use crate::recognizer::{self, Transcriber};

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
pub fn sidecar_root() -> Result<PathBuf> {
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
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf())
}

/// Resolve a model directory (`sidecar-rs/models/<name>`) for an engine.
pub fn model_dir(name: &str) -> Result<PathBuf> {
    Ok(sidecar_root()?.join("models").join(name))
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

/// Run the sidecar: start audio capture, build the transcriber on a worker
/// thread, and process JSON commands from stdin.
///
/// `init` runs inside the recognizer thread and is responsible for acquiring
/// the model (download if needed) and loading it. It runs there — rather than
/// before the initial `ready` event — so a slow first-run download or model
/// load does not block startup, and so backends whose handles are not `Send`
/// (e.g. a GPU device) never cross threads.
pub fn run(
    init: impl FnOnce() -> Result<Box<dyn Transcriber>> + Send + 'static,
) -> Result<()> {
    load_env_file();

    let emitter = Arc::new(Emitter::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    // Audio → recognizer channel
    let (audio_tx, audio_rx) = bounded::<Vec<f32>>(32);

    // Start default capture; a later set_mic can replace it.
    let audio_stream = Arc::new(Mutex::new(None::<AudioStream>));
    match audio::start_capture(None, audio_tx.clone()) {
        Ok(s) => {
            *audio_stream.lock().unwrap() = Some(s);
        }
        Err(e) => emitter.emit(Event::Error {
            msg: format!("open default stream: {e}"),
        }),
    }

    // Recognizer thread: build the transcriber, then run the VAD loop.
    let emit_rec = emitter.clone();
    let rec_handle = thread::spawn(move || {
        let transcriber = match init() {
            Ok(t) => t,
            Err(e) => {
                emit_rec.emit(Event::Error {
                    msg: format!("model init: {e}"),
                });
                return;
            }
        };
        if let Err(e) = recognizer::run_recognizer(transcriber, audio_rx, |ev| emit_rec.emit(ev)) {
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
