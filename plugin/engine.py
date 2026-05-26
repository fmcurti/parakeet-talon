r"""
Local STT engines for Talon.

Registers one custom AbstractEngine per available sidecar binary (parakeet,
qwen). Each engine delegates recognition to its external Rust sidecar and, on
each recognized phrase, routes text through speech_system so Talon's grammar and
action pipeline runs unchanged.

On startup the engine named by the `user.stt_default_engine` setting (default
"qwen") is activated. Switch at runtime with the "use qwen" / "use parakeet"
voice commands, or the `user.stt_select_engine` action.

Install via scripts/install.sh (macOS/Linux) or scripts/install.ps1 (Windows).
"""

import json
import logging
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

from talon import Module, settings, speech_system
from talon.engines.dummy import DummyEngine

log = logging.getLogger("parakeet")

mod = Module()
mod.setting(
    "stt_default_engine",
    type=str,
    default="qwen",
    desc="Local STT engine to activate on startup: 'parakeet' or 'qwen'.",
)

# Plugin lives at <repo>/plugin/engine.py (when installed via script, this path
# is reached through a symlink at <talon_user>/parakeet). resolve() follows the
# symlink so the sidecar paths point inside the repo.
PLUGIN_DIR = Path(__file__).resolve().parent
REPO_DIR = PLUGIN_DIR.parent
RELEASE_DIR = REPO_DIR / "sidecar-rs" / "target" / "release"
_EXE = ".exe" if sys.platform == "win32" else ""

# (engine name, sidecar binary path). An engine registers only if its binary
# exists, so platforms without a given binary simply don't show that engine.
ENGINES = [
    ("parakeet", RELEASE_DIR / f"parakeet-sidecar{_EXE}"),
    ("qwen", RELEASE_DIR / f"qwen-sidecar{_EXE}"),
]

_KEEP_RE = re.compile(r"[^a-z0-9\s-]+")
_WS_RE = re.compile(r"\s+")


def _clean_phrase(text: str) -> str:
    text = text.lower()
    text = _KEEP_RE.sub(" ", text)
    text = _WS_RE.sub(" ", text).strip()
    return text


