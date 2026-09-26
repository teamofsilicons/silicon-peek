"""peek IPC protocol v1 in Python, and a scriptable fake Peek.app (BLUEPRINT §1.6).

A frame is one JSON object and a newline; `"bin":[n1,n2,…]` announces that
n1+n2+… raw bytes follow the newline. Requests carry `v`, `id` and `op`;
replies carry the same `id` with `ok` and `result`/`error`; events carry
`event` and no `id`.

`FakeUi` connects to peekd with `role: "ui"` (peekd only accepts the
executable named by PEEK_UI_EXECUTABLE, see `process_executable()`), records
every event, answers peekd's requests (drawing.validate, drawing.load,
doctor, app.uninstall, app.update.prepare, app.quit) and sends UI requests
(answer, voice.submit, speech.done, shown.done, dismissed, message).
"""

from __future__ import annotations

import ctypes
import io
import json
import os
import queue
import socket
import struct
import threading
import time
import uuid
import wave
from typing import Any, Callable

MAX_LINE = 1024 * 1024


def process_executable(pid: int | None = None) -> str:
    """The executable path macOS reports for a process (proc_pidpath), which is
    what peekd compares with PEEK_UI_EXECUTABLE."""
    libc = ctypes.CDLL(None, use_errno=True)
    buf = ctypes.create_string_buffer(4096)
    n = libc.proc_pidpath(ctypes.c_int(pid or os.getpid()), buf, ctypes.c_uint32(len(buf)))
    if n <= 0:
        raise OSError(ctypes.get_errno(), "proc_pidpath failed")
    return os.path.realpath(buf.value.decode())


class Frame:
    def __init__(self, header: dict, blobs: list[bytes]) -> None:
        self.header = header
        self.blobs = blobs

    def __repr__(self) -> str:
        return f"Frame({json.dumps(self.header)[:200]}, blobs={[len(b) for b in self.blobs]})"


def encode(header: dict, blobs: list[bytes] | None = None) -> bytes:
    header = dict(header)
    blobs = blobs or []
    if blobs:
        header["bin"] = [len(b) for b in blobs]
    line = json.dumps(header, separators=(",", ":"), ensure_ascii=False).encode()
    if len(line) > MAX_LINE:
        raise ValueError("frame header too large")
    return line + b"\n" + b"".join(blobs)


class FrameReader:
    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.buf = bytearray()

    def _fill(self) -> bool:
        chunk = self.sock.recv(65536)
        if not chunk:
            return False
        self.buf.extend(chunk)
        return True

    def read(self) -> Frame | None:
        while b"\n" not in self.buf:
            if len(self.buf) > MAX_LINE or not self._fill():
                return None
        i = self.buf.index(b"\n")
        header = json.loads(self.buf[:i].decode())
        del self.buf[: i + 1]
        sizes = header.pop("bin", []) or []
        need = sum(sizes)
        while len(self.buf) < need:
            if not self._fill():
                return None
        blobs, off = [], 0
        for n in sizes:
            blobs.append(bytes(self.buf[off : off + n]))
            off += n
        del self.buf[:need]
        return Frame(header, blobs)


class IpcError(Exception):
    def __init__(self, error: dict) -> None:
        super().__init__(f"{error.get('code')}: {error.get('message')}")
        self.error = error


