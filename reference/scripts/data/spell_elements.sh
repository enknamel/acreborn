#!/bin/sh
# Regenerate crates/ac-world/data/spell_elements.csv: the damage type
# each attack spell deals.
#
# The client's own SpellTable says a spell's school, mana and formula
# but not what it hits with, so a client that wants to throw fire at
# something weak to fire has to be told. The server knows: ACE's spell
# table carries the element as `e_Type`, a DamageType bit (1 slash,
# 2 pierce, 4 bludgeon, 8 cold, 16 fire, 32 acid, 64 electric,
# 128 health, 256 stamina, 512 mana, 1024 nether).
#
# Spell ids are the client's own, so they can be looked up directly
# against the SpellTable.
#
# Usage: reference/scripts/data/spell_elements.sh > crates/ac-world/data/spell_elements.csv
set -e

SQL='select id, e_Type, name from spell
  where e_Type is not null and e_Type > 0 order by id;'

cat <<'HEADER'
# What each attack spell hits with. The element is a DamageType bit:
# 1 slash, 2 pierce, 4 bludgeon, 8 cold, 16 fire, 32 acid,
# 64 electric, 128 health, 256 stamina, 512 mana, 1024 nether.
# spell_id,element,name
# The name is there to read by; the client matches on the id.
# Regenerate with reference/scripts/data/spell_elements.sh (reads the
# ACE world database, the community's reconstruction of retail's server
# data; the client's SpellTable does not carry the element).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 3 { gsub(/,/, ";", $3); printf "%s,%s,%s\n", $1, $2, $3 }' |
  LC_ALL=C sort -t, -k1,1n
