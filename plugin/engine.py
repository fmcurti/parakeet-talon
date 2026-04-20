r"""
Parakeet STT engine for Talon.

Registers a custom AbstractEngine subclass that delegates recognition to an
external Parakeet sidecar process (onnx-asr). On each recognized phrase,
routes text through speech_system so Talon's grammar and action pipeline
runs unchanged.

Setup (one-time):
  cd ~/.talon/user/parakeet     (mac)  or  %APPDATA%\talon\user\parakeet  (win)
  python3 -m venv .venv
  .venv/bin/pip install -r requirements.txt
  # model is auto-downloaded on first recognition: nemo-parakeet-tdt-0.6b-v2

After setup, restart Talon and pick "parakeet" from the tray Active Engine menu.
"""

import json
import logging
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

from talon import speech_system
from talon.engines import EngineStatus
from talon.engines.dummy import DummyEngine

log = logging.getLogger("parakeet")

PLUGIN_DIR = Path(__file__).resolve().parent
SIDECAR_SCRIPT = PLUGIN_DIR / "sidecar.py"

if sys.platform == "win32":
    VENV_PY = PLUGIN_DIR / ".venv" / "Scripts" / "python.exe"
else:
    VENV_PY = PLUGIN_DIR / ".venv" / "bin" / "python"

_KEEP_RE = re.compile(r"[^a-z0-9\s-]+")
_WS_RE = re.compile(r"\s+")


def _clean_phrase(text: str) -> str:
    text = text.lower()
    text = _KEEP_RE.sub(" ", text)
    text = _WS_RE.sub(" ", text).strip()
    return text


class ParakeetEngine(DummyEngine):
    def __init__(self):
        super().__init__(name="parakeet", language="en_US")
        self._proc = None
        self._reader = None
        self._err_reader = None
        self._mic_name = None
        self._started_at = 0.0
        self._sidecar_ready = False
        self._lock = threading.Lock()

    def __repr__(self):
        return f"ParakeetEngine({self.name})"

    def enable(self):
        log.info("parakeet: enable")
        with self._lock:
            if self._proc and self._proc.poll() is None:
                return
            self._spawn()

    def disable(self):
        log.info("parakeet: disable")
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
        log.info(f"parakeet: set_mic={name!r}")
        self._send({"cmd": "set_mic", "name": name})

    def mimic(self, phrase):
        if isinstance(phrase, (list, tuple)):
            text = " ".join(phrase)
        else:
            text = phrase
        try:
            speech_system.mimic(text)
        except Exception:
            log.exception("parakeet: mimic dispatch failed")

    def _spawn(self):
        if not VENV_PY.exists():
            log.error(
                f"parakeet: sidecar venv python missing at {VENV_PY}; "
                f"run `python3 -m venv .venv && .venv/bin/pip install -r requirements.txt` in {PLUGIN_DIR}"
            )
            return
        if not SIDECAR_SCRIPT.exists():
            log.error(f"parakeet: sidecar script missing at {SIDECAR_SCRIPT}")
            return
        try:
            self._proc = subprocess.Popen(
                [str(VENV_PY), "-u", str(SIDECAR_SCRIPT)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                cwd=str(PLUGIN_DIR),
            )
        except Exception:
            log.exception("parakeet: failed to spawn sidecar")
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
        log.info(f"parakeet: sidecar spawned pid={self._proc.pid}")

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
            log.exception("parakeet: send failed")

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
                log.debug(f"parakeet: non-json stdout: {line!r}")
                continue
            ev = msg.get("event")
            if ev == "phrase":
                raw = (msg.get("text") or "").strip()
                text = _clean_phrase(raw)
                if not text:
                    continue
                cap, _flag = speech_system.parse(text)
                if cap is None:
                    log.info(f"parakeet: no-match {raw!r} -> {text!r}")
                    continue
                log.info(f"parakeet: matched {raw!r} -> {text!r}")
                try:
                    speech_system.mimic(text)
                except Exception:
                    log.exception("parakeet: dispatch failed")
            elif ev == "ready":
                self._sidecar_ready = True
                log.info("parakeet: sidecar ready")
                try:
                    speech_system.dispatch("update_engines")
                except Exception:
                    log.exception("parakeet: update_engines dispatch failed")
                try:
                    speech_system.pick_engine()
                except Exception:
                    log.exception("parakeet: pick_engine failed")
            elif ev == "error":
                log.error(f"parakeet: sidecar error: {msg.get('msg')}")
            else:
                log.debug(f"parakeet: event {msg!r}")
        log.info("parakeet: stdout closed")
        self._sidecar_ready = False

    def _err_loop(self):
        proc = self._proc
        if not proc or not proc.stderr:
            return
        for line in proc.stderr:
            line = line.rstrip()
            if line:
                log.info(f"parakeet[stderr] {line}")


_REGISTERED = False


def _register_once():
    global _REGISTERED
    if _REGISTERED:
        return
    for old in list(speech_system.engines.keys()):
        if type(old).__name__ == "ParakeetEngine":
            try:
                old.close()
            except Exception:
                log.exception("parakeet: failed to close prior engine")
            try:
                speech_system.remove_engine(old)
            except Exception:
                pass
    engine = ParakeetEngine()
    speech_system.add_engine(engine)
    _REGISTERED = True
    log.info("parakeet: registered")
    # Nudge the tray menu so the new engine shows up in the Active Engine dropdown.
    # On some builds `add_engine` doesn't auto-fire `update_engines`, and the menu
    # stays stale until Talon restarts.
    try:
        speech_system.dispatch("update_engines")
    except Exception:
        log.exception("parakeet: update_engines dispatch failed")
    proxy = speech_system.engines.get(engine)
    if proxy is not None:
        try:
            proxy.enable()
        except Exception:
            log.exception("parakeet: auto-enable failed")


_register_once()
