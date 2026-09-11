#!/bin/sh
# Run the live acceptance suite against the local ACE server and report.
#
# One login does every check: an account may hold one session at a time
# and the server makes a fresh login wait about eighty seconds, so a
# check per login would take all morning.
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="$here/../target/release/acswarm"
host=${ACSWARM_HOST:-127.0.0.1}
acct=${ACSWARM_ACCOUNT:-acreborn5}
char=${ACSWARM_CHARACTER:-+Caius}
pass=${ACSWARM_PASSWORD:-testpass}
out=${1:-$here/last.log}

# The suite reads its settings from its own directory, so it needs to be
# told where the DATs are the same way the app is.
[ -f "$here/data-dir" ] || cp "${XDG_CONFIG_HOME:-$HOME/.config}/acswarm/data-dir" "$here/data-dir"

ACSWARM_SCRIPTS="$here" ACSWARM_CONFIG_DIR="$here" \
  "$bin" --headless --connect "$host" --client "$acct:$pass:$char" \
  --script "$here/lines.txt" --duration "${ACSWARM_DURATION:-60}" \
  > "$out" 2>&1 || true

echo
grep -h "CHECK " "$out" | sed 's/^.*CHECK /  /' | sort -u
echo
p=$(grep -hc " PASS " "$out" || true)
f=$(grep -hc " FAIL " "$out" || true)
echo "  $p passed, $f failed   ($out)"
[ "$f" = "0" ]
