#!/usr/bin/env python3
"""End-to-end test of peek on one Mac: the REAL `peek` CLI (temp SILICON_HOME),
the REAL peekd (isolated run) and the REAL peek-server (temp SQLite), with fake
IAM and Ting services and a fake Peek.app speaking IPC v1 over peekd's socket.
Speech uses real ElevenLabs TTS directly with a short-lived Deepgram JWT;
OpenAI STT uses peek-server's relay.

    python3 scripts/e2e/e2e.py [--no-build] [--keep] [--no-stt]

Export PEEK_DEEPGRAM_API_KEY for TTS and PEEK_OPENAI_API_KEY for STT.
Only peek-server receives these keys. Missing Deepgram skips TTS; --no-stt
or a missing OpenAI key skips the STT round trip of the generated audio.

Steps: login (fake SLT) → login status → register side 3 (ctrl+cmd+3) →
register drawing (the fake UI validates it; staged under its own filename) →
send --speak (tts.* frames reach the fake UI) → send --ask single_choice →
fake UI clicks → the exact §3.5 Ting body arrives at the fake Ting → voice
answer (the TTS audio, resampled to 16 kHz mono WAV, voice.submit → real STT
→ matched → ting) → Carbon voice message (the voice.submit reply names the
message; its stt.result carries that id → ting) → Carbon message → early
token expiry (one forced refresh) → send --ask --wait (answer on stdout, no
ting) → queue v2 (strict FIFO, queue_full with exit 4, peek queue, peek cancel,
peek queue clear, --replace) → --expires-in on a show while the screen is
locked (expires unseen: peek.send.expired shown:false) → scheduling (--in 5s
fires: peek.schedule.due, then peek.send.shown after the UI's `shown`;
schedule list/cancel/clear) → ui.status push + doctor → logout. The fake UI
behaves like Peek.app build 1002 (it reports `shown`). Exits non-zero on the
first failed check; latencies are printed at the end.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import local_stack  # noqa: E402
from peek_ipc import FakeUi, process_executable, resample_s16le, wav_bytes  # noqa: E402

def _workspace_version() -> str:
    """The workspace version (Cargo.toml `[workspace.package] version`)."""
    in_package = False
    for line in (Path(__file__).resolve().parents[2] / "Cargo.toml").read_text().splitlines():
        if line.strip().startswith("["):
            in_package = line.strip() == "[workspace.package]"
        elif in_package and line.strip().startswith("version"):
            return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit("Cargo.toml has no [workspace.package] version")


PEEK_VERSION = _workspace_version()
ACTOR = "si:e2e-silicon"
ORG = "tos"
SLOT = 3
ISI = "e2e-flow"
SPEAK_TEXT = "The second one."
OPTIONS = [
    {"id": "first", "label": "The first one"},
    {"id": "second", "label": "The second one"},
    {"id": "third", "label": "The third one"},
]
DRAWING = """// e2e drawing: a glass circle with a dot that follows speech.
peek.frame((ctx, input) => {
  ctx.fillGlass(null, { style: 'regular' });
  const r = 10 + 20 * ((input.speech && input.speech.level) || 0);
  ctx.beginPath();
  ctx.arc(50, 50, r, 0, Math.PI * 2);
  ctx.fillStyle = '#ffffff';
  ctx.fill();
  return false;
});
"""


class Failure(Exception):
    pass


def check(cond: bool, what: str) -> None:
    if not cond:
        raise Failure(what)
    print(f"  ok  {what}")


class Run:
    def __init__(self, stack: local_stack.Stack, ui: FakeUi) -> None:
        self.stack = stack
        self.ui = ui
        self.env = {**local_stack.base_env(), **stack.cli_env(), "ISI": ISI}
        self.metrics: dict[str, float] = {}

    # --------------------------------------------------------------- helpers
    def peek(self, *args: str, ok: bool = True, timeout: float = 60) -> dict:
        argv = [str(local_stack.BIN / "peek"), *args, "--json"]
        t0 = time.monotonic()
        p = subprocess.run(argv, env=self.env, cwd=self.stack.root, capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)
        took = time.monotonic() - t0
        label = " ".join(a if len(a) < 40 else a[:37] + "…" for a in args)
        if ok and p.returncode != 0:
            raise Failure(f"`peek {label}` exited {p.returncode}: {p.stderr.strip()}")
        out = p.stdout.strip()
        try:
            value = json.loads(out) if out else {}
        except json.JSONDecodeError as e:
            raise Failure(f"`peek {label}` printed non-JSON stdout: {out!r}") from e
        print(f"  ran peek {label} ({took * 1000:.0f} ms, exit {p.returncode})")
        value["_exit"] = p.returncode
        value["_stderr"] = p.stderr
        return value

    def get(self, url: str) -> dict:
        with urllib.request.urlopen(url, timeout=5) as r:  # noqa: S310 - loopback fakes
            return json.loads(r.read())

    def tings(self) -> list[dict]:
        return self.get(f"{self.stack.ting}/_fake/tings")["tings"]

    def wait_ting(self, key: str, timeout: float = 30) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for t in self.tings():
                if t["body"].get("key") == key:
                    return t
            time.sleep(0.1)
        raise Failure(f"no ting with key {key} reached the fake Ting within {timeout}s")

    def check_exact_body(self, ting: dict, ting_type: str) -> dict:
        """BLUEPRINT §3.5 step 3: TingSend {org_id, type, for, key, data,
        metadata} in that order, compact, data/metadata with sorted keys."""
        body = ting["body"]
        expected = (
            "{"
            + f'"org_id":{json.dumps(ORG)},"type":{json.dumps(ting_type)},"for":{json.dumps(ACTOR)},'
            + f'"key":{json.dumps(body["key"], ensure_ascii=False)},'
            + f'"data":{json.dumps(body["data"], separators=(",", ":"), sort_keys=True, ensure_ascii=False)},'
            + f'"metadata":{json.dumps(body["metadata"], separators=(",", ":"), sort_keys=True, ensure_ascii=False)}'
            + "}"
        )
        check(ting["raw"] == expected, f"{ting_type}: the Ting body is exactly the §3.5 TingSend serialization")
        check(ting["content_type"] == "application/json", f"{ting_type}: Content-Type application/json")
        check(ting["idempotency_key_header"] is None, f"{ting_type}: no Idempotency-Key header to Ting (the body key is the idempotency)")
        check(body["metadata"] == {"isi": ISI, "peek_version": PEEK_VERSION}, f"{ting_type}: metadata is {{isi, peek_version}}")
        data = body["data"]
        check(data["schema"] == 1 and data["slot"] == SLOT and data["context"] == "production", f"{ting_type}: schema 1, slot {SLOT}, production")
        return data

    def ask(self, question: str, *extra: str) -> tuple[dict, dict]:
        ask_json = json.dumps({"question": question, "type": "single_choice", "options": OPTIONS})
        r = self.peek("send", "--ask", ask_json, *extra)
        check(r.get("ask_id", "").startswith("ask_"), "send --ask returns an ask_id")
        show = self.ui.wait_event(lambda e: e.get("event") == "peek.show" and e.get("send_id") == r["send_id"], 15, "peek.show for the ask")
        check(show.get("ask_id") == r["ask_id"], "peek.show carries the top-level ask_id")
        check(show["ask"]["question"] == question, "peek.show carries the question")
        return r, show

    # ----------------------------------------------------------------- steps
    def login(self) -> None:
        print("login")
        r = self.peek("login", f"e2e-slt-{os.getpid()}-{int(time.time())}")
        check(r["authenticated"] is True and r["id"] == ACTOR and r["org_id"] == ORG, "peek login authenticates the fake SLT as si:e2e-silicon in tos")
        check(r["ting"]["subscribed"] is True, "login enrolled the Ting recipient grant (proof-checked by the fake Ting)")
        subs = self.get(f"{self.stack.ting}/_fake/subscriptions")["subscriptions"]
        check(len(subs) == 1 and json.loads(subs[0]["raw"]) == {"org_id": ORG, "app_id": "peek", "for": ACTOR}, "the registration body is exactly {org_id, app_id, for}")
        s = self.peek("login", "status")
        check(s["authenticated"] is True and s["custody"] == "client" and s["daemon"]["attached"] is True, "peek login status: authenticated, attached to peekd")

    def register(self) -> None:
        print("register")
        r = self.peek("register", "side", str(SLOT))
        check(r["slot"]["index"] == SLOT and r.get("hotkey") == f"ctrl+cmd+{SLOT}", f"register side {SLOT} → hotkey ctrl+cmd+{SLOT}")
        state = self.ui.wait_event(lambda e: e.get("event") == "slots.state" and any(s.get("index") == SLOT and s.get("actor_id") == ACTOR for s in e.get("slots", [])), 10, "slots.state with slot 3")
        slot = next(s for s in state["slots"] if s["index"] == SLOT)
        check(slot["context"] == "production" and slot["org_id"] == ORG, "slots.state names the Silicon on its slot")
        path = self.stack.root / "e2e-visual.js"
        path.write_text(DRAWING)
        sha = hashlib.sha256(DRAWING.encode()).hexdigest()
        d = self.peek("register", "drawing", str(path), timeout=130)
        validate = next(q for q in self.ui.requests if q["op"] == "drawing.validate")
        check(os.path.basename(validate["script_path"]) == "e2e-visual.js", "drawing.validate stages the script under its original filename")
        check(d["sha256"] == sha and d["active"] is True, "register drawing: validated by the UI and active")
        check(d["stats"]["frames"] == 90, "the validator's stats come back to the CLI")
        deadline = time.monotonic() + 10
        while not any(q["op"] == "drawing.load" and q.get("sha256") == sha for q in self.ui.requests):
            if time.monotonic() > deadline:
                raise Failure("peekd never asked the UI to drawing.load the new drawing")
            time.sleep(0.05)
        check(True, "peekd asked the UI to drawing.load it (same sha256)")
        deadline = time.monotonic() + 20
        while True:
            status, headers, body = self.api_get("/api/v1/drawings/current")
            if status == 200:
                break
            if time.monotonic() > deadline:
                raise Failure(f"peekd never uploaded the drawing to peek-server (last status {status})")
            time.sleep(0.3)
        check(body == DRAWING.encode() and headers.get("x-peek-drawing-sha256") == sha, "peekd's outbox uploaded the drawing to peek-server (same bytes and sha256)")

    def session(self) -> tuple[str, str]:
        """The CLI's current access token and org (test harness only)."""
        store = self.stack.root / "silicon" / ".peek" / "session.json"
        slot = json.loads(store.read_text())["slots"][f"{self.stack.api}#production"]
        return slot["access_token"], slot["org_id"]

    def api_get(self, path: str) -> tuple[int, dict, bytes]:
        token, org = self.session()
        req = urllib.request.Request(f"{self.stack.api}{path}", headers={"Authorization": f"Bearer {token}", "X-Org-ID": org})
        try:
            with urllib.request.urlopen(req, timeout=10) as r:  # noqa: S310 - loopback
                return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
        except urllib.error.HTTPError as e:
            return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read()

    def speak(self) -> bytes:
        print("send --speak (real ElevenLabs through a direct Deepgram connection)")
        t0 = time.monotonic()
        r = self.peek("send", "--speak", SPEAK_TEXT, "--voice", "JBFqnCBsd6RMkjVDRZzb", "--voice-instructions", "Warm and relaxed")
        check(r["speech"]["status"] in ("pending", "cached"), "send --speak queues speech")
        check(r["speech"]["model"] == "JBFqnCBsd6RMkjVDRZzb", "send --voice selects the ElevenLabs voice")
        sid = r["send_id"]
        begin = self.ui.wait_event(lambda e: e.get("event") == "tts.begin" and e.get("send_id") == sid, 25, "tts.begin")
        check(begin["format"] == "s16le" and begin["sample_rate"] == 24000 and begin["channels"] == 1, "tts.begin: s16le 24 kHz mono")
        check(begin["est_frames"] > 0, "tts.begin carries est_frames")
        end = self.ui.wait_event(lambda e: e.get("event") in ("tts.end", "tts.error") and e.get("send_id") == sid, 30, "tts.end")
        check(end["event"] == "tts.end", "the stream ends with tts.end (no tts.error)")
        chunks = [e for e in self.ui.events_named("tts.chunk") if e["send_id"] == sid]
        check([c["seq"] for c in chunks] == list(range(len(chunks))), f"tts.chunk seq runs 0…{len(chunks) - 1} without gaps")
        pcm = bytes(self.ui.audio[sid])
        check(end["total_frames"] == len(pcm) // 2 and len(pcm) > 24000, f"tts.end total_frames = bytes ÷ 2 ({end['total_frames']} frames, {len(pcm) / 48000:.2f} s)")
        self.metrics["tts_first_chunk_ms_after_send"] = (self.ui.tts_first_chunk_at[sid] - t0) * 1000
        total_ms = len(pcm) // 48
        self.ui.call("speech.done", {"send_id": sid, "stopped_by_user": False, "played_ms": total_ms, "total_ms": total_ms})
        self.ui.call("shown.done", {"send_id": sid, "visible_ms": total_ms + 1500, "reason": "speech_done"})
        check(True, "the fake UI reported speech.done and shown.done")
        return pcm

    def click_answer(self) -> None:
        print("send --ask single_choice → click")
        r, _ = self.ask("Which one should I open?")
        t0 = time.monotonic()
        self.ui.call("answer", {"send_id": r["send_id"], "ask_id": r["ask_id"], "value": "first", "via": "click"})
        self.metrics["answer_ack_ms"] = (time.monotonic() - t0) * 1000
        check(self.metrics["answer_ack_ms"] < 500, f"answer acked promptly ({self.metrics['answer_ack_ms']:.0f} ms)")
        ting = self.wait_ting(f"{ACTOR}/{r['ask_id']}/answered")
        data = self.check_exact_body(ting, "peek.ask.answered")
        check(data["answer"] == {"kind": "single_choice", "option_id": "first", "label": "The first one"}, "answer {kind, option_id, label}")
        check(data["via"] == "click" and data.get("transcript") is None, "via click, no transcript")
        check(data["ask_id"] == r["ask_id"] and data["send_id"] == r["send_id"] and data["ask_type"] == "single_choice", "ask_id, send_id, ask_type")
        g = self.wait_delivery(r["ask_id"])
        check(g["delivery"]["ting_id"] == ting["id"], "peek ask get shows the accepted delivery with Ting's id")

    def presence(self, speech: bool) -> None:
        print("presence: a locked screen holds bubbles; unlocking shows them in order")
        self.ui.call("presence", {"available": False, "reason": "locked"})
        s = self.peek("status")
        check(s.get("carbon") == {"available": False, "reason": "locked", "paused": False}, "peek status reports the Carbon away (locked)")
        first_args = ["--speak", "Welcome back."] if speech else ["--show", json.dumps({"elements": [{"type": "text", "text": "welcome back"}]})]
        a = self.peek("send", *first_args)
        check(a["status"] == "queued" and any(w["code"] == "carbon_away" for w in a["warnings"]), "a send while the screen is locked is queued with warning carbon_away")
        b = self.peek("send", "--show", json.dumps({"elements": [{"type": "text", "text": "second"}]}))
        check(b["status"] == "queued", "a second send waits behind it instead of replacing it")
        time.sleep(0.8)
        held = {a["send_id"], b["send_id"]}
        pushed = [e for e in self.ui.events_named("peek.show") + self.ui.events_named("tts.begin") if e.get("send_id") in held]
        check(not pushed, "no peek.show and no speech reach the UI while the Carbon is away")
        self.ui.call("presence", {"available": True, "reason": "ok"})
        first = self.ui.wait_event(lambda e: e.get("event") == "peek.show" and e.get("send_id") == a["send_id"], 10, "peek.show of the first held send")
        check(first["queued_behind"] == 1, "unlocking shows the held sends in order (one still behind)")
        if speech:
            end = self.ui.wait_event(lambda e: e.get("event") in ("tts.end", "tts.error") and e.get("send_id") == a["send_id"], 30, "speech of the held send")
            check(end["event"] == "tts.end", "speech starts only once the bubble is actually shown")
            total_ms = end["total_frames"] // 24
            self.ui.call("speech.done", {"send_id": a["send_id"], "stopped_by_user": False, "played_ms": total_ms, "total_ms": total_ms})
            # A speak-only bubble closes on its slide-back (shown.done), never on speech.done.
            self.ui.call("shown.done", {"send_id": a["send_id"], "visible_ms": total_ms + 1500, "reason": "speech_done"})
        else:
            self.ui.call("shown.done", {"send_id": a["send_id"], "visible_ms": 4000, "reason": "auto"})
        self.ui.wait_event(lambda e: e.get("event") == "peek.show" and e.get("send_id") == b["send_id"], 15, "peek.show of the second held send")
        self.ui.call("shown.done", {"send_id": b["send_id"], "visible_ms": 4000, "reason": "auto"})
        s = self.peek("status")
        check(s.get("carbon", {}).get("available") is True, "peek status reports the Carbon back")
        h = self.peek("history", "--limit", "5")
        held_items = [i for i in h.get("items", []) if i["send_id"] == a["send_id"]]
        check(bool(held_items) and any(w["code"] == "carbon_away" for w in held_items[0].get("warnings") or []), "history keeps the carbon_away warning")

    # ------------------------------------------------------------ 0.1.2 steps
    @staticmethod
    def show_json(text: str) -> str:
        return json.dumps({"elements": [{"type": "text", "text": text}]})

    @staticmethod
    def error_of(r: dict) -> dict:
        """The {"error":{…}} object a failed --json run printed on stderr."""
        for line in reversed(r.get("_stderr", "").splitlines()):
            try:
                return json.loads(line)["error"]
            except (json.JSONDecodeError, KeyError, TypeError):
                continue
        raise Failure(f"no JSON error on stderr: {r.get('_stderr')!r}")

    def wait_show(self, send_id: str, what: str) -> dict:
        return self.ui.wait_event(lambda e: e.get("event") == "peek.show" and e.get("send_id") == send_id, 15, what)

    def done(self, send_id: str) -> None:
        self.ui.call("shown.done", {"send_id": send_id, "visible_ms": 4000, "reason": "auto"})

    def queue_v2(self) -> None:
        print("queue v2: always queue (strict FIFO), queue_full, peek queue, cancel, clear, --replace")
        tings_before = len(self.tings())
        a = self.peek("send", "--show", self.show_json("A"))
        check(a["status"] == "showing" and a["queue_position"] == 0 and a["waiting"] == 0, "the first send is shown (queue position 0)")
        self.wait_show(a["send_id"], "peek.show of A")
        waiting = []
        for text in ["B", "C", "D", "E", "F"]:
            r = self.peek("send", "--show", self.show_json(text))
            check(r["status"] == "queued" and r["queue_position"] == len(waiting) + 1, f"{text} waits at position {len(waiting) + 1} (nothing is replaced)")
            waiting.append(r["send_id"])
        badge = self.ui.wait_event(lambda e: e.get("event") == "queue.state" and e.get("send_id") == a["send_id"] and e.get("waiting") == 5, 10, "queue.state +5")
        check(badge["slot"] == SLOT, "queue.state tells the UI the +5 badge of the bubble on screen")
        full = self.peek("send", "--show", self.show_json("G"), ok=False)
        err = self.error_of(full)
        check(full["_exit"] == 4 and err["code"] == "queue_full", "a sixth waiting send fails with queue_full (exit 4)")
        check(err["details"]["queued"] == 5 and err["details"]["limit"] == 5 and err["details"]["on_screen"] == a["send_id"], "queue_full details: queued 5, limit 5, the send on screen")
        check("peek cancel <send_id>" in err["message"] and "peek queue clear" in err["message"], "the message says how to make room")
        q = self.peek("queue")
        check(q["on_screen"]["send_id"] == a["send_id"] and [w["send_id"] for w in q["waiting"]] == waiting, "peek queue lists the one on screen and the five waiting, in order")
        c = self.peek("cancel", waiting[1])
        check(c["was"] == "waiting" and c["queue_position"] == 2 and c["state"] == "cancelled", "peek cancel withdraws C (#2 in line)")
        cleared = self.peek("queue", "clear")
        rest = [waiting[0], *waiting[2:]]
        check(cleared["cancelled"] == rest and cleared["on_screen_cancelled"] is False, "peek queue clear drops the four still waiting; A stays")
        self.ui.wait_event(lambda e: e.get("event") == "queue.state" and e.get("send_id") == a["send_id"] and e.get("waiting") == 0, 10, "queue.state +0")
        x = self.peek("send", "--show", self.show_json("X"))
        y = self.peek("send", "--show", self.show_json("Y"))
        self.done(a["send_id"])
        sx = self.wait_show(x["send_id"], "peek.show of X")
        check(sx["queued_behind"] == 1, "A done → X shown next with Y behind it (FIFO)")
        self.done(x["send_id"])
        self.wait_show(y["send_id"], "peek.show of Y")
        z = self.peek("send", "--show", self.show_json("Z"), "--replace")
        check(z["status"] == "showing" and z["replaced_send_id"] == y["send_id"], "--replace takes over Y at once")
        cancel = self.ui.wait_event(lambda e: e.get("event") == "peek.cancel" and e.get("send_id") == y["send_id"], 10, "peek.cancel of Y")
        check(cancel["reason"] == "replaced", "peek.cancel{reason: replaced} for the replaced bubble")
        sz = self.wait_show(z["send_id"], "peek.show of Z")
        check(sz.get("replaces") == y["send_id"], "peek.show of Z names the send it replaces")
        self.done(z["send_id"])
        h = self.peek("history", "--limit", "20")
        reasons = {i["send_id"]: i.get("close_reason") for i in h["items"]}
        check(reasons.get(waiting[1]) == "cancelled" and reasons.get(waiting[0]) == "cleared" and reasons.get(y["send_id"]) == "replaced", "history: cancelled, cleared and replaced")
        time.sleep(1.0)
        check(len(self.tings()) == tings_before, "no Ting event for the Silicon's own cancel, clear or replace")

    def expiry_while_locked(self) -> None:
        print("--expires-in on a show while the screen is locked: expires unseen")
        self.ui.call("presence", {"available": False, "reason": "locked", "paused": False})
        r = self.peek("send", "--show", self.show_json("stale news"), "--expires-in", "10s")
        check(r["status"] == "queued" and r["expires_at"] is not None, "the show waits (held) with its deadline")
        ting = self.wait_ting(f"{ACTOR}/{r['send_id']}/send_expired", timeout=40)
        data = self.check_exact_body(ting, "peek.send.expired")
        check(data["send_id"] == r["send_id"] and data["kind"] == "show", "peek.send.expired names the send and its kind")
        check(data["shown"] is False and data["shown_at"] is None, "it says the send was never shown")
        check(data["scheduled"] is False and data["schedule_id"] is None, "it was not scheduled")
        self.ui.call("presence", {"available": True, "reason": "ok", "paused": False})
        time.sleep(1.0)
        check(not any(e.get("send_id") == r["send_id"] for e in self.ui.events_named("peek.show")), "an expired send is never shown")

    def scheduling(self) -> None:
        print("scheduling: --in 5s comes due, is shown, and both events reach the Silicon")
        r = self.peek("send", "--show", self.show_json("scheduled hello"), "--in", "5s")
        check(r["status"] == "scheduled" and r["schedule_id"].startswith("sch_") and r["queue_position"] is None, "send --in 5s is scheduled")
        listed = self.peek("schedule", "list")
        check([i["schedule_id"] for i in listed["scheduled"]] == [r["schedule_id"]], "peek schedule list shows it")
        due = self.wait_ting(f"{ACTOR}/{r['schedule_id']}/due", timeout=40)
        data = self.check_exact_body(due, "peek.schedule.due")
        check(data["send_id"] == r["send_id"] and data["outcome"] == "shown" and data["replaced_send_id"] is None, "peek.schedule.due: outcome shown, same send_id")
        show = self.wait_show(r["send_id"], "peek.show of the scheduled send")
        check(show.get("schedule_id") == r["schedule_id"], "peek.show carries the schedule_id")
        shown = self.wait_ting(f"{ACTOR}/{r['send_id']}/shown", timeout=20)
        data = self.check_exact_body(shown, "peek.send.shown")
        check(data["scheduled"] is True and data["schedule_id"] == r["schedule_id"], "peek.send.shown after the UI's shown, scheduled true")
        self.done(r["send_id"])
        later = self.peek("send", "--speak", "later", "--in", "1h")
        c = self.peek("schedule", "cancel", later["schedule_id"])
        check(c["state"] == "cancelled" and c["send_id"] == later["send_id"], "peek schedule cancel withdraws one before it is due")
        fired = self.peek("schedule", "cancel", r["schedule_id"])
        check(fired["state"] == "fired", "cancelling one that already fired says so")
        self.peek("send", "--speak", "one", "--in", "2h")
        self.peek("send", "--speak", "two", "--in", "3h")
        cl = self.peek("schedule", "clear")
        check(len(cl["cancelled"]) == 2, "peek schedule clear cancels the rest")

    def wait_delivery(self, ask_id: str) -> dict:
        deadline = time.monotonic() + 15
        while True:
            g = self.peek("ask", "get", ask_id)
            if (g.get("delivery") or {}).get("status") == "accepted":
                return g
            if time.monotonic() > deadline:
                raise Failure(f"ask {ask_id} delivery never became accepted: {g.get('delivery')}")
            time.sleep(0.3)

    def voice_answer(self, pcm24: bytes) -> None:
        print("voice answer (real STT through the speech proxy)")
        r, _ = self.ask("Which one did you mean?")
        pcm16 = resample_s16le(pcm24, 24000, 16000)
        wav = wav_bytes(pcm16, 16000)
        duration = len(pcm16) * 1000 // 32000
        t0 = time.monotonic()
        reply = self.ui.call("voice.submit", {"send_id": r["send_id"], "ask_id": r["ask_id"], "slot": SLOT, "duration_ms": duration, "languages": ["en-US"]}, [wav])
        ack = (time.monotonic() - t0) * 1000
        check(ack < 500, f"voice.submit acked promptly ({ack:.0f} ms)")
        check(reply == {"message_id": None}, f"a voice answer's voice.submit reply names no message ({reply})")
        stt = self.ui.wait_event(lambda e: e.get("event") == "stt.result" and e.get("ask_id") == r["ask_id"], 30, "stt.result")
        self.metrics["stt_ms_after_submit"] = (stt["_at"] - t0) * 1000
        check(stt["outcome"] == "matched" and stt["value"] == "second", f"stt.result matched option `second` ({self.metrics['stt_ms_after_submit']:.0f} ms)")
        ting = self.wait_ting(f"{ACTOR}/{r['ask_id']}/answered")
        data = self.check_exact_body(ting, "peek.ask.answered")
        check(data["answer"]["option_id"] == "second" and data["via"] == "voice", "the voice answer is delivered as option `second`, via voice")
        check("second" in (data.get("transcript") or "").lower(), f"the ting carries the final transcript ({data.get('transcript')!r})")

    def voice_message(self, pcm24: bytes) -> None:
        print("Carbon voice message (real STT; the voice.submit reply names the message)")
        pcm16 = resample_s16le(pcm24, 24000, 16000)
        wav = wav_bytes(pcm16, 16000)
        duration = len(pcm16) * 1000 // 32000
        t0 = time.monotonic()
        reply = self.ui.call("voice.submit", {"send_id": None, "ask_id": None, "slot": SLOT, "duration_ms": duration, "languages": ["en-US"]}, [wav])
        message_id = (reply or {}).get("message_id") or ""
        check(message_id.startswith("cmsg_"), f"voice.submit names the voice message ({message_id})")
        stt = self.ui.wait_event(lambda e: e.get("event") == "stt.result" and e.get("message_id") == message_id, 30, "stt.result for the voice message")
        self.metrics["voice_message_stt_ms"] = (stt["_at"] - t0) * 1000
        check(stt.get("ask_id") is None and stt["outcome"] == "matched", f"stt.result for {message_id} is matched")
        check("second" in str(stt.get("value") or "").lower(), f"stt.result carries the transcript ({stt.get('value')!r})")
        ting = self.wait_ting(f"{ACTOR}/{message_id}/message")
        data = self.check_exact_body(ting, "peek.message.received")
        check(data["message_id"] == message_id and data["via"] == "voice", "the voice message is delivered under the id the reply named, via voice")
        check("second" in data["text"].lower(), f"the ting carries the transcript as the message text ({data['text']!r})")

    def message(self) -> None:
        print("Carbon message")
        last_send = self.peek("history", "--limit", "1")["items"][0]["send_id"]
        m = self.ui.call("message", {"slot": SLOT, "text": "remind me about this at 5", "via": "keyboard"})
        check(m["message_id"].startswith("cmsg_"), "message returns a cmsg_ id")
        ting = self.wait_ting(f"{ACTOR}/{m['message_id']}/message")
        data = self.check_exact_body(ting, "peek.message.received")
        check(data["text"] == "remind me about this at 5" and data["via"] == "keyboard", "the message text and via")
        check(data["in_reply_to"] == last_send, "in_reply_to is the slot's most recent send")

    def refresh(self) -> None:
        print("refresh (IAM invalidates the access token early)")
        before = self.get(f"{self.stack.iam}/_fake/state")["refreshes"]
        req = urllib.request.Request(f"{self.stack.iam}/_fake/expire-access", data=b"", method="POST")
        with urllib.request.urlopen(req, timeout=5):  # noqa: S310 - loopback fake
            pass
        s = self.peek("login", "status")
        check(s["authenticated"] is True, "login status: /me → 401 → one forced refresh → authenticated")
        check(self.get(f"{self.stack.iam}/_fake/state")["refreshes"] == before + 1, "the refresh token was rotated exactly once at IAM")

    def wait_answer(self) -> None:
        print("send --ask --wait (answer on stdout instead of a ting)")
        ask_json = json.dumps({"question": "Open the third one?", "type": "single_choice", "options": OPTIONS})
        argv = [str(local_stack.BIN / "peek"), "send", "--ask", ask_json, "--wait", "30", "--json"]
        p = subprocess.Popen(argv, env=self.env, cwd=self.stack.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, stdin=subprocess.DEVNULL)
        show = self.ui.wait_event(lambda e: e.get("event") == "peek.show" and (e.get("ask") or {}).get("question") == "Open the third one?", 15, "peek.show for the --wait ask")
        self.ui.call("answer", {"send_id": show["send_id"], "ask_id": show["ask_id"], "value": "third", "via": "keyboard"})
        out, err = p.communicate(timeout=30)
        check(p.returncode == 0, f"send --ask --wait 30 exits 0 ({err.strip()[:200]})")
        result = json.loads(out)
        check(result["state"] == "answered" and result["answer"]["option_id"] == "third" and result["via"] == "keyboard", "the answer is printed on stdout")
        time.sleep(1.5)
        check(not any(t["body"]["key"] == f"{ACTOR}/{show['ask_id']}/answered" for t in self.tings()), "no ting for an answer delivered to a live waiter (D22)")

    def doctor(self) -> None:
        print("ui.status push and doctor")
        status = {
            "mic": "granted",
            "hotkeys": {"modifier": "ctrl+cmd", "registered": [f"ctrl+cmd+{SLOT}"], "failed": [], "problems": []},
            "glass": "live",
            "services": "disabled",
            "app_build": 1002,
            "app_version": "0.1.2",
        }
        check(self.ui.call("ui.status", status) == {}, "peekd accepts the app's ui.status push")
        d = self.peek("doctor")
        checks = {c["name"]: c for c in d.get("checks", [])}
        check(checks.get("mic", {}).get("status") == "ok", "doctor relays the UI's microphone state through peekd")
        check(checks.get("hotkeys", {}).get("status") == "ok", "doctor relays the UI's hotkey registrations through peekd")

    def logout(self) -> None:
        print("logout")
        before = len(self.get(f"{self.stack.iam}/_fake/state")["revocations"])
        r = self.peek("logout")
        check(r.get("remote_revocation") in ("confirmed", None) or r.get("logged_out") is True, f"peek logout ({ {k: v for k, v in r.items() if not k.startswith('_')} })")
        after = self.get(f"{self.stack.iam}/_fake/state")["revocations"]
        check(len(after) > before, "logout revoked the token family at IAM")
        s = self.peek("login", "status", ok=False)
        check(s.get("authenticated") is False, "login status is unauthenticated after logout")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--keep", action="store_true", help="keep the stack directory (logs) even on success")
    parser.add_argument("--no-stt", action="store_true", help="skip OpenAI transcription; ElevenLabs TTS still runs when configured")
    args = parser.parse_args()

    root = Path(tempfile.mkdtemp(prefix="peek-e2e."))
    stack = None
    ui = None
    ok = False
    try:
        stack = local_stack.start(root, no_build=args.no_build, stt=not args.no_stt, ui_executable=process_executable(), quiet=True)
        print(f"stack: {stack.root} (server {stack.api}, peekd {stack.socket})")
        ui = FakeUi(stack.socket)
        hello = ui.connect()
        check(hello["protocol"] == 1, f"the fake UI completed hello (peekd {hello['peekd_version']})")
        run = Run(stack, ui)
        run.login()
        run.register()
        pcm = None
        if stack.state.get("elevenlabs"):
            pcm = run.speak()
        else:
            print("send --speak: SKIPPED (no Deepgram key)")
        run.click_answer()
        run.presence(speech=pcm is not None)
        if pcm is not None and stack.state.get("openai"):
            run.voice_answer(pcm)
            run.voice_message(pcm)
        run.message()
        run.refresh()
        run.wait_answer()
        run.queue_v2()
        run.expiry_while_locked()
        run.scheduling()
        run.doctor()
        run.logout()
        ok = True
        print("\nPASS")
        for k, v in run.metrics.items():
            print(f"  {k}: {v:.0f}")
        return 0
    except (Failure, TimeoutError) as e:
        print(f"\nFAIL: {e}", file=sys.stderr)
        return 1
    finally:
        if ui:
            ui.close()
        if stack:
            local_stack.stop(stack.root)
        if ok and not args.keep:
            shutil.rmtree(root, ignore_errors=True)
        else:
            print(f"logs kept in {root}/logs and {root}/support/peekd.log", file=sys.stderr)


if __name__ == "__main__":
    sys.exit(main())
