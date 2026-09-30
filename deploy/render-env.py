#!/usr/bin/env python3
"""Validate peek-server's runtime JSON and render it as a systemd EnvironmentFile.

The Secrets Manager secret silicon-peek/production/runtime holds one JSON object of strings
(keys: deploy/runtime.json.example, meaning: BLUEPRINT §5.2). deploy/install.sh pipes it here on
the host; operators run --check locally before `aws secretsmanager put-secret-value`:

  python3 deploy/render-env.py --check < ~/.peek-operator/runtime.json
  aws secretsmanager get-secret-value … --query SecretString --output text \
    | python3 render-env.py --output /etc/peek/runtime.env

Runs on the host's /usr/bin/python3 (Amazon Linux 2023: Python 3.9), so it stays 3.9-compatible.
Values are read from stdin (never argv), validated per key, and written atomically with mode
0600. No value is ever printed: reports name keys only. Unknown PEEK_* keys are rendered with a
warning (newer servers may read them); any other unknown key is refused.

Optional keys may be "" (not configured). PEEK_HONEYCOMB_SERVICE_TOKEN may be empty too: the
server then answers 503 on the Honeycomb lifecycle participant routes (no testing environments),
and this tool warns. PEEK_DEEPGRAM_MIP_OPT_OUT remains "true" for legacy key validation.
PEEK_BYO_DEEPGRAM_HOSTS is optional and may even be left out (secrets written before it existed):
"" or absent keeps the default *.deepgram.com for org BYO base URLs.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path
from urllib.parse import urlsplit

DATA_DIR = "/var/lib/peek/"


class ConfigError(Exception):
    pass


def _https_origin(value: str) -> str | None:
    parts = urlsplit(value)
    if parts.scheme != "https" or not parts.hostname or parts.username or parts.password:
        return "must be an https:// origin without credentials"
    if parts.path not in ("", "/") or parts.query or parts.fragment or value.endswith("/"):
        return "must be an origin (scheme and host) without a path, query or trailing slash"
    return None


def _wss_url(value: str) -> str | None:
    if value != "wss://agent.deepgram.com/v1/agent/converse":
        return "must be wss://agent.deepgram.com/v1/agent/converse"
    return None


def _origins(value: str) -> str | None:
    for origin in value.split(","):
        problem = _https_origin(origin.strip())
        if problem:
            return f"entry {origin.strip()!r} {problem}"
    return None


def _under_data_dir(value: str) -> str | None:
    if not value.startswith(DATA_DIR) or "/../" in value or value.endswith("/.."):
        return f"must be a path under {DATA_DIR} (the only directory peek-server may write: ReadWritePaths)"
    return None


def _int_range(low: int, high: int) -> Callable[[str], str | None]:
    def check(value: str) -> str | None:
        if not re.fullmatch(r"[0-9]+", value) or not low <= int(value) <= high:
            return f"must be an integer from {low} to {high}"
        return None

    return check


def _one_of(*allowed: str) -> Callable[[str], str | None]:
    return lambda value: None if value in allowed else f"must be one of {', '.join(allowed)}"


def _pattern(regex: str, description: str) -> Callable[[str], str | None]:
    return lambda value: None if re.fullmatch(regex, value) else f"must be {description}"


HEX64 = _pattern(r"[0-9a-f]{64}", "64 lowercase hex characters (openssl rand -hex 32)")


def _host_name(host: str) -> bool:
    labels = host.split(".")
    return (
        len(host) <= 253
        and len(labels) >= 2
        and all(re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label) for label in labels)
        # A dotted-decimal "host" is an IP address, not a name.
        and not all(label.isdigit() for label in labels)
    )


def _byo_hosts(value: str) -> str | None:
    """PEEK_BYO_DEEPGRAM_HOSTS, as src/config.rs ByoHosts::parse reads it."""
    entries = [e.strip().lower() for e in value.split(",") if e.strip()]
    if not entries:
        return "must list at least one host (leave it empty for the default *.deepgram.com)"
    for entry in entries:
        if entry == "*":
            continue
        if entry.startswith("*."):
            if not _host_name(entry[2:]):
                return f"entry {entry!r} must be `*.` followed by a domain such as deepgram.com"
            continue
        if not _host_name(entry) or entry.endswith(".localhost"):
            return (
                f"entry {entry!r} must be a host name (such as api.deepgram.com), `*.domain` or `*`; "
                "IP addresses and localhost are never allowed"
            )
    return None


# key -> (required non-empty?, is secret?, validator for non-empty values)
SPEC: dict[str, tuple[bool, bool, Callable[[str], str | None]]] = {
    "PEEK_BIND": (True, False, _one_of("127.0.0.1:8080")),
    "PEEK_DATABASE_PATH": (True, False, _under_data_dir),
    "PEEK_TEST_DATABASE_PATH": (True, False, _under_data_dir),
    "PEEK_PUBLIC_ORIGIN": (True, False, _https_origin),
    "PEEK_WEB_ORIGINS": (True, False, _origins),
    "PEEK_IAM_BASE_URL": (True, False, _https_origin),
    "PEEK_IAM_APP_ID": (True, False, _one_of("peek")),
    # Empty until `honeycomb apps create` (P5); /readyz then reports iam_config: missing.
    "PEEK_IAM_APP_SECRET": (False, True, _pattern(r"ask_[A-Za-z0-9_-]{16,}", "an IAM app secret starting with ask_")),
    "PEEK_IAM_WEBHOOK_SECRET": (True, True, HEX64),
    "PEEK_IAM_WEBHOOK_KEY_VERSION": (True, False, _int_range(1, 1_000_000)),
    "PEEK_IAM_REQUEST_TIMEOUT_SECONDS": (True, False, _int_range(1, 120)),
    "PEEK_TING_BASE_URL": (True, False, _https_origin),
    "PEEK_TING_REQUEST_TIMEOUT_SECONDS": (True, False, _int_range(1, 120)),
    "PEEK_HONEYCOMB_URL": (True, False, _https_origin),
    # Optional: without it the lifecycle participant routes answer 503 (warned about below).
    "PEEK_HONEYCOMB_SERVICE_TOKEN": (False, True, _pattern(r"[\x21-\x7e]{32,512}", "32-512 visible ASCII characters")),
    "PEEK_ENCRYPTION_KEY": (True, True, HEX64),
    # Current speech providers; production and testing use separate credentials.
    "PEEK_ELEVENLABS_AGENT_URL": (False, False, _wss_url),
    "PEEK_OPENAI_API_KEY": (False, True, _pattern(r"[\x21-\x7e]{16,512}", "an OpenAI API key")),
    "PEEK_OPENAI_TEST_API_KEY": (False, True, _pattern(r"[\x21-\x7e]{16,512}", "an OpenAI API key")),
    "PEEK_OPENAI_BASE_URL": (False, False, _https_origin),
    # Deepgram mints direct ElevenLabs TTS connection tokens; org BYO keys remain inactive.
    "PEEK_DEEPGRAM_API_KEY": (False, True, _pattern(r"[\x21-\x7e]{16,512}", "a Deepgram API key")),
    "PEEK_DEEPGRAM_TEST_API_KEY": (False, True, _pattern(r"[\x21-\x7e]{16,512}", "a Deepgram API key")),
    "PEEK_DEEPGRAM_BASE_URL": (True, False, _https_origin),
    "PEEK_DEEPGRAM_TOKEN_TTL_SECONDS": (True, False, _int_range(1, 3600)),
    "PEEK_DEEPGRAM_MIP_OPT_OUT": (True, False, _one_of("true")),
    # Optional: hosts an org's own Deepgram key may name as its --base-url ("" or absent → *.deepgram.com).
    "PEEK_BYO_DEEPGRAM_HOSTS": (False, False, _byo_hosts),
    "PEEK_GITHUB_ISSUES_TOKEN": (False, True, _pattern(r"[\x21-\x7e]{20,512}", "a fine-grained GitHub token")),
    "PEEK_GITHUB_REPO": (True, False, _one_of("teamofsilicons/silicon-peek")),
    "PEEK_TELEMETRY": (True, False, _one_of("on", "off")),
    # Space Station table keys arrive in P6.
    "PEEK_BACKEND_TABLE_KEY": (False, True, _pattern(r"table-peekbackend-[0-9a-f]{32}", "table-peekbackend-<32 hex>")),
    "PEEK_CLIDAEMON_TABLE_KEY": (
        False,
        True,
        _pattern(r"table-peekclidaemon-[0-9a-f]{32}", "table-peekclidaemon-<32 hex>"),
    ),
    "PEEK_FRONTEND_ANALYTICS_TABLE_KEY": (
        False,
        True,
        _pattern(r"table-peekfrontendanalytics-[0-9a-f]{32}", "table-peekfrontendanalytics-<32 hex>"),
    ),
    "PEEK_FRONTEND_EVENTS_TABLE_KEY": (
        False,
        True,
        _pattern(r"table-peekfrontendevents-[0-9a-f]{32}", "table-peekfrontendevents-<32 hex>"),
    ),
    "PEEK_TELEMETRY_HOME": (True, False, _under_data_dir),
    "PEEK_TELEMETRY_URL": (True, False, _https_origin),
    "RUST_LOG": (True, False, _pattern(r"[A-Za-z0-9_=,.:\-]+", "a tracing filter such as info,peek=info")),
    # Optional additions of the P0 server (all may be ""):
    "PEEK_ENVIRONMENT": (False, False, _one_of("production", "development")),
    # Leave "" in production (the IAM SDK then follows PEEK_TELEMETRY).
    "PEEK_IAM_SDK_TELEMETRY": (False, False, _one_of("on", "off")),
    # The rotation overlap (BLUEPRINT §2.10): both or neither.
    "PEEK_IAM_WEBHOOK_PREVIOUS_SECRET": (False, True, HEX64),
    "PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION": (False, False, _int_range(1, 1_000_000)),
    "PEEK_GITHUB_API_URL": (False, False, _https_origin),
}

# Keys that may be empty but deserve a warning when they are.
# Optional keys added after the first deploy: a runtime secret written before them may leave them
# out entirely (the server then uses its default), so an existing secret keeps rendering.
MAY_BE_ABSENT = frozenset({
    "PEEK_BYO_DEEPGRAM_HOSTS",
    "PEEK_ELEVENLABS_AGENT_URL",
    "PEEK_OPENAI_API_KEY", "PEEK_OPENAI_TEST_API_KEY", "PEEK_OPENAI_BASE_URL",
})

EMPTY_WARNINGS: dict[str, str] = {
    "PEEK_HONEYCOMB_SERVICE_TOKEN": (
        "must not be empty for testing environments; the Honeycomb lifecycle participant routes answer 503 "
        "until it is set"
    ),
}


def quote(value: str) -> str:
    """systemd EnvironmentFile double-quoted value: escape backslash and double quote."""
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def validate(document: object) -> tuple[dict[str, str], list[str], list[str]]:
    """Return (values, empty optional secrets, warnings) or raise ConfigError listing every problem."""
    if not isinstance(document, dict):
        raise ConfigError('the runtime secret must be one JSON object of "KEY": "string" pairs')
    problems: list[str] = []
    warnings: list[str] = []
    values: dict[str, str] = {}
    for key, value in document.items():
        if key not in SPEC and not key.startswith("PEEK_"):
            problems.append(
                f"{key}: unknown key; peek-server reads only PEEK_* and RUST_LOG (see deploy/runtime.json.example)"
            )
            continue
        if not isinstance(value, str):
            problems.append(f'{key}: must be a JSON string (write "60", not 60)')
            continue
        if any(character in value for character in "\n\r\0"):
            problems.append(f"{key}: contains a newline or NUL, which an EnvironmentFile cannot carry")
            continue
        if value != value.strip():
            problems.append(f"{key}: has leading or trailing whitespace")
            continue
        if key not in SPEC:
            warnings.append(f"{key}: not known to this deploy tool; rendered unchecked")
        values[key] = value
    empty_optional = []
    for key, (required, secret, check) in SPEC.items():
        value = values.get(key)
        if value is None and key in MAY_BE_ABSENT:
            continue
        if value is None:
            problems.append(f'{key}: missing (use "" for an optional value that is not configured yet)')
        elif not value:
            if required:
                problems.append(f"{key}: must not be empty")
            elif secret:
                empty_optional.append(key)
            if key in EMPTY_WARNINGS:
                warnings.append(f"{key}: {EMPTY_WARNINGS[key]}")
        else:
            problem = check(value)
            if problem:
                problems.append(f"{key}: {problem}")
    previous_secret = values.get("PEEK_IAM_WEBHOOK_PREVIOUS_SECRET", "")
    previous_version = values.get("PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION", "")
    if previous_secret and not previous_version:
        problems.append(
            "PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION: is required when PEEK_IAM_WEBHOOK_PREVIOUS_SECRET is set"
        )
    if previous_version and previous_version == values.get("PEEK_IAM_WEBHOOK_KEY_VERSION"):
        problems.append("PEEK_IAM_WEBHOOK_PREVIOUS_KEY_VERSION: must differ from PEEK_IAM_WEBHOOK_KEY_VERSION")
    if problems:
        message = "invalid runtime configuration:\n  " + "\n  ".join(problems)
        if warnings:
            message += "\nwarnings:\n  " + "\n  ".join(warnings)
        raise ConfigError(message)
    return values, empty_optional, warnings


def render(values: dict[str, str]) -> str:
    header = "# Rendered by deploy/render-env.py from Secrets Manager silicon-peek/production/runtime. Do not edit.\n"
    return header + "".join(f"{key}={quote(value)}\n" for key, value in values.items())


def write_atomically(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--output", type=Path, help="EnvironmentFile to write (0600), e.g. /etc/peek/runtime.env")
    target.add_argument("--check", action="store_true", help="validate stdin only; print key names, never values")
    args = parser.parse_args(argv)
    raw = sys.stdin.read()
    try:
        try:
            document = json.loads(raw, object_pairs_hook=_no_duplicates)
        except json.JSONDecodeError as error:
            raise ConfigError(f"stdin is not JSON: {error.msg} at line {error.lineno} column {error.colno}") from error
        values, empty_optional, warnings = validate(document)
    except ConfigError as error:
        print(f"render-env: error: {error}", file=sys.stderr)
        return 1
    for warning in warnings:
        print(f"render-env: warning: {warning}", file=sys.stderr)
    if empty_optional:
        print(f"render-env: not configured yet: {', '.join(empty_optional)}", file=sys.stderr)
    if args.check:
        print(f"render-env: {len(values)} keys valid")
        return 0
    write_atomically(args.output, render(values))
    print(f"render-env: wrote {args.output} ({len(values)} keys, mode 0600)", file=sys.stderr)
    return 0


def _no_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
    seen: dict[str, object] = {}
    for key, value in pairs:
        if key in seen:
            raise ConfigError(f"duplicate key {key}")
        seen[key] = value
    return seen


if __name__ == "__main__":
    sys.exit(main())
