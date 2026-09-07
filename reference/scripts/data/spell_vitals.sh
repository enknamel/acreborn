#!/bin/sh
# Regenerate crates/ac-world/data/spell_vitals.csv: the spells that move
# health, stamina and mana about.
#
# Two kinds. A boost restores a vital by an amount (Heal Self,
# Revitalize Self); a transfer drains a share of one vital into
# another (Stamina to Mana Self), less a loss that may be negative,
# which is a gain. These are how a caster keeps going: stamina is
# poured into mana, and Revitalize, which costs mana, refills the
# stamina for less than it made. The client's own SpellTable does not
# say which spell does which, so this does.
#
# Vitals are the server's PropertyAttribute2nd ids: 2 health, 4 stamina,
# 6 mana. Nothing is matched by name.
#
# Usage: reference/scripts/data/spell_vitals.sh > crates/ac-world/data/spell_vitals.csv
set -e

# A boost says which vital it restores through its damage type (128
# health, 256 stamina, 512 mana), the way the server reads it.
SQL='select id, "boost",
    case damage_Type when 128 then 2 when 256 then 4 when 512 then 6 else 0 end,
    boost, boost + boost_Variance, 0, 0, 0, name
  from spell where boost is not null and boost <> 0
union all
select id, "transfer", source, destination, 0,
    proportion, loss_Percent, coalesce(transfer_Cap, 0), name
  from spell where source is not null and source > 0
order by 1;'

cat <<'HEADER'
# Spells that move health, stamina and mana about.
# spell_id,kind,a,b,c,proportion,loss,cap,name
# kind "boost": restores vital a by b to c points.
# kind "transfer": drains `proportion` of vital a into vital b, less
#   `loss` of it (negative is a gain), at most `cap` when cap is not 0.
# Vitals: 2 health, 4 stamina, 6 mana. The name is there to read by;
# the client matches on the id.
# Regenerate with reference/scripts/data/spell_vitals.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client's SpellTable does not carry this).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 9 {
    gsub(/,/, ";", $9)
    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5, $6, $7, $8, $9
  }' |
  LC_ALL=C sort -t, -k1,1n
