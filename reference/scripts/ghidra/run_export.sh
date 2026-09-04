#!/usr/bin/env bash
# Regenerate reference/decomp and reference/dumps from the Ghidra project.
# The Ghidra GUI must be closed (it holds ~/code/acclient.lock).
#
#   reference/scripts/ghidra/run_export.sh            # full export
#   reference/scripts/ghidra/run_export.sh 200        # smoke test on 200 functions
#   RTTI=1 reference/scripts/ghidra/run_export.sh     # run RTTI class recovery first (modifies the project)
#   ANALYZE=1 reference/scripts/ghidra/run_export.sh  # re-run full auto-analysis first (modifies the project;
#                                                     #   needed once because the GUI analysis ran without decompiler natives)
#
# Ghidra's public macOS build ships no native decompiler; build it once with
#   cd $GHIDRA_HOME/support/gradle && gradle buildNatives
set -euo pipefail
GHIDRA="${GHIDRA_HOME:-$HOME/Downloads/ghidra_12.1.3_PUBLIC}"
PROJECT_DIR="${GHIDRA_PROJECT_DIR:-$HOME/code}"
PROJECT="${GHIDRA_PROJECT:-acclient}"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$(cd "$HERE/../.." && pwd)"
MAX="${1:-}"
export MAXMEM="${MAXMEM:-16G}"

if pgrep -f "ghidra.GhidraClassLoader" >/dev/null; then
  echo "Ghidra GUI is running; close it first." >&2
  exit 1
fi

if [[ "${ANALYZE:-0}" == "1" ]]; then
  "$GHIDRA/support/analyzeHeadless" "$PROJECT_DIR" "$PROJECT" -process acclient.exe -max-cpu "${MAX_CPU:-8}"
fi

if [[ "${RTTI:-0}" == "1" ]]; then
  "$GHIDRA/support/analyzeHeadless" "$PROJECT_DIR" "$PROJECT" -process acclient.exe -noanalysis \
    -postScript RecoverClassesFromRTTIScript.java \
    -postScript AddVfunctionCallRefScript.java
fi

"$GHIDRA/support/analyzeHeadless" "$PROJECT_DIR" "$PROJECT" -process acclient.exe -noanalysis -readOnly \
  -max-cpu "${MAX_CPU:-8}" -scriptPath "$HERE" -postScript ExportAll.java "$OUT" ${MAX:+"$MAX"}
