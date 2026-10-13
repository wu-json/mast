---
name: test-mast
description: How to test mast — unit/render tests, plus driving real claude, codex and pi sessions in a pseudo-terminal to verify mast detects their idle, working and needs-you states. Use after changing process discovery, state detection, or the TUI.
---

# Testing mast

mast has two layers to test: pure logic and rendering (fast, offline), and
detection against real agents (slow, needs the agent CLIs installed and
logged in).

## 1. Unit and render tests

```sh
cargo test
```

- `process::tests` — which command lines count as an agent, `ps` etime parsing.
- `agents::tests` — timestamp parsing, pi's session-dir slug, title extraction.
- `ui::tests` — renders the TUI into a `TestBackend` buffer and asserts on
  the text. Run `cargo test renders_agents -- --nocapture` to print the frame
  and eyeball layout changes. Update these assertions when the layout changes.

## 2. Snapshot what is running now

```sh
cargo build && ./target/debug/mast --once
```

Prints one tab-separated row per agent: `agent  status  time-in-status  pid  cwd  title`.
Compare against the agents you know are running. Never launch the full TUI
(`mast` with no args) from a non-interactive shell — it needs a real
terminal and blocks until `q`.

## 3. Drive a live agent end to end

`scripts/agent_pty.py` starts an agent in a pseudo-terminal (no tmux
needed), answers the terminal queries agent TUIs block on at startup, types
a prompt, and prints the matching `mast --once` row every 5 seconds:

```sh
cargo build
python3 scripts/agent_pty.py -- codex
python3 scripts/agent_pty.py -- pi
python3 scripts/agent_pty.py --keys '\x1b[B\r' -- claude          # accept the folder trust dialog
python3 scripts/agent_pty.py --keys '\x1b[B\r' \
  --prompt "run the shell command: touch /tmp/mast-perm-test" \
  -- claude --permission-mode default                            # exercise "needs you"
```

Expected rows: `idle` after startup, `working` right after the prompt, then
`idle` once the reply lands (or `needs you` while claude waits on the
permission prompt). Each run uses a fresh temp dir unless `--cwd` is given.

Useful flags: `--wait SECS` (how long to watch), `--startup SECS` (boot
time before typing), `--keys` (escaped keystrokes sent before the prompt),
`--screen` (print the tail of the agent's screen — use this first whenever a
run looks wrong; it shows trust dialogs, login prompts, or errors).

Notes:

- Each run sends a real prompt and costs a small request on that agent's account.
- The script strips `CLAUDECODE`, `CLAUDE_CODE_*`, `CODEX_THREAD*` and `HERDR_*`
  from the child env. A claude started inside another claude otherwise counts
  as a nested session, never writes `~/.claude/sessions/<pid>.json`, and shows
  as `unknown`.
- The agent is killed when the script exits; nothing is left running.
- Codex has no on-disk signal for approval prompts, so it only reports
  `working` / `idle`.

## Where each state comes from

| agent  | source | states |
| ------ | ------ | ------ |
| claude | `~/.claude/sessions/<pid>.json` `status`: `busy` / `waiting` / `idle`; title from the last `custom-title` or `ai-title` entry in `~/.claude/projects/<cwd>/<sessionId>.jsonl` | working / needs you / idle |
| codex  | latest `task_started` vs `task_complete` / `turn_aborted` in the rollout under `~/.codex/sessions/` | working / idle |
| pi     | last `message` in `~/.pi/agent/sessions/--<cwd>--/*.jsonl`; `user`, `toolResult` or assistant `toolUse` means a turn is in flight | working / idle |

If an agent changes its on-disk format, reproduce with step 3, inspect the
newest session file, and update `src/agents.rs`.
