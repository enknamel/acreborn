#!/bin/sh
# Regenerate crates/ac-world/data/spell_effects.csv: what each
# enchantment does to the thing it lands on.
#
# The client's own SpellTable says a spell's school, level, and whether
# it is cast on the caster, a creature or an item, but not what it
# changes. The server knows: a stat-mod type (which kind of statistic),
# a key (which one: a skill id, an attribute id, a resistance) and a
# value. With those a client can tell Heavy Weapon Mastery from War
# Magic Mastery without reading either name, and so buff a character for
# the skills it has actually trained.
#
# Nothing is matched by name; see spell_elements.sh for why that would
# lose the strongest spells.
#
# Usage: reference/scripts/data/spell_effects.sh > crates/ac-world/data/spell_effects.csv
set -e

SQL='select id, stat_Mod_Type, coalesce(stat_Mod_Key, 0), round(stat_Mod_Val, 4), name
from spell where stat_Mod_Type is not null and stat_Mod_Type > 0
order by id;'

cat <<'HEADER'
# What each enchantment changes on whatever it lands on.
# spell_id,mod_type,mod_key,mod_val,name
# mod_type is the server's EnchantmentTypeFlags word; its low byte says
# what kind of statistic (1 attribute, 2 vital, 4 int, 8 float, 0x10
# skill, 0x80 body armour) and mod_key which one (a skill id, an
# attribute id, a float property such as a resistance). The name is
# there to read by; the client matches on the id.
# Regenerate with reference/scripts/data/spell_effects.sh (reads the
# ACE world database, the community's reconstruction of retail's server
# data; the client's SpellTable does not carry the effect).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 5 {
    gsub(/,/, ";", $5)
    printf "%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5
  }' |
  LC_ALL=C sort -t, -k1,1n
