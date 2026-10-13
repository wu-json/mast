---
name: record-demo
description: How to regenerate the README screenshot and demo GIF of mast (assets/screenshot.png, assets/demo.gif) with VHS in Docker against fake claude, codex and pi sessions. Use after changing the TUI's look, or when asked to update the README media.
---

# Recording the mast demo

The README media is rendered, never captured from a real desktop:

```sh
demo/render.sh
```

This builds `demo/Dockerfile` (mast compiled for Linux on top of the
`ghcr.io/charmbracelet/vhs` image) and runs it with `assets/` mounted as the
output dir. It writes `assets/demo.gif` and `assets/screenshot.png` in about a
minute. Needs a running Docker daemon (on macOS, `orbctl start` for OrbStack).

## Pieces

| file | role |
| ---- | ---- |
| `demo/fake_agents.py` | spawns `sleep` processes renamed to `claude`, `codex` and `pi` in `~/src/<project>` and writes the session files mast reads for each; waits until `mast` is running, then plays a timeline of state changes |
| `demo/demo.tape` | VHS script: window size, theme, typing `mast`, when to take the screenshot, when to quit |
| `demo/shell.sh` | the prompt shown in the recording, sourced while hidden |
| `demo/record.sh` | container entrypoint: starts the fake agents, runs VHS |

Everything runs inside the container with `HOME=/home/dev`, so no real agent,
path, username or session from the host can appear in the output.

## Changing the story

The timeline in `fake_agents.py` `main()` is in seconds after mast starts
(`at(6)` etc.). The screenshot is taken at `Sleep 11s` after `mast` is typed
in `demo.tape`; keep it after the last change you want visible in the still
(currently: one needs you, one working, four idle). If you lengthen the
timeline, lengthen the tape's final `Sleep` before `Type "q"` too.

Session file formats the fakes must match are listed in
[`../test-mast/SKILL.md`](../test-mast/SKILL.md#where-each-state-comes-from); if
mast's detection changes, update the fakes to match.

## Checking the result

Look at the output before committing. Pull single frames from the GIF:

```sh
ffmpeg -loglevel error -y -ss 12 -i assets/demo.gif -frames:v 1 /tmp/frame.png
```

Check that every row fits (the header counts must match the visible rows; if
not, raise `Set Height`), that the first frame shows only the prompt, and that
the screenshot shows all three states.
