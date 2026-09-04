#!/usr/bin/env bash
# Run a local ACE server in Docker for testing the client.
#
#   tools/ace/up.sh          # build (first time: ~10 min + world DB download) and start
#   tools/ace/logs.sh        # follow server logs
#   tools/ace/down.sh        # stop
#
# Requires Docker Desktop running, reference/ext/ACE cloned, and AC_DATA_DIR
# pointing at the DAT files (they are bind-mounted read-only into the server).
set -euo pipefail
: "${AC_DATA_DIR:?set AC_DATA_DIR to the directory holding client_*.dat}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ACE="$ROOT/reference/ext/ACE"
RUN="$ROOT/reference/ace-run"
mkdir -p "$RUN"/{Config,Content,Logs,Mods,db-data}
# Dats: symlink the real directory (ACE only reads them).
ln -sfn "$AC_DATA_DIR" "$RUN/Dats"
cp "$ACE/docker-compose.yml" "$RUN/docker-compose.yml"
cp "$ACE/docker.env" "$RUN/docker.env"
# Build context is the ACE checkout.
sed -i '' "s|build: \.|build: $ACE|" "$RUN/docker-compose.yml"
# Allow the world to be entered by the first account without manual admin setup.
grep -q ACE_NONINTERACTIVE_SETUP "$RUN/docker.env" || echo "ACE_NONINTERACTIVE_SETUP=true" >> "$RUN/docker.env"
cd "$RUN"
docker compose up -d --build
echo "ACE starting. Login port udp/9000. Follow with: tools/ace/logs.sh"
echo "First account to log in is auto-created (AllowAutoAccountCreation) and promoted to admin."
