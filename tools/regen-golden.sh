#!/usr/bin/env bash
# Regenerate tests/golden/dat from ACE's DatLoader (the reference implementation).
# Requires: dotnet SDK, reference/ext/ACE cloned, AC_DATA_DIR set.
set -euo pipefail
: "${AC_DATA_DIR:?set AC_DATA_DIR to the directory holding client_*.dat}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOL="$ROOT/reference/tools/AceDump"
TMP="$(mktemp -d)"
dotnet build -c Release -v q --nologo "$TOOL" >/dev/null
for d in portal cell_1; do
  dotnet "$TOOL/bin/Release/net8.0/AceDump.dll" manifest "$AC_DATA_DIR/client_$d.dat" > "$TMP/$d.tsv"
  tail -n +2 "$TMP/$d.tsv" | shasum -a 256 | cut -d' ' -f1 > "$ROOT/tests/golden/dat/${d}_manifest.sha256"
  { head -1 "$TMP/$d.tsv"; tail -n +2 "$TMP/$d.tsv" | awk 'NR%97==1'; } > "$ROOT/tests/golden/dat/${d}_sample.tsv"
done
echo "golden data regenerated in tests/golden/dat"