class Connection:
    """One socket to peekd with a reader thread: replies resolve pending calls,
    events go to `on_event`, requests to `on_request`."""

    def __init__(self, path: str, on_event: Callable[[Frame], None], on_request: Callable[[Frame], tuple[Any, list[bytes]]]) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.reader = FrameReader(self.sock)
        self.write_lock = threading.Lock()
        self.pending: dict[str, queue.Queue] = {}
        self.on_event = on_event
        self.on_request = on_request
        self.closed = threading.Event()
        self.thread: threading.Thread | None = None

    def start(self) -> None:
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def send(self, header: dict, blobs: list[bytes] | None = None) -> None:
        data = encode(header, blobs)
        with self.write_lock:
            self.sock.sendall(data)

    def call(self, op: str, fields: dict | None = None, blobs: list[bytes] | None = None, timeout: float = 30.0) -> tuple[Any, list[bytes]]:
        rid = str(uuid.uuid4())
        q: queue.Queue = queue.Queue(maxsize=1)
        self.pending[rid] = q
        self.send({"v": 1, "id": rid, "op": op, **(fields or {})}, blobs)
        try:
            frame = q.get(timeout=timeout)
        except queue.Empty:
            raise TimeoutError(f"no reply to {op} within {timeout}s") from None
        finally:
            self.pending.pop(rid, None)
        if frame is None:
            raise ConnectionError(f"peekd closed the connection during {op}")
        if not frame.header.get("ok"):
            raise IpcError(frame.header.get("error") or {})
        return frame.header.get("result"), frame.blobs

    def hello_sync(self, fields: dict) -> dict:
        """The handshake, before the reader thread starts."""
        rid = str(uuid.uuid4())
        self.send({"v": 1, "id": rid, "op": "hello", **fields})
        frame = self.reader.read()
        if frame is None:
            raise ConnectionError("peekd closed the connection during hello")
        if not frame.header.get("ok"):
            raise IpcError(frame.header.get("error") or {})
        return frame.header["result"]

    def _run(self) -> None:
        try:
            while True:
                frame = self.reader.read()
                if frame is None:
                    break
                h = frame.header
                if "event" in h:
                    self.on_event(frame)
                elif "op" in h:
                    threading.Thread(target=self._answer, args=(frame,), daemon=True).start()
                elif "id" in h:
                    q = self.pending.get(h["id"])
                    if q is not None:
                        q.put(frame)
        except OSError:
            pass
        finally:
            self.closed.set()
            for q in list(self.pending.values()):
                q.put(None)

    def _answer(self, frame: Frame) -> None:
        rid = frame.header["id"]
        try:
            result, blobs = self.on_request(frame)
            self.send({"v": 1, "id": rid, "ok": True, "result": result}, blobs)
        except IpcError as e:
            self.send({"v": 1, "id": rid, "ok": False, "error": e.error})
        except Exception as e:  # noqa: BLE001 - report any fake-UI bug to peekd
            self.send({"v": 1, "id": rid, "ok": False, "error": {"code": "internal_error", "message": f"fake UI: {e}", "retryable": False}})

    def close(self) -> None:
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.sock.close()


