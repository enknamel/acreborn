#!/bin/sh
# Regenerate crates/ac-world/data/recalls.csv: where each spell that
# sends its target to a fixed place puts them. Aerlinthe Recall,
# Ulgrim's Recall, Lyceum Recall and the other "... Recall" spells a
# character learns from a quest; also the sendings cast by gems and by
# NPCs, which land in the same table.
#
# The client's own SpellTable says such a spell exists (its meta type
# is PortalSending) but not where it goes: the server keeps that in its
# spell table (ACE reads `spell.Position` from these columns and
# teleports the target there), so the destinations are copied from the
# local ACE world database.
#
# The recalls whose destination is the character's own (Lifestone
# Recall, Lifestone Sending, Portal Recall, Primary and Secondary Portal
# Recall) are not here: their destinations are positions the character
# saved by using a lifestone, tying a portal or walking through one,
# and `ac_world::recalls` names them in code.
#
# Usage: reference/scripts/data/recalls.sh > crates/ac-world/data/recalls.csv
set -e

SQL='select id, name, hex(position_Obj_Cell_ID),
  round(position_Origin_X, 3), round(position_Origin_Y, 3),
  round(position_Origin_Z, 3)
from spell
where position_Obj_Cell_ID is not null
order by id;'

cat <<'HEADER'
# Spells that send whoever they are cast on to a fixed place.
# spell_id,name,cell,x,y,z
# Cells are hex; the x/y/z are local to the cell's landblock.
# Regenerate with reference/scripts/data/recalls.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client's own SpellTable says the spell exists but not
# where it goes).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 6 {
    gsub(/,/, ";", $2)
    printf "%s,%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5, $6
  }'
