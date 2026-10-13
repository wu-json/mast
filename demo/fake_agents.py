#!/usr/bin/env python3

import json
import os
import subprocess
import time
import uuid
from datetime import datetime, timedelta, timezone

HOME = os.environ["HOME"]
SRC = os.path.join(HOME, "src")
CLAUDE = os.path.join(HOME, ".claude")
CODEX = os.path.join(HOME, ".codex")
PI = os.path.join(HOME, ".pi", "agent")


def iso(ago: float = 0) -> str:
    t = datetime.now(timezone.utc) - timedelta(seconds=ago)
    return t.strftime("%Y-%m-%dT%H:%M:%S.") + f"{t.microsecond // 1000:03d}Z"


def write_json(path: str, value: dict) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(value, f)


def append_jsonl(path: str, *rows: dict) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "a") as f:
        for row in rows:
            f.write(json.dumps(row) + "\n")


def spawn(name: str, project: str) -> subprocess.Popen:
    cwd = os.path.join(SRC, project)
    os.makedirs(cwd, exist_ok=True)
    return subprocess.Popen([name, "infinity"], executable="/usr/bin/sleep", cwd=cwd)


class Claude:
    def __init__(self, project: str, title: str, status: str, ago: float):
        self.proc = spawn("claude", project)
        self.cwd = os.path.join(SRC, project)
        self.session = str(uuid.uuid4())
        slug = "".join(c if c.isalnum() else "-" for c in self.cwd)
        append_jsonl(
            os.path.join(CLAUDE, "projects", slug, f"{self.session}.jsonl"),
            {"type": "ai-title", "aiTitle": title, "sessionId": self.session},
        )
        self.set(status, ago)

    def set(self, status: str, ago: float = 0) -> None:
        ms = int((time.time() - ago) * 1000)
        write_json(
            os.path.join(CLAUDE, "sessions", f"{self.proc.pid}.json"),
            {
                "pid": self.proc.pid,
                "sessionId": self.session,
                "cwd": self.cwd,
                "status": status,
                "statusUpdatedAt": ms,
                "updatedAt": ms,
            },
        )


class Codex:
    def __init__(self, project: str, title: str, working: bool, ago: float):
        self.proc = spawn("codex", project)
        session = str(uuid.uuid4())
        day = datetime.now(timezone.utc).strftime("%Y/%m/%d")
        self.rollout = os.path.join(CODEX, "sessions", day, f"rollout-{session}.jsonl")
        append_jsonl(
            self.rollout,
            {"type": "session_meta", "payload": {"id": session, "cwd": os.path.join(SRC, project)}},
        )
        append_jsonl(os.path.join(CODEX, "session_index.jsonl"), {"id": session, "thread_name": title})
        self.event("task_started", ago + (0 if working else 90))
        if not working:
            self.event("task_complete", ago)

    def event(self, kind: str, ago: float = 0) -> None:
        append_jsonl(self.rollout, {"timestamp": iso(ago), "type": "event_msg", "payload": {"type": kind}})


class Pi:
    def __init__(self, project: str, title: str, working: bool, ago: float):
        self.proc = spawn("pi", project)
        cwd = os.path.join(SRC, project)
        slug = "--" + cwd.strip("/").replace("/", "-") + "--"
        self.log = os.path.join(PI, "sessions", slug, f"{uuid.uuid4()}.jsonl")
        self.message("user", None, ago + 60, title)
        if not working:
            self.message("assistant", "stop", ago)

    def message(self, role: str, stop, ago: float = 0, text: str = "ok") -> None:
        msg = {"role": role, "content": [{"type": "text", "text": text}]}
        if stop:
            msg["stopReason"] = stop
        append_jsonl(self.log, {"type": "message", "timestamp": iso(ago), "message": msg})


def wait_for_mast() -> None:
    while subprocess.run(["pgrep", "-x", "mast"], capture_output=True).returncode != 0:
        time.sleep(0.1)


def main() -> None:
    billing = Claude("billing", "Migrate billing to Stripe v2 API", "busy", 8 * 60 + 12)
    uploads = Codex("web", "Add retry with backoff to uploads", True, 3 * 60 + 40)
    auth = Claude("api", "Fix flaky auth token refresh test", "idle", 12 * 60 + 5)
    infra = Codex("infra", "Bump terraform providers", False, 41 * 60)
    docs = Pi("docs", "write the getting started guide", False, 2 * 3600 + 300)

    wait_for_mast()
    start = time.time()

    def at(seconds: float) -> None:
        time.sleep(max(0, start + seconds - time.time()))

    at(3.5)
    uploads.event("task_complete")
    at(6)
    billing.set("waiting")
    at(9)
    cli = Claude("cli", "Add --json flag to export command", "busy", 0)
    at(13)
    auth.set("busy")
    at(16)
    billing.set("busy")
    at(19)
    docs.proc.kill()
    docs.proc.wait()
    at(60)
    for p in (billing, uploads, auth, infra, docs, cli):
        p.proc.kill()


if __name__ == "__main__":
    main()
