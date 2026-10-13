# mast

A small terminal UI that shows every coding agent running on your machine and
what each one is doing.

![mast showing claude, codex and pi agents and what each is doing](assets/screenshot.png)

Supports [Claude Code](https://claude.com/claude-code), [Codex](https://github.com/openai/codex)
and [pi](https://github.com/badlogic/pi-mono), in any terminal, tmux pane or IDE.

## Demo

![mast picking up agents as they start, finish, stop to ask for permission and exit](assets/demo.gif)

## Install

Download a binary for macOS or Linux (x86_64 or arm64) from
[Releases](https://github.com/wu-json/mast/releases), or build from source:

```sh
cargo install --git https://github.com/wu-json/mast
```

## Usage

```sh
mast          # live view, q to quit
mast --once   # tab-separated snapshot: agent, status, time in status, pid, cwd, title
```

The live view also sets the terminal title, so a mast tab in WezTerm, iTerm2,
tmux and so on reads like `⛵ mast · 1 needs you · 2 working`. When mast exits, the
previous title comes back.

## How it works

mast finds agent processes with `ps` (the same process-name matching
[herdr](https://herdr.dev) uses), then reads the session state each agent
already writes to disk:

- **claude** — `~/.claude/sessions/<pid>.json` has a live `busy` / `waiting` / `idle` status,
  and the session transcript has the AI-generated title claude shows in your terminal's title bar.
- **codex** — the session rollout in `~/.codex/sessions/` logs when each turn starts and ends;
  the thread name comes from `~/.codex/session_index.jsonl`.
- **pi** — the session log in `~/.pi/agent/sessions/` shows whether a turn is still in flight.

Nothing needs to be installed into the agents, and it only reads files.
`CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `PI_CODING_AGENT_DIR` are respected.

## Name

A [mast](https://en.wikipedia.org/wiki/Mast_(sailing)) holds the crow's nest:
the lookout where you watch over everything at once.

## Testing

See [`.agents/skills/test-mast/SKILL.md`](.agents/skills/test-mast/SKILL.md).

## Releasing

Run the [Release](https://github.com/wu-json/mast/actions/workflows/release.yml) workflow
and pick patch, minor or major. It bumps the version in `Cargo.toml`, tags it, builds
each platform on its own runner, signs and notarizes the macOS binaries, and publishes
a GitHub release. Tick `dry_run` to build and sign without pushing or publishing.

## Recording the demo

The screenshot and GIF above are rendered with [VHS](https://github.com/charmbracelet/vhs)
in Docker against fake agent sessions, so nothing from your machine ends up in them:

```sh
demo/render.sh
```

See [`.agents/skills/record-demo/SKILL.md`](.agents/skills/record-demo/SKILL.md).
