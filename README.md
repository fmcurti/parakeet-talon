# parakeet-talon

Custom [Talon](https://talonvoice.com) speech engines backed by local, native Rust
sidecars. Two transcription engines ship side by side; the default activates on
startup and you switch between them with voice commands:

- **parakeet** — [NVIDIA Parakeet-TDT v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
  via [`parakeet-rs`](https://github.com/altunenes/parakeet-rs) on the `ort` bindings to
  ONNX Runtime.
- **qwen** — [Qwen3-ASR 0.6B](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) via the pure-Rust
  [`qwen3-asr`](https://github.com/alan890104/qwen3-asr-rs) crate (candle). Uses Metal on
  macOS and CPU on Linux/Windows.

Both feed recognized phrases into Talon's grammar pipeline via `speech_system.mimic()`, and
both are cross-platform (macOS, Windows, Linux).

## Architecture

```
 Talon process                              Rust sidecars (one binary per engine)
┌────────────────────────────┐             ┌────────────────────────────┐
│ plugin/engine.py           │             │ parakeet-sidecar           │
│   SidecarEngine per binary │◀── JSON ───▶│ qwen-sidecar               │
│   spawns sidecar subproc   │  over pipes │   cpal capture + resample  │
│   on phrase → mimic()      │             │   energy-based VAD         │
└────────────────────────────┘             │   <engine>::transcribe     │
                                            └────────────────────────────┘
```

- `plugin/engine.py` runs in Talon's embedded Python. It registers one `SidecarEngine`
  (subclass of `AbstractEngine` via `DummyEngine`) per sidecar binary that exists on disk.
  On startup it activates the engine named by the `user.stt_default_engine` setting
  (default `qwen`), spawning its sidecar. Switch at runtime with the **"use qwen"** /
  **"use parakeet"** voice commands (the `user.stt_select_engine` action), which enables
  the chosen sidecar and shuts the other down. (Talon's stock tray menu only lists the
  active engine and installable built-ins, so engine selection is done here, not there.)
- `sidecar-rs/` is a Cargo workspace. The shared `core` crate handles mic capture (`cpal`),
  resampling to 16 kHz mono (`rubato`), energy-based VAD to slice utterances, the Talon
  JSON protocol, and model download. The `parakeet` and `qwen` binary crates each implement
  the `Transcriber` trait and wire in their model. One VAD-segmented utterance in → one
  recognized phrase out.

```
sidecar-rs/
  core/       shared audio + VAD + protocol + model download
  parakeet/   parakeet-sidecar  (parakeet-rs / ONNX Runtime)
  qwen/       qwen-sidecar      (qwen3-asr / candle)
```

## Install

Install scripts default to downloading the **prebuilt sidecar binaries** from the latest
[GitHub Release](https://github.com/fmcurti/parakeet-talon/releases), falling back to a
local `cargo build --release` (which builds both binaries) if a download fails or you pass
`--build`.

### macOS / Linux

```bash
git clone https://github.com/fmcurti/parakeet-talon ~/personal/parakeet-talon
~/personal/parakeet-talon/scripts/install.sh
```

### Windows

```powershell
git clone https://github.com/fmcurti/parakeet-talon $env:USERPROFILE\personal\parakeet-talon
powershell -ExecutionPolicy Bypass -File $env:USERPROFILE\personal\parakeet-talon\scripts\install.ps1
```

The script:

1. Symlinks (Unix) or junction-links (Windows) `plugin/` into `~/.talon/user/parakeet`
   (or `%APPDATA%\talon\user\parakeet`). Any existing `parakeet/` is backed up as
   `parakeet.bak.<timestamp>`.
2. Downloads `parakeet-sidecar-<os>-<arch>.{tar.gz,zip}` and `qwen-sidecar-<os>-<arch>.…`
   from the latest release, verifies the SHA256, and drops the binaries under
   `sidecar-rs/target/release/`.
3. Falls back to `cargo build --release` if a prebuilt is missing, or if you pass
   `--build` / `-Build` / set `FORCE_BUILD=1`.

Restart Talon after install. **qwen** activates by default; say **"use parakeet"** to
switch engines and **"use qwen"** to switch back, or set `user.stt_default_engine` to
change the startup default. On first use, each engine downloads its weights from Hugging
Face into `sidecar-rs/models/`:

- parakeet → `parakeet-tdt-v3/` (~2.5 GB)
- qwen → `qwen3-asr-0.6b/` (~1.7 GB)

### Unsigned-binary notes

- **macOS**: the installer runs `xattr -d com.apple.quarantine` on the downloaded binaries
  so Gatekeeper lets them execute. If Talon launches a sidecar manually in a way that
  re-applies the flag, you can rerun that command by hand.
- **Windows**: SmartScreen may warn on first download of an unsigned exe. "More info" →
  "Run anyway" clears it once.

## Configuration

VAD knobs live in `parakeet.env` at the repo root and apply to **both** engines. Everything
is commented out by default; uncomment a line to override. Resolved values print to stderr /
`talon.log` on startup so you can verify the file was read.

| Variable | Default | What it does |
|---|---|---|
| `PARAKEET_VAD_THRESHOLD` | 0.005 | Mic energy floor |
| `PARAKEET_VAD_START_MS` | 160 | Speech needed before a phrase opens |
| `PARAKEET_VAD_END_MS` | 800 | Trailing silence before we decode — main latency knob |
| `PARAKEET_VAD_MIN_MS` | 320 | Discard utterances shorter than this |
| `PARAKEET_VAD_MAX_MS` | 10000 | Force-commit long utterances |

The qwen engine has one extra knob:

| Variable | Default | What it does |
|---|---|---|
| `QWEN_LANGUAGE` | english | Language forced on the Qwen decoder. A lowercase language name (e.g. `spanish`) forces it; `auto` enables per-utterance detection |

Shell/system env vars take precedence over the file so one-off overrides work without
editing it.

## Cutting a release

Releases trigger on tag push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The GH Actions workflow at `.github/workflows/release.yml` builds the whole workspace on
macOS (aarch64 + x86_64), Linux x86_64, and Windows x86_64, then uploads a `.tar.gz`/`.zip`
archive plus SHA256 for **each** sidecar binary (`parakeet-sidecar-*` and `qwen-sidecar-*`).
Non-tag pushes can also trigger the workflow via `workflow_dispatch` — outputs end up as
Actions artifacts rather than release assets.

## Troubleshooting

- **"No speech engine loaded" / nothing transcribes**: check `~/.talon/talon.log` for
  `parakeet: registered engines [...]` and `parakeet: selected engine '…'`. If an engine is
  missing, its binary isn't under `sidecar-rs/target/release/` — re-run the install script or
  `cargo build --release`. To force one, say **"use qwen"** / **"use parakeet"**, or set the
  `user.stt_default_engine` setting.
- **Phrases recognized but no action fires**: install the canonical command set with
  `git clone https://github.com/talonhub/community ~/.talon/user/community`.
- **Double recognition after hot-reloading engine.py**: `_register_once()` closes any prior
  `SidecarEngine` on load. If you still see stray sidecar processes,
  `pkill -f '\-sidecar'` and touch `plugin/engine.py` for a clean respawn.
- **"decoded X ms audio in Y ms" not appearing**: VAD didn't fire. Lower
  `PARAKEET_VAD_THRESHOLD` (e.g. `0.002`) for a quieter mic, or check that `cpal` opened
  the right input device in the stderr log.

## License

MIT.
