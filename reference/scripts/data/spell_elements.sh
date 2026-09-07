#!/bin/sh
# Regenerate crates/ac-world/data/spell_elements.csv: which element each
# spell is about.
#
# The client's own SpellTable says a spell's school, mana and formula but
# not what it hits with, so a client that wants to throw fire at
# something weak to fire has to be told. The server knows.
#
# Two roles are recorded:
#   attack  a spell that deals damage of an element (ACE's `e_Type`).
#   vuln    a spell that makes a target take more of an element (a
#           resistance enchantment that raises the multiplier).
#
# Nothing here is matched by name. Spell levels 1 to 6 are numbered but
# level 7 has its own names ("Curse of the Blades") and level 8 mixes
# the two ("Incantation of Blade Vulnerability Other"), so a name filter
# silently drops the strongest spells. The client decides which of these
# it can actually use: the SpellTable says a spell's level and whether
# it needs a target, so the self-cast versions are refused there.
#
# Spell ids are the client's own, so they can be looked up directly
# against the SpellTable. The level of a spell is not recorded here: the
# client has it, and picks the strongest one it can cast.
#
# Usage: reference/scripts/data/spell_elements.sh > crates/ac-world/data/spell_elements.csv
set -e

# stat_Mod_Key on a resistance enchantment is the PropertyFloat id of the
# resistance it changes; map those to the damage-type bit.
SQL='select id, "attack", e_Type, name from spell
  where e_Type is not null and e_Type > 0
union all
select id, "vuln",
  case stat_Mod_Key when 64 then 1 when 65 then 2 when 66 then 4
    when 68 then 8 when 67 then 16 when 69 then 32 when 70 then 64
    when 166 then 1024 else 0 end,
  name
from spell
  where stat_Mod_Key in (64,65,66,67,68,69,70,166)
    and stat_Mod_Val > 1
order by 1;'

cat <<'HEADER'
# Which element each spell is about.
# spell_id,role,element,name
# role is "attack" (deals that element) or "vuln" (makes whoever it is
# cast on take more of it: the client only uses the ones that take a
# target, so it never casts one on itself). The element is a DamageType bit: 1 slash, 2 pierce,
# 4 bludgeon, 8 cold, 16 fire, 32 acid, 64 electric, 128 health,
# 256 stamina, 512 mana, 1024 nether. The name is there to read by; the
# client matches on the id.
# Regenerate with reference/scripts/data/spell_elements.sh (reads the
# ACE world database, the community's reconstruction of retail's server
# data; the client's SpellTable does not carry the element).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 4 && $3 != 0 {
    gsub(/,/, ";", $4)
    printf "%s,%s,%s,%s\n", $1, $2, $3, $4
  }' |
  LC_ALL=C sort -t, -k1,1n
