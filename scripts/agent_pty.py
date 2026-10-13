#!/usr/bin/env python3
"""Drive a real coding agent in a pseudo-terminal and snapshot `mast --once`.

Lets an agent (or a human) watch mast's state detection change as a live
claude / codex / pi session goes idle -> working -> idle, without tmux.

usage: scripts/agent_pty.py [--cwd DIR] [--prompt TEXT] [--wait SECS] -- AGENT [ARGS...]
"""

import argparse
import os
import pty
import re
import select
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAST = os.path.join(ROOT, "target", "debug", "mast")

# Agent TUIs block on startup until the terminal answers these queries.
REPLIES = [
    (b"\x1b[6n", b"\x1b[1;1R"),  # cursor position
    (b"\x1b[c", b"\x1b[?62c"),  # primary device attributes
    (b"\x1b]10;?", b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\"),  # foreground color
    (b"\x1b]11;?", b"\x1b]11;rgb:1111/1111/1111\x1b\\"),  # background color
]


def screen_text(raw: bytes) -> str:
    text = re.sub(rb"\x1b\[[0-9;?<>=]*[a-zA-Z]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[=>()][A-Z0-9]?", b" ", raw)
    return re.sub(r"\s+", " ", text.decode("utf-8", "replace")).strip()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cwd", help="directory to run the agent in (default: fresh temp dir)")
    ap.add_argument("--prompt", default="reply with just the word hi")
    ap.add_argument("--wait", type=float, default=45, help="seconds to let the turn run")
    ap.add_argument("--startup", type=float, default=8, help="seconds to let the agent boot")
    ap.add_argument("--keys", default="", help="keys to send before the prompt, with escapes (e.g. '\\x1b[B\\r' to accept a trust dialog)")
    ap.add_argument("--screen", action="store_true", help="print the agent's last screen text")
    ap.add_argument("agent", nargs="+")
    args = ap.parse_args()

    if not os.path.exists(MAST):
        sys.exit("build first: cargo build")
    cwd = args.cwd or tempfile.mkdtemp(prefix="mast-test-")

    pid, fd = pty.fork()
    if pid == 0:
        # When run from inside an agent, drop the markers that make the child
        # think it is a nested session (nested claude skips its session file).
        for key in list(os.environ):
            if key.startswith(("CLAUDECODE", "CLAUDE_CODE_", "CODEX_THREAD", "HERDR_")):
                del os.environ[key]
        os.chdir(cwd)
        os.execvp(args.agent[0], args.agent)

    screen = bytearray()

    def pump(seconds: float) -> None:
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.1)
            if not ready:
                continue
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                return
            screen.extend(chunk)
            del screen[:-200_000]
            for query, reply in REPLIES:
                if query in chunk:
                    os.write(fd, reply)

    def family() -> set:
        # Launchers often re-exec or fork the real agent, so match the whole tree.
        rows = subprocess.run(["ps", "-axo", "pid=,ppid="], capture_output=True, text=True).stdout
        children = {}
        for row in rows.split("\n"):
            if row.strip():
                child, parent = map(int, row.split())
                children.setdefault(parent, []).append(child)
        tree, todo = set(), [pid]
        while todo:
            p = todo.pop()
            tree.add(p)
            todo.extend(children.get(p, []))
        return tree

    def snapshot(label: str) -> None:
        out = subprocess.run([MAST, "--once"], capture_output=True, text=True).stdout
        tree = family()
        mine = [l for l in out.splitlines() if int(l.split("\t")[3]) in tree]
        print(f"[{label}] " + (mine[0].replace("\t", "  ") if mine else f"pid {pid} not detected"), flush=True)

    try:
        pump(args.startup)
        if args.keys:
            os.write(fd, args.keys.encode().decode("unicode_escape").encode())
            pump(args.startup)
        snapshot("started")
        os.write(fd, args.prompt.encode())
        pump(1)
        os.write(fd, b"\r")
        pump(3)
        snapshot("prompted")
        end = time.time() + args.wait
        while time.time() < end:
            pump(5)
            snapshot(f"+{int(args.wait - (end - time.time()))}s")
        if args.screen:
            print("--- screen tail\n" + screen_text(bytes(screen))[-1500:])
    finally:
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass  # the agent already exited
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
