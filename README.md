# mast

A small terminal UI that shows every coding agent running on your machine and
what each one is doing.

```
  ⛵ mast                                             1 working  ·  2 idle

  ⠋  claude  Migrate billing to v2                        working     8m
  ●  codex   Add retry to uploads                         idle       11m
  ●  pi      write the readme                             idle        2h
```

Supports [Claude Code](https://claude.com/claude-code), [Codex](https://github.com/openai/codex)
and [pi](https://github.com/badlogic/pi-mono), in any terminal, tmux pane or IDE.

## Install

```sh
cargo install --path .
```

## Usage

```sh
mast          # live view, q to quit
mast --once   # tab-separated snapshot: agent, status, time in status, pid, cwd, title
```

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
