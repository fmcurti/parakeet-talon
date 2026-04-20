# parakeet-talon

Custom [Talon](https://talonvoice.com) speech engine backed by [NVIDIA Parakeet-TDT](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) via [onnx-asr](https://github.com/istupakov/onnx-asr). Runs Parakeet in an external sidecar process and feeds recognized phrases into Talon's grammar pipeline using `speech_system.mimic()`. Works on macOS and Windows; Linux should work too.

## Architecture

```
 Talon process                              Sidecar venv
┌────────────────────────────┐             ┌───────────────────────────┐
│ plugin/engine.py           │             │ plugin/sidecar.py         │
│   subclass DummyEngine     │◀── JSON ───▶│   sounddevice capture     │
│   spawns sidecar subproc   │  over pipes │   onnx-asr streaming loop │
│   on phrase → mimic()      │             │   energy-based VAD        │
└────────────────────────────┘             └───────────────────────────┘
```

- `plugin/engine.py` runs in Talon's embedded Python, registers a `ParakeetEngine` (subclass of `AbstractEngine` via `DummyEngine`). Auto-enables and auto-picks itself as the active engine on startup.
- `plugin/sidecar.py` runs in its own venv with `onnx-asr` + `sounddevice`. Captures mic audio, chunks on simple energy VAD, runs Parakeet, emits recognized phrases as JSON to the parent.

## Install

### macOS / Linux

```bash
git clone <this repo> ~/personal/parakeet-talon
~/personal/parakeet-talon/scripts/install.sh
```

### Windows

```powershell
git clone <this repo> $env:USERPROFILE\personal\parakeet-talon
powershell -ExecutionPolicy Bypass -File $env:USERPROFILE\personal\parakeet-talon\scripts\install.ps1
```

Either script:

1. Symlinks (macOS/Linux) or junction-links (Windows) the repo's `plugin/` into Talon's user directory as `parakeet/`.
2. Creates a `.venv` inside `plugin/` and installs `requirements.txt` into it.
3. Preserves any existing `parakeet/` folder as `parakeet.bak.<timestamp>`.

Restart Talon after install. The model (~2 GB for v3) downloads from Hugging Face on first run.

## Configuration

- **Model**: set env var `PARAKEET_MODEL` to `nemo-parakeet-tdt-0.6b-v2` (English-only, faster) or `nemo-parakeet-tdt-0.6b-v3` (25 European languages, default).
- **ONNX providers**: set `PARAKEET_PROVIDERS=CUDAExecutionProvider` on NVIDIA GPUs or `DmlExecutionProvider` for DirectML on Windows. Default is `CPUExecutionProvider`.
- **VAD thresholds**: tune constants at the top of `plugin/sidecar.py` (`VAD_ENERGY_THRESHOLD`, `VAD_END_FRAMES`, etc.).

## Troubleshooting

- **No `parakeet` entry in the Active Engine menu**: check `~/.talon/talon.log` for `parakeet: registered` / import errors.
- **`model load failed`**: CoreML EP is finicky; stick with `CPUExecutionProvider` on macOS.
- **Double recognition after editing code**: the engine kills orphan sidecars on reload; if you see two sidecar processes, `pkill -f sidecar.py` (or `Stop-Process`) and `touch plugin/engine.py` to respawn cleanly.
- **Phrases recognized but nothing fires**: install the canonical command set with `git clone https://github.com/talonhub/community ~/.talon/user/community`.

## License

MIT.
