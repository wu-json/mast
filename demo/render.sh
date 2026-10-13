#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
docker build -f "$root/demo/Dockerfile" -t mast-demo "$root"
docker run --rm -v "$root/assets:/out" mast-demo
