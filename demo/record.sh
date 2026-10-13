#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$HOME/src"
python3 /demo/fake_agents.py &
vhs /demo/demo.tape
kill %1 2>/dev/null || true
