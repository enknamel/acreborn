#!/bin/sh
# Regenerate crates/ac-world/data/properties.csv: the name of every
# property the server can send when it identifies an item.
#
# A loot rule should be able to ask about anything the server says, not
# only the dozen fields a client happens to have a field for. The
# numbers are the wire's; the names are what a person reads in the
# rule editor, and they come from the enums the emulator and the
# community share (read, never copied: only the number and the name of
# each constant is taken).
#
# Usage: reference/scripts/data/properties.sh > crates/ac-world/data/properties.csv
set -e
SRC=reference/ext/ACE/Source/ACE.Entity/Enum/Properties

echo "# Every property an identify can carry: what to call it, by kind."
echo "# kind,id,name"
for kind in Int Int64 Bool Float String DataId; do
  f="$SRC/Property$kind.cs"
  [ -f "$f" ] || continue
  lower=$(echo "$kind" | tr 'A-Z' 'a-z')
  sed -n 's/^[[:space:]]*\([A-Za-z_][A-Za-z0-9_]*\)[[:space:]]*=[[:space:]]*\([0-9]\{1,\}\),\{0,1\}.*$/\1,\2/p' "$f" |
    awk -F, -v k="$lower" '$2 != "" { print k "," $2 "," $1 }'
done | LC_ALL=C sort -t, -k1,1 -k2,2n -u