class FakeUi:
    """A scripted Peek.app. Every event is appended to `events` (header plus
    blob sizes); TTS chunks are also collected per send in `audio`."""

    def __init__(self, socket_path: str, app_build: int = 1000, app_version: str = "0.1.0") -> None:
        self.socket_path = socket_path
        self.app_build = app_build
        self.app_version = app_version
        self.lock = threading.Condition()
        self.events: list[dict] = []
        self.requests: list[dict] = []
        self.audio: dict[str, bytearray] = {}
        self.tts_first_chunk_at: dict[str, float] = {}
        self.tts_begin_at: dict[str, float] = {}
        self.validate_result: dict | None = None
        self.conn: Connection | None = None
        self.hello: dict | None = None

    # -------------------------------------------------------------- lifecycle
    def connect(self, timeout: float = 15.0) -> dict:
        deadline = time.monotonic() + timeout
        last: Exception | None = None
        while time.monotonic() < deadline:
            try:
                conn = Connection(self.socket_path, self._event, self._request)
                self.hello = conn.hello_sync({"role": "ui", "app_build": self.app_build, "app_version": self.app_version, "protocols": [1]})
                self.conn = conn
                conn.start()
                return self.hello
            except (OSError, ConnectionError) as e:
                last = e
                time.sleep(0.1)
        raise TimeoutError(f"could not connect to peekd at {self.socket_path}: {last}")

    def close(self) -> None:
        if self.conn:
            self.conn.close()

    # ----------------------------------------------------------------- events
    def _event(self, frame: Frame) -> None:
        h = dict(frame.header)
        now = time.monotonic()
        name = h.get("event")
        if name == "tts.begin":
            self.tts_begin_at[h["send_id"]] = now
            self.audio.setdefault(h["send_id"], bytearray())
        elif name == "tts.chunk":
            self.tts_first_chunk_at.setdefault(h["send_id"], now)
            self.audio.setdefault(h["send_id"], bytearray()).extend(b"".join(frame.blobs))
        h["_blobs"] = [len(b) for b in frame.blobs]
        h["_at"] = now
        with self.lock:
            self.events.append(h)
            self.lock.notify_all()

    def wait_event(self, predicate: Callable[[dict], bool], timeout: float = 30.0, what: str = "event") -> dict:
        deadline = time.monotonic() + timeout
        with self.lock:
            while True:
                for e in self.events:
                    if predicate(e):
                        return e
                left = deadline - time.monotonic()
                if left <= 0:
                    names = [e.get("event") for e in self.events[-20:]]
                    raise TimeoutError(f"timed out waiting for {what}; recent events: {names}")
                self.lock.wait(left)

    def events_named(self, name: str) -> list[dict]:
        with self.lock:
            return [e for e in self.events if e.get("event") == name]

    # --------------------------------------------------------------- requests
    def _request(self, frame: Frame) -> tuple[Any, list[bytes]]:
        h = frame.header
        op = h.get("op")
        with self.lock:
            self.requests.append({k: v for k, v in h.items() if k != "id"})
            self.lock.notify_all()
        if op == "drawing.validate":
            script = open(h["script_path"], "rb").read()
            result = self.validate_result or {
                "ok": True,
                "stats": {"frames": 90, "p50_ms": 0.4, "p95_ms": 0.9, "max_ms": 1.3, "ops_max": 42, "glass_rebuilds": 1},
                "warnings": [],
                "logs": [f"fake validator read {os.path.basename(h['script_path'])} ({len(script)} bytes)"],
                "error": None,
            }
            blobs = [PNG_1PX] if h.get("preview") else []
            return result, blobs
        if op == "drawing.load":
            return {"ok": True}, []
        if op == "doctor":
            return {"mic": "granted", "hotkeys": {"registered": [f"ctrl+cmd+{i}" for i in range(1, 9)], "failed": []}}, []
        if op == "app.uninstall":
            return {"accepted": True}, []
        if op in ("app.update.prepare", "app.quit"):
            return {"ready": False}, []
        raise IpcError({"code": "unknown_op", "message": f"the fake UI does not know {op}", "retryable": False})

    def call(self, op: str, fields: dict, blobs: list[bytes] | None = None, timeout: float = 30.0) -> Any:
        assert self.conn is not None, "connect() first"
        result, _ = self.conn.call(op, fields, blobs, timeout)
        return result


# A 1×1 transparent PNG for preview replies.
PNG_1PX = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010806000000"
    "1f15c4890000000d49444154789c6360000002000100e221bc330000000049454e44ae426082"
)


def resample_s16le(pcm: bytes, src_rate: int, dst_rate: int) -> bytes:
    """Linear-interpolation resampling of mono s16le (good enough for STT)."""
    n = len(pcm) // 2
    if n == 0:
        return b""
    samples = struct.unpack(f"<{n}h", pcm[: n * 2])
    out_n = int(n * dst_rate / src_rate)
    step = src_rate / dst_rate
    out = []
    for i in range(out_n):
        x = i * step
        j = int(x)
        frac = x - j
        a = samples[j]
        b = samples[j + 1] if j + 1 < n else a
        out.append(int(round(a + (b - a) * frac)))
    return struct.pack(f"<{len(out)}h", *out)


def wav_bytes(pcm: bytes, rate: int) -> bytes:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm)
    return buf.getvalue()