class SidecarEngine(DummyEngine):
    def __init__(self, name: str, sidecar_bin: Path):
        super().__init__(name=name, language="en_US")
        self._bin = sidecar_bin
        self._log = logging.getLogger(name)
        self._proc = None
        self._reader = None
        self._err_reader = None
        self._mic_name = None
        self._started_at = 0.0
        self._sidecar_ready = False
        self._lock = threading.Lock()

    def __repr__(self):
        return f"SidecarEngine({self.name})"

    def enable(self):
        self._log.info(f"{self.name}: enable")
        with self._lock:
            if self._proc and self._proc.poll() is None:
                return
            self._spawn()

    def disable(self):
        self._log.info(f"{self.name}: disable")
        with self._lock:
            self._shutdown()

    def close(self):
        with self._lock:
            self._shutdown()

    def status(self):
        s = super().status()
        alive = self._proc is not None and self._proc.poll() is None
        try:
            s.ready = bool(alive and self._sidecar_ready)
        except Exception:
            pass
        return s

    def set_microphone(self, device):
        try:
            super().set_microphone(device)
        except Exception:
            pass
        name = getattr(device, "name", None) if device else None
        self._mic_name = name
        self._log.info(f"{self.name}: set_mic={name!r}")
        self._send({"cmd": "set_mic", "name": name})

    def mimic(self, phrase):
        if isinstance(phrase, (list, tuple)):
            text = " ".join(phrase)
        else:
            text = phrase
        try:
            speech_system.mimic(text)
        except Exception:
            self._log.exception(f"{self.name}: mimic dispatch failed")

    def _spawn(self):
        if not self._bin.exists():
            self._log.error(
                f"{self.name}: sidecar binary missing at {self._bin}; "
                f"run `cargo build --release` in {REPO_DIR / 'sidecar-rs'} or re-run the install script"
            )
            return
        try:
            self._proc = subprocess.Popen(
                [str(self._bin)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                cwd=str(PLUGIN_DIR),
            )
        except Exception:
            self._log.exception(f"{self.name}: failed to spawn sidecar")
            self._proc = None
            return
        self._started_at = time.time()
        self._sidecar_ready = False
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()
        self._err_reader = threading.Thread(target=self._err_loop, daemon=True)
        self._err_reader.start()
        if self._mic_name:
            self._send({"cmd": "set_mic", "name": self._mic_name})
        self._log.info(f"{self.name}: sidecar spawned pid={self._proc.pid}")

    def _shutdown(self):
        proc = self._proc
        if not proc:
            return
        try:
            self._send({"cmd": "quit"})
        except Exception:
            pass
        try:
            proc.terminate()
            proc.wait(timeout=2.0)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
        self._proc = None
        self._sidecar_ready = False

    def _send(self, msg):
        proc = self._proc
        if not proc or proc.poll() is not None or not proc.stdin:
            return
        try:
            proc.stdin.write(json.dumps(msg) + "\n")
            proc.stdin.flush()
        except Exception:
            self._log.exception(f"{self.name}: send failed")

    def _read_loop(self):
        proc = self._proc
        if not proc or not proc.stdout:
            return
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except Exception:
                self._log.debug(f"{self.name}: non-json stdout: {line!r}")
                continue
            ev = msg.get("event")
            if ev == "phrase":
                raw = (msg.get("text") or "").strip()
                text = _clean_phrase(raw)
                if not text:
                    continue
                cap, _flag = speech_system.parse(text)
                if cap is None:
                    self._log.info(f"{self.name}: no-match {raw!r} -> {text!r}")
                    continue
                self._log.info(f"{self.name}: matched {raw!r} -> {text!r}")
                try:
                    speech_system.mimic(text)
                except Exception:
                    self._log.exception(f"{self.name}: dispatch failed")
            elif ev == "ready":
                self._sidecar_ready = True
                self._log.info(f"{self.name}: sidecar ready")
                # Refresh the tray menu and make this (the enabled) engine the
                # active one now that its sidecar can transcribe.
                try:
                    speech_system.dispatch("update_engines")
                except Exception:
                    self._log.exception(f"{self.name}: update_engines dispatch failed")
                try:
                    speech_system.pick_engine()
                except Exception:
                    self._log.exception(f"{self.name}: pick_engine failed")
            elif ev == "error":
                self._log.error(f"{self.name}: sidecar error: {msg.get('msg')}")
            else:
                self._log.debug(f"{self.name}: event {msg!r}")
        self._log.info(f"{self.name}: stdout closed")
        self._sidecar_ready = False

    def _err_loop(self):
        proc = self._proc
        if not proc or not proc.stderr:
            return
        for line in proc.stderr:
            line = line.rstrip()
            if line:
                self._log.info(f"{self.name}[stderr] {line}")


def _our_engines():
    return [e for e in speech_system.engines.keys() if type(e).__name__ == "SidecarEngine"]


def _default_engine() -> str:
    try:
        val = settings.get("user.stt_default_engine")
        if val:
            return str(val)
    except Exception:
        pass
    return "qwen"


def _select(name: str):
    """Activate the named engine (spawning its sidecar) and disable the others."""
    engines = _our_engines()
    if not engines:
        log.error("parakeet: no engines registered to select")
        return
    target = next((e for e in engines if e.name == name), None)
    if target is None:
        target = engines[0]
        log.warning(f"parakeet: engine {name!r} not available; using {target.name!r}")
    for e in engines:
        proxy = speech_system.engines.get(e)
        if proxy is None:
            continue
        try:
            if e is target:
                proxy.enable()
            else:
                proxy.disable()
        except Exception:
            log.exception(f"parakeet: enable/disable {e.name} failed")
    try:
        speech_system.dispatch("update_engines")
    except Exception:
        log.exception("parakeet: update_engines dispatch failed")
    try:
        speech_system.pick_engine()
    except Exception:
        log.exception("parakeet: pick_engine failed")
    log.info(f"parakeet: selected engine {target.name!r}")


@mod.action_class
class Actions:
    def stt_select_engine(name: str):
        """Activate a local STT engine by name ('parakeet' or 'qwen')."""
        _select(name)


_REGISTERED = False


def _register_once():
    global _REGISTERED
    if _REGISTERED:
        return
    # Drop any engines we registered on a previous import/reload.
    for old in list(speech_system.engines.keys()):
        if type(old).__name__ == "SidecarEngine":
            try:
                old.close()
            except Exception:
                log.exception("parakeet: failed to close prior engine")
            try:
                speech_system.remove_engine(old)
            except Exception:
                pass

    registered = []
    for name, sidecar_bin in ENGINES:
        if not sidecar_bin.exists():
            log.info(f"{name}: sidecar binary not found at {sidecar_bin}; skipping registration")
            continue
        speech_system.add_engine(SidecarEngine(name, sidecar_bin))
        registered.append(name)

    _REGISTERED = True
    if not registered:
        log.error(
            "parakeet: no sidecar binaries found; build them in "
            f"{REPO_DIR / 'sidecar-rs'} or re-run the install script"
        )
        return
    log.info(f"parakeet: registered engines {registered}")
    # Activate the default engine on startup. Enabling an engine spawns its
    # sidecar and makes it the loaded/active engine; the others stay disabled.
    # Switch at runtime with the "use qwen" / "use parakeet" voice commands
    # (or the user.stt_select_engine action).
    _select(_default_engine())


_register_once()
