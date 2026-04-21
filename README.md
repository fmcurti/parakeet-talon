# parakeet-talon

Custom [Talon](https://talonvoice.com) speech engine backed by
[NVIDIA Parakeet-TDT v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) running in a
native Rust sidecar ([`parakeet-rs`](https://github.com/altunenes/parakeet-rs) on top of
the `ort` bindings to ONNX Runtime). Feeds recognized phrases into Talon's grammar pipeline
via `speech_system.mimic()`. Cross-platform (macOS, Windows, Linux).

## Architecture

```
 Talon process                              Rust sidecar (single binary)
┌────────────────────────────┐             ┌────────────────────────────┐
│ plugin/engine.py           │             │ sidecar-rs/                │
│   subclass DummyEngine     │◀── JSON ───▶│   cpal capture + resample  │
│   spawns sidecar subproc   │  over pipes │   energy-based VAD         │
│   on phrase → mimic()      │             │   ParakeetTDT::transcribe  │
└────────────────────────────┘             └────────────────────────────┘
```

- `plugin/engine.py` runs in Talon's embedded Python. Registers `ParakeetEngine` (subclass
  of `AbstractEngine` via `DummyEngine`), auto-enables on registration, auto-picks itself
  when the sidecar reports ready, and re-dispatches `update_engines` so the tray menu
  refreshes.
- `sidecar-rs/` builds to a single static binary. Captures mic audio with `cpal`, resamples
  to 16 kHz mono via `rubato`, runs energy-based VAD to slice utterances, then calls
  `ParakeetTDT::transcribe_samples` per utterance. The ONNX Runtime library is
  statically linked — the binary is self-contained.

## Install

Install scripts default to downloading the **prebuilt sidecar binary** from the latest
[GitHub Release](https://github.com/fmcurti/parakeet-talon/releases), falling back to a
local `cargo build --release` if the download fails or you pass `--build`.

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
2. Downloads `parakeet-sidecar-<os>-<arch>.{tar.gz,zip}` from the latest release, verifies
   the SHA256, and drops the binary at `sidecar-rs/target/release/parakeet-sidecar[.exe]`.
3. Falls back to `cargo build --release` if no prebuilt matches this platform, or if you
   pass `--build` / `-Build` / set `FORCE_BUILD=1`.

Restart Talon after install. On first activation the sidecar downloads ~2.5 GB of Parakeet
v3 weights from Hugging Face into `sidecar-rs/models/parakeet-tdt-v3/`.

### Unsigned-binary notes

- **macOS**: the installer runs `xattr -d com.apple.quarantine` on the downloaded binary
  so Gatekeeper lets it execute. If Talon launches the sidecar manually in a way that
  re-applies the flag, you can rerun that command by hand.
- **Windows**: SmartScreen may warn on first download of an unsigned exe. "More info" →
  "Run anyway" clears it once.

## Configuration

VAD knobs live in `parakeet.env` at the repo root. Everything is commented out by default;
uncomment a line to override. Resolved values print to stderr / `talon.log` on startup so
you can verify the file was read.

| Variable | Default | What it does |
|---|---|---|
| `PARAKEET_VAD_THRESHOLD` | 0.005 | Mic energy floor |
| `PARAKEET_VAD_START_MS` | 160 | Speech needed before a phrase opens |
| `PARAKEET_VAD_END_MS` | 800 | Trailing silence before we decode — main latency knob |
| `PARAKEET_VAD_MIN_MS` | 320 | Discard utterances shorter than this |
| `PARAKEET_VAD_MAX_MS` | 10000 | Force-commit long utterances |

Shell/system env vars take precedence over the file so one-off overrides work without
editing it.

## Cutting a release

Releases trigger on tag push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The GH Actions workflow at `.github/workflows/release.yml` builds the sidecar on macOS
(aarch64 + x86_64), Linux x86_64, and Windows x86_64, then uploads `.tar.gz`/`.zip`
archives plus SHA256 checksums to the release. Non-tag pushes can also trigger the
workflow via `workflow_dispatch` — artifacts end up as Actions artifacts rather than
release assets.

## Troubleshooting

- **No `parakeet` entry in the Active Engine menu**: check `~/.talon/talon.log` for
  `parakeet: registered` / import errors. The engine also calls `update_engines` after
  registration to force the tray menu to refresh.
- **Phrases recognized but no action fires**: install the canonical command set with
  `git clone https://github.com/talonhub/community ~/.talon/user/community`.
- **Double recognition after hot-reloading engine.py**: `_register_once()` kills any prior
  `ParakeetEngine` on load, so it shouldn't happen — but if you see two sidecar processes,
  `pkill -f parakeet-sidecar` and touch `plugin/engine.py` for a clean respawn.
- **"decoded X ms audio in Y ms" not appearing**: VAD didn't fire. Lower
  `PARAKEET_VAD_THRESHOLD` (e.g. `0.002`) for a quieter mic, or check that `cpal` opened
  the right input device in the stderr log.

## License

MIT.
