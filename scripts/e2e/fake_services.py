#!/usr/bin/env python3
"""Local fakes of IAM and Ting for end-to-end runs of peek (development only).

They speak exactly the routes peek-server calls, with the request and response
shapes of silicon-iam-client 4.0.0 and Ting 0.1.x, keep everything in memory,
and never talk to a real service. NOT a security boundary.

    python3 scripts/e2e/fake_services.py --iam-port 18081 --ting-port 18082 \
        [--app-secret ask_...] [--ready-file PATH]

IAM (http://127.0.0.1:<iam-port>)
  POST /api/v1/app-auth/tokens         form app_id + slt | refresh_token (login, refresh).
                                       Any SLT logs in as FAKE_ACTOR (si:e2e-silicon) in
                                       FAKE_ORG (tos); an SLT is single use, except that the
                                       same Idempotency-Key replays the first response.
  POST /api/v1/oauth/introspect        form token; org-bound with X-Org-ID, else unscoped
                                       (the SDK's `authorizations()`).
  POST /api/v1/oauth/revoke            form token (+ token_type_hint); unknown tokens are fine.
  GET  /api/v1/me                      Bearer oat_ (application reads).
  GET  /api/v1/obo-access/applications/ting/endpoints
  POST /api/v1/obo-access/exchanges    verifies the X-OBO-Signature HMAC when --app-secret is
                                       given, the subject token and org, then mints a
                                       single-use proof bound to (endpoint, method, body_sha256).
  GET  /_fake/state                    logins, refreshes, revocations and exchanges (for tests).
  POST /_fake/expire-access            invalidates every access token (refresh tokens keep
                                       working), so the next call must refresh.

Ting (http://127.0.0.1:<ting-port>)
  POST /v1/subscriptions, /v1/subscriptions/revoke, /v1/tings
                                       each consumes a proof and checks that sha256(raw body)
                                       equals the digest the proof was minted for.
  GET  /_fake/tings                    every accepted /v1/tings call: the exact raw body
                                       (`raw`), its parsed JSON (`body`) and the test headers.
  GET  /_fake/subscriptions            every accepted registration.
  POST /_fake/reset                    forget recorded tings and subscriptions.

Environment: FAKE_ACTOR, FAKE_ORG, FAKE_DISPLAY_NAME.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import secrets
import signal
import sys
import threading
import time
import urllib.parse
import uuid
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ACTOR = os.environ.get("FAKE_ACTOR", "si:e2e-silicon")
ORG = os.environ.get("FAKE_ORG", "tos")
DISPLAY_NAME = os.environ.get("FAKE_DISPLAY_NAME", "E2E Silicon")
APP_ID = "peek"
SCOPES = [
    "obo:ting:subscriptions.register",
    "obo:ting:subscriptions.revoke",
    "obo:ting:tings.send",
    "self.identity.read",
    "self.membership.read",
    "self.profile.read",
]
ACCESS_TTL = 1800
PROOF_TTL = 60
ORG_UUID = str(uuid.UUID(int=0x0192F2D27C9E7CC08B2E6F3A2B1C0D9E))
TING_ENDPOINTS = {
    "subscriptions.register": "/v1/subscriptions",
    "subscriptions.revoke": "/v1/subscriptions/revoke",
    "tings.send": "/v1/tings",
}


class State:
    """Everything both fakes share (one process, one lock)."""

    def __init__(self, app_secret: str | None) -> None:
        self.lock = threading.Lock()
        self.app_secret = app_secret
        self.access: dict[str, float] = {}  # oat_ -> expiry
        self.refresh: dict[str, str] = {}  # ort_ -> current oat_
        self.spent_slts: set[str] = set()
        self.replays: dict[str, dict] = {}  # idempotency key -> token response
        self.proofs: dict[str, dict] = {}  # proof -> binding
        self.logins: list[dict] = []
        self.refreshes = 0
        self.revocations: list[dict] = []
        self.exchanges: list[dict] = []
        self.tings: list[dict] = []
        self.subscriptions: list[dict] = []
        self.subscribed = False

    def active(self, token: str) -> bool:
        with self.lock:
            return self.access.get(token, 0) > time.time()


STATE = State(None)


def now_rfc3339(offset: int = 0) -> str:
    return (datetime.now(timezone.utc) + timedelta(seconds=offset)).strftime("%Y-%m-%dT%H:%M:%SZ")


def kind(actor: str) -> str:
    return "silicon" if actor.startswith("si:") else "carbon"


def token(prefix: str) -> str:
    return prefix + secrets.token_urlsafe(24).replace("-", "a").replace("_", "b")


def authorization() -> dict:
    return {
        "actor_type": kind(ACTOR),
        "public_id": ACTOR,
        "organization_id": ORG_UUID,
        "org_id": ORG,
        "membership_id": f"{ACTOR}[{ORG}]",
        "membership_version": 1,
        "authorization_epoch": 1,
        "audience": APP_ID,
        "testing_environment_id": None,
        "scopes": SCOPES,
        "org_role": "owner",
        "tags": None,
    }


def issue() -> dict:
    access, refresh = token("oat_"), token("ort_")
    with STATE.lock:
        STATE.access[access] = time.time() + ACCESS_TTL
        STATE.refresh[refresh] = access
    return {
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL,
        "scope": " ".join(SCOPES),
        "actor": {"type": kind(ACTOR), "public_id": ACTOR},
    }


class Handler(BaseHTTPRequestHandler):
    server_version = "peek-e2e-fake/1"
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt: str, *args) -> None:  # stdlib signature
        # Never log bodies or credentials; the method, path and status are enough.
        sys.stderr.write(f"[fake {self.server.name}] {self.command} {self.path.split('?')[0]} {args[1] if len(args) > 1 else ''}\n")  # type: ignore[attr-defined]

    def raw_body(self) -> bytes:
        return self.rfile.read(int(self.headers.get("content-length") or 0))

    def form(self, raw: bytes) -> dict[str, str]:
        return {k: v[0] for k, v in urllib.parse.parse_qs(raw.decode(), keep_blank_values=True).items()}

    def bearer(self) -> str:
        return self.headers.get("authorization", "").removeprefix("Bearer ")

    def reply(self, status: int, payload: object = None) -> None:
        data = b"" if payload is None else json.dumps(payload).encode()
        self.send_response(status)
        if payload is not None:
            self.send_header("content-type", "application/json")
        self.send_header("x-request-id", str(uuid.uuid4()))
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def error(self, status: int, code: str, message: str | None = None) -> None:
        self.reply(status, {"error": {"code": code, "message": message or f"fake: {code}", "request_id": str(uuid.uuid4())}})


class IamHandler(Handler):
    def app_authenticated(self) -> bool:
        """Basic peek:<app secret> when the fake knows the secret."""
        header = self.headers.get("authorization", "")
        if not header.startswith("Basic "):
            return False
        try:
            user, _, secret = base64.b64decode(header[6:]).decode().partition(":")
        except ValueError:
            return False
        if user != APP_ID:
            return False
        return STATE.app_secret is None or hmac.compare_digest(secret, STATE.app_secret)

    def do_GET(self) -> None:  # noqa: N802 - stdlib naming
        path = self.path.split("?")[0]
        if path == "/api/v1/obo-access/applications/ting/endpoints":
            self.reply(200, {
                "application": {"app_id": "ting", "org_id": ORG},
                "endpoints": [
                    {"endpoint_id": e, "path": p, "critical": True, "metadata": {}, "ttl_seconds": PROOF_TTL}
                    for e, p in TING_ENDPOINTS.items()
                ],
            })
        elif path == "/api/v1/me":
            if not STATE.active(self.bearer()):
                self.error(401, "unauthenticated")
            else:
                self.reply(200, {"principal_id": ACTOR, "type": kind(ACTOR), "display_name": DISPLAY_NAME, "version": 1})
        elif path == "/_fake/state":
            with STATE.lock:
                self.reply(200, {
                    "logins": STATE.logins,
                    "refreshes": STATE.refreshes,
                    "revocations": STATE.revocations,
                    "exchanges": STATE.exchanges,
                    "active_access_tokens": sum(1 for e in STATE.access.values() if e > time.time()),
                })
        else:
            self.error(404, "not_found")

    def do_POST(self) -> None:  # noqa: N802 - stdlib naming
        path = self.path.split("?")[0]
        raw = self.raw_body()
        if path == "/api/v1/app-auth/tokens":
            self.tokens(raw)
        elif path == "/api/v1/oauth/introspect":
            self.introspect(raw)
        elif path == "/api/v1/oauth/revoke":
            if not self.app_authenticated():
                self.error(401, "invalid_client")
                return
            form = self.form(raw)
            tok = form.get("token", "")
            with STATE.lock:
                access = STATE.refresh.pop(tok, None)
                STATE.access.pop(access or tok, None)
                STATE.revocations.append({"kind": form.get("token_type_hint"), "known": access is not None or tok.startswith("oat_")})
            self.reply(200)
        elif path == "/api/v1/obo-access/exchanges":
            self.exchange(raw)
        elif path == "/_fake/expire-access":
            with STATE.lock:
                expired = len(STATE.access)
                STATE.access.clear()
            self.reply(200, {"expired": expired})
        else:
            self.error(404, "not_found")

    def tokens(self, raw: bytes) -> None:
        if not self.app_authenticated():
            self.error(401, "invalid_client")
            return
        form = self.form(raw)
        if form.get("app_id") != APP_ID:
            self.error(400, "invalid_request", "app_id must be peek")
            return
        key = self.headers.get("idempotency-key")
        if "slt" in form:
            with STATE.lock:
                replay = STATE.replays.get(key or "")
                spent = form["slt"] in STATE.spent_slts
            if replay is not None:
                self.reply(200, replay)
                return
            if spent or not form["slt"]:
                self.error(400, "invalid_grant", "the short-lived token is spent or invalid")
                return
            response = issue()
            with STATE.lock:
                STATE.spent_slts.add(form["slt"])
                if key:
                    STATE.replays[key] = response
                STATE.logins.append({"at": now_rfc3339(), "idempotency_key": bool(key)})
            self.reply(200, response)
        elif "refresh_token" in form:
            with STATE.lock:
                replay = STATE.replays.get(key or "")
            if replay is not None:
                self.reply(200, replay)
                return
            with STATE.lock:
                old = STATE.refresh.pop(form["refresh_token"], None)
                if old:
                    STATE.access.pop(old, None)
            if old is None:
                self.error(400, "invalid_grant")
                return
            response = issue()
            with STATE.lock:
                STATE.refreshes += 1
                if key:
                    STATE.replays[key] = response
            self.reply(200, response)
        else:
            self.error(400, "invalid_request")

    def introspect(self, raw: bytes) -> None:
        if not self.app_authenticated():
            self.error(401, "invalid_client")
            return
        tok = self.form(raw).get("token", "")
        org = self.headers.get("x-org-id")
        if not STATE.active(tok) or (org is not None and org != ORG):
            self.reply(200, {"active": False})
        elif org is None:
            self.reply(200, {"active": True, "client_id": APP_ID, "audience": APP_ID, "authorizations": [authorization()]})
        else:
            with STATE.lock:
                expires = int(STATE.access[tok])
            self.reply(200, {
                "active": True, "public_id": ACTOR, "actor_type": kind(ACTOR), "client_id": APP_ID,
                "org_id": ORG, "membership_id": f"{ACTOR}[{ORG}]", "scope": " ".join(SCOPES),
                "audience": APP_ID, "issued_at": expires - ACCESS_TTL, "expires_at": expires,
                "authorization_epoch": 1, "authorization": authorization(),
            })

    def exchange(self, raw: bytes) -> None:
        if not self.app_authenticated():
            self.error(401, "invalid_client")
            return
        try:
            req = json.loads(raw)
            endpoint_id = req["endpoint_id"]
            binding = req["request"]
            method, digest = binding["method"], binding["body_sha256"]
        except (ValueError, KeyError, TypeError):
            self.error(400, "invalid_request")
            return
        path = TING_ENDPOINTS.get(endpoint_id)
        if req.get("audience") != "ting" or path is None:
            self.error(400, "invalid_request", "unknown audience or endpoint")
            return
        ts = self.headers.get("x-obo-timestamp", "")
        key = self.headers.get("idempotency-key", "")
        if STATE.app_secret is not None:
            canonical = f"{ts}.{method}.{path}.{digest}.{key}".encode()
            expected = hmac.new(STATE.app_secret.encode(), canonical, hashlib.sha256).hexdigest()
            if not hmac.compare_digest(expected, self.headers.get("x-obo-signature", "")):
                self.error(401, "invalid_signature")
                return
        if not ts.isdigit() or abs(time.time() - int(ts)) > 60:
            self.error(401, "timestamp_out_of_tolerance")
            return
        if not STATE.active(req.get("subject_token", "")) or req.get("org_id") not in (None, ORG):
            self.error(401, "invalid_subject_token")
            return
        proof = "proof_" + secrets.token_hex(16)
        with STATE.lock:
            STATE.proofs[proof] = {"endpoint_id": endpoint_id, "method": method, "body_sha256": digest, "expires": time.time() + PROOF_TTL}
            STATE.exchanges.append({"endpoint_id": endpoint_id, "body_sha256": digest, "at": now_rfc3339()})
        self.reply(200, {"access_proof": proof, "proof_id": str(uuid.uuid4()), "expires_in": PROOF_TTL, "expires_at": now_rfc3339(PROOF_TTL)})


class TingHandler(Handler):
    counter = 0

    def consume_proof(self, endpoint_id: str, raw: bytes) -> bool:
        proof = self.bearer()
        with STATE.lock:
            binding = STATE.proofs.pop(proof, None)
        if binding is None:
            self.error(401, "invalid_proof")
            return False
        if binding["expires"] < time.time():
            self.error(401, "proof_expired")
            return False
        if binding["endpoint_id"] != endpoint_id or binding["method"] != "POST":
            self.error(403, "permission_denied", "the proof was minted for another endpoint")
            return False
        if hashlib.sha256(raw).hexdigest() != binding["body_sha256"]:
            self.error(403, "permission_denied", "the body does not match the proof's body_sha256")
            return False
        return True

    def do_GET(self) -> None:  # noqa: N802 - stdlib naming
        path = self.path.split("?")[0]
        with STATE.lock:
            if path == "/_fake/tings":
                self.reply(200, {"tings": STATE.tings})
            elif path == "/_fake/subscriptions":
                self.reply(200, {"subscriptions": STATE.subscriptions})
            else:
                self.error(404, "not_found")

    def do_POST(self) -> None:  # noqa: N802 - stdlib naming
        path = self.path.split("?")[0]
        raw = self.raw_body()
        if path == "/_fake/reset":
            with STATE.lock:
                STATE.tings.clear()
                STATE.subscriptions.clear()
            self.reply(204)
            return
        endpoint = {v: k for k, v in TING_ENDPOINTS.items()}.get(path)
        if endpoint is None:
            self.error(404, "not_found")
            return
        if not self.consume_proof(endpoint, raw):
            return
        try:
            body = json.loads(raw)
        except ValueError:
            self.error(400, "invalid_json")
            return
        if endpoint == "subscriptions.register":
            with STATE.lock:
                created = not STATE.subscribed
                STATE.subscribed = True
                STATE.subscriptions.append({"raw": raw.decode(), "body": body, "at": now_rfc3339()})
            self.reply(201 if created else 200, {"id": "sub_e2e", "app_id": body.get("app_id"), "for": body.get("for"), "active": True, "required_delivery": False})
        elif endpoint == "subscriptions.revoke":
            with STATE.lock:
                STATE.subscribed = False
            self.reply(200, {"id": "sub_e2e", "active": False})
        else:
            with STATE.lock:
                if not STATE.subscribed:
                    self.error(403, "recipient_not_registered")
                    return
                replay = next((t for t in STATE.tings if t["body"].get("key") == body.get("key")), None)
                if replay is not None and replay["raw"] != raw.decode():
                    self.error(409, "idempotency_conflict")
                    return
                if replay is None:
                    TingHandler.counter += 1
                    record = {
                        "id": f"msg_e2e_{TingHandler.counter}",
                        "raw": raw.decode(),
                        "body": body,
                        "content_type": self.headers.get("content-type"),
                        "idempotency_key_header": self.headers.get("idempotency-key"),
                        "testing_headers": {h: self.headers.get(h) is not None for h in ("iam_test_app_secret", "x-testing-environment-key")},
                        "received_at": now_rfc3339(),
                    }
                    STATE.tings.append(record)
                    status, ting_id = 202, record["id"]
                else:
                    status, ting_id = 200, replay["id"]
            sys.stderr.write(f"[fake ting] accepted {body.get('type')} key={body.get('key')}\n")
            self.reply(status, {"id": ting_id, "created_at": now_rfc3339(), "status": "accepted", "key": body.get("key"), "silent": False})


def serve(name: str, port: int, handler: type[Handler]) -> ThreadingHTTPServer:
    httpd = ThreadingHTTPServer(("127.0.0.1", port), handler)
    httpd.daemon_threads = True
    httpd.name = name  # type: ignore[attr-defined]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--iam-port", type=int, default=8081)
    parser.add_argument("--ting-port", type=int, default=8082)
    parser.add_argument("--app-secret", help="peek's ask_ secret; enables Basic-auth and OBO signature checks")
    parser.add_argument("--app-secret-file", help="read --app-secret from a file")
    parser.add_argument("--ready-file", help="write the two ports here as JSON once listening")
    args = parser.parse_args()
    secret = args.app_secret
    if args.app_secret_file:
        with open(args.app_secret_file, encoding="utf-8") as f:
            secret = f.read().strip()
    STATE.app_secret = secret or None
    iam = serve("iam", args.iam_port, IamHandler)
    ting = serve("ting", args.ting_port, TingHandler)
    ports = {"iam": iam.server_address[1], "ting": ting.server_address[1]}
    print(f"fake iam on http://127.0.0.1:{ports['iam']}, fake ting on http://127.0.0.1:{ports['ting']}", flush=True)
    if args.ready_file:
        tmp = args.ready_file + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(ports, f)
        os.replace(tmp, args.ready_file)
    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    try:
        stop.wait()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
