#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../reference/ace-run" && docker compose logs -f --tail=100 ace-server
