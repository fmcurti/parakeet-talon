"""
Parakeet sidecar: captures audio, runs onnx-asr Parakeet, emits recognized
phrases to stdout as JSON lines. Controlled via JSON commands on stdin.

Must be run as __main__ from the sidecar venv; when Talon's loader imports
this file for scanning, nothing heavy executes.

Protocol (line-delimited JSON):
  in:  {"cmd": "set_mic", "name": "<device name or null>"}
  in:  {"cmd": "quit"}
  out: {"event": "ready"}
  out: {"event": "phrase", "text": "..."}
  out: {"event": "error",  "msg": "..."}
"""


def main():
    import json
    import queue
    import sys
    import threading
    import time

    import numpy as np
    import sounddevice as sd
    import onnx_asr

    SAMPLE_RATE = 16000
    FRAME_MS = 30
    FRAME_LEN = int(SAMPLE_RATE * FRAME_MS / 1000)
    VAD_START_FRAMES = 3
    VAD_END_FRAMES = 25
    VAD_ENERGY_THRESHOLD = 0.005
    MAX_PHRASE_SECONDS = 10.0
    MIN_PHRASE_SECONDS = 0.3

    emit_lock = threading.Lock()

    def emit(event, **kw):
        line = json.dumps({"event": event, **kw})
        with emit_lock:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    def log(msg):
        sys.stderr.write(f"[sidecar] {msg}\n")
        sys.stderr.flush()

    import os
    providers_env = os.environ.get("PARAKEET_PROVIDERS", "CPUExecutionProvider")
    providers = [p.strip() for p in providers_env.split(",") if p.strip()]
    model_name = os.environ.get("PARAKEET_MODEL", "nemo-parakeet-tdt-0.6b-v3")
    log(f"loading {model_name} providers={providers}")
    try:
        model = onnx_asr.load_model(model_name, providers=providers)
    except Exception as e:
        emit("error", msg=f"model load failed: {e}")
        sys.exit(1)
    log("model loaded")

    stream_lock = threading.Lock()
    stream_state = {"stream": None, "device": None}
    audio_q: "queue.Queue[np.ndarray]" = queue.Queue()

    def audio_callback(indata, frames, time_info, status):
        if status:
            log(f"audio status: {status}")
        audio_q.put(indata.copy().reshape(-1))

    def find_device_index(name):
        if not name:
            return None
        try:
            devs = sd.query_devices()
        except Exception as e:
            log(f"query_devices failed: {e}")
            return None
        for i, d in enumerate(devs):
            if d.get("max_input_channels", 0) > 0 and name in d.get("name", ""):
                return i
        return None

    def stop_stream_locked():
        s = stream_state["stream"]
        if s is not None:
            try:
                s.stop()
                s.close()
            except Exception as e:
                log(f"stop_stream error: {e}")
        stream_state["stream"] = None
        stream_state["device"] = None

    def open_stream(device_name):
        with stream_lock:
            stop_stream_locked()
            idx = find_device_index(device_name)
            log(f"opening stream name={device_name!r} idx={idx}")
            stream = sd.InputStream(
                samplerate=SAMPLE_RATE,
                channels=1,
                dtype="float32",
                device=idx,
                blocksize=FRAME_LEN,
                callback=audio_callback,
            )
            stream.start()
            stream_state["stream"] = stream
            stream_state["device"] = device_name

    def recognize_worker():
        buf = []
        speaking = False
        above = 0
        below = 0
        phrase_start = 0.0

        while True:
            try:
                frame = audio_q.get(timeout=0.5)
            except queue.Empty:
                continue

            rms = float(np.sqrt(np.mean(frame.astype(np.float32) ** 2)))
            loud = rms >= VAD_ENERGY_THRESHOLD

            if not speaking:
                if loud:
                    above += 1
                    if above >= VAD_START_FRAMES:
                        speaking = True
                        phrase_start = time.time()
                        buf = [frame]
                        above = 0
                        below = 0
                else:
                    above = 0
                continue

            buf.append(frame)
            if loud:
                below = 0
            else:
                below += 1
            duration = time.time() - phrase_start
            if below >= VAD_END_FRAMES or duration > MAX_PHRASE_SECONDS:
                speaking = False
                below = 0
                if duration >= MIN_PHRASE_SECONDS and buf:
                    audio = np.concatenate(buf).astype(np.float32)
                    try:
                        text = model.recognize(audio)
                        if isinstance(text, list):
                            text = text[0] if text else ""
                        text = (text or "").strip()
                        if text:
                            emit("phrase", text=text)
                    except Exception as e:
                        emit("error", msg=f"recognize failed: {e}")
                buf = []

    def stdin_loop():
        for raw in sys.stdin:
            line = raw.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except Exception:
                continue
            cmd = msg.get("cmd")
            if cmd == "quit":
                with stream_lock:
                    stop_stream_locked()
                sys.exit(0)
            elif cmd == "set_mic":
                try:
                    open_stream(msg.get("name"))
                except Exception as e:
                    emit("error", msg=f"set_mic failed: {e}")

    threading.Thread(target=recognize_worker, daemon=True).start()

    try:
        open_stream(None)
        emit("ready")
    except Exception as e:
        emit("error", msg=f"open default stream failed: {e}")

    try:
        stdin_loop()
    except KeyboardInterrupt:
        pass
    finally:
        with stream_lock:
            stop_stream_locked()


if __name__ == "__main__":
    main()
