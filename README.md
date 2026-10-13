# mast

A terminal UI that shows every coding agent running on your machine and what
each one is doing.

![mast showing claude, codex and pi agents and what each is doing](assets/screenshot.png)

Works with [Claude Code](https://claude.com/claude-code), [Codex](https://github.com/openai/codex)
and [pi](https://github.com/badlogic/pi-mono), wherever they run: any terminal, tmux pane or IDE.

![mast picking up agents as they start, finish, stop to ask for permission and exit](assets/demo.gif)

## Install

With [Homebrew](https://brew.sh) on macOS or Linux:

```sh
brew install wu-json/asahi/mast
```

Or grab a binary from [Releases](https://github.com/wu-json/mast/releases),
or build from source:

```sh
cargo install --git https://github.com/wu-json/mast
```

## Usage

```sh
mast          # live view, q to quit
mast --once   # print a tab-separated snapshot and exit
```

While it runs, mast also sets your terminal's tab title, e.g. `⛵ mast · 1 needs you · 2 working`.

## How it works

mast finds agents in the process list (using the same name matching as
[herdr](https://herdr.dev)), then reads the session files each one already
writes under `~/.claude`, `~/.codex` and `~/.pi` to tell whether it's working,
idle or waiting on you.

It only reads files, so there's nothing to install into the agents. Custom
config directories set with `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or
`PI_CODING_AGENT_DIR` are picked up.

## Name

A [mast](https://en.wikipedia.org/wiki/Mast_(sailing)) holds the
[crow's nest](https://en.wikipedia.org/wiki/Crow%27s_nest): the lookout where
you watch over everything at once.

<img src="assets/crows-nest.jpg" alt="A crow's nest on a tug boat's mast" width="240">

I thought about calling it crow's nest, but mast is easier to type.

<sub>Photo: [Crows nest on a tug boat](https://commons.wikimedia.org/wiki/File:Crows_nest_on_a_tug_boat_-_Flickr_-_p_a_h.jpg)
by [Paul](https://www.flickr.com/people/64654599@N00), [CC BY 2.0](https://creativecommons.org/licenses/by/2.0/).</sub>
