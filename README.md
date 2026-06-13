# parakeet-talon

Custom [Talon](https://talonvoice.com) speech engines backed by local, native Rust
sidecars. Two transcription engines ship side by side; the default activates on
startup and you switch between them with voice commands:

- **parakeet** — [NVIDIA Parakeet-TDT v2](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2)
  (English-only) via [`parakeet-rs`](https://github.com/altunenes/parakeet-rs) on the `ort`
  bindings to ONNX Runtime.
- **qwen** — [Qwen3-ASR 0.6B](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) (int8) via
  [`sherpa-onnx`](https://github.com/k2-fsa/sherpa-onnx) on ONNX Runtime. Runs fast on CPU
  across all platforms; language is forced to English by default (`QWEN_LANGUAGE`).

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
  (default `qwen`), spawning its sidecar. Switch at runtime with the short voice codes in
  `engine.talon` — **"use par"** / **"use para"** for parakeet, **"use Q"** for qwen —
  via the `user.stt_select_engine` action, which enables the chosen sidecar and shuts the
  other down. (Talon's stock tray menu only lists the active engine and installable
  built-ins, so engine selection is done here, not there.)
- `sidecar-rs/` is a Cargo workspace. The shared `core` crate handles mic capture (`cpal`),
  resampling to 16 kHz mono (`rubato`), energy-based VAD to slice utterances, the Talon
  JSON protocol, and model download. The `parakeet` and `qwen` binary crates each implement
  the `Transcriber` trait and wire in their model. One VAD-segmented utterance in → one
  recognized phrase out.

```
sidecar-rs/
  core/       shared audio + VAD + protocol + model download
  parakeet/   parakeet-sidecar  (parakeet-rs / ONNX Runtime)
  qwen/       qwen-sidecar      (sherpa-onnx / ONNX Runtime, int8)
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

Restart Talon after install. **qwen** activates by default; say **"use par"** (or
**"use para"**) to switch to Parakeet and **"use Q"** to switch back, or set
`user.stt_default_engine` to change the startup default. On first use, each engine
downloads its model into `sidecar-rs/models/`:

- parakeet → `parakeet-tdt-v2/` (~2.5 GB, from Hugging Face)
- qwen → `sherpa-onnx-qwen3-asr-0.6B-int8-…/` (~850 MB download, ~1 GB on disk, from the
  sherpa-onnx releases)

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
| `PARAKEET_VAD_THRESHOLD` | 0.005 | Mic energy floor (checked per 32 ms window, so a word starting mid-chunk still triggers) |
| `PARAKEET_VAD_START_MS` | 160 | Speech needed before a phrase opens |
| `PARAKEET_VAD_END_MS` | 800 | Trailing silence before we decode — main latency knob |
| `PARAKEET_VAD_MIN_MS` | 320 | Discard phrases with less speech than this (trailing silence doesn't count) |
| `PARAKEET_VAD_MAX_MS` | 10000 | Force-commit long utterances |
| `PARAKEET_VAD_PREROLL_MS` | 320 | Audio kept from before the trigger and prepended to the phrase, so quiet word onsets aren't clipped |

The qwen engine has one extra knob:

| Variable | Default | What it does |
|---|---|---|
| `QWEN_LANGUAGE` | english | Language the Qwen decoder is forced to (Talon commands are English). A language name like `spanish` forces that instead; `auto` re-enables per-utterance detection |

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
  `cargo build --release`. To force one, say **"use Q"** (qwen) / **"use par"** (parakeet),
  or set the `user.stt_default_engine` setting.
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
