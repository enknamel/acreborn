#!/bin/sh
# Regenerate crates/ac-world/data/fletching.csv: how ammunition is made.
#
# Arrows, quarrels and atlatl darts are made by using a bundle of heads
# on a bundle of shafts; the head decides the element of what comes out
# and the shaft decides what it fits. A character out of the ammunition
# it wants can make more if it carries the two bundles and its Fletching
# is up to the recipe. The recipes are server data, so this is a copy.
#
# Nothing is matched by name: the element of the result is its damage
# type, and what it fits is its ammunition type, both read from the
# result's own weenie.
#
# Usage: reference/scripts/data/fletching.sh > crates/ac-world/data/fletching.csv
set -e

SQL='select cb.source_W_C_I_D, cb.target_W_C_I_D, r.success_W_C_I_D,
    r.success_Amount, r.difficulty,
    coalesce((select value from weenie_properties_int where object_Id = r.success_W_C_I_D and type = 45), 0),
    coalesce((select value from weenie_properties_int where object_Id = r.success_W_C_I_D and type = 50), 0),
    ss.value, ts.value, coalesce(rs.value, "")
  from cook_book cb
  join recipe r on r.id = cb.recipe_Id
  join weenie_properties_string ss on ss.object_Id = cb.source_W_C_I_D and ss.type = 1
  join weenie_properties_string ts on ts.object_Id = cb.target_W_C_I_D and ts.type = 1
  left join weenie_properties_string rs on rs.object_Id = r.success_W_C_I_D and rs.type = 1
  where r.skill = 37
    and (select value from weenie_properties_int where object_Id = r.success_W_C_I_D and type = 51) = 3
  order by cb.source_W_C_I_D, cb.target_W_C_I_D;'

cat <<'HEADER'
# How ammunition is made: use `source` (a bundle of heads) on `target`
# (a bundle of shafts) and `amount` of `result` come out, if Fletching
# is up to `difficulty`.
# source_wcid,target_wcid,result_wcid,amount,difficulty,element,ammo_type,source_name,target_name,result_name
# element is the result's damage type (2 pierce, 4 bludgeon, 8 cold,
# 16 fire, 32 acid, 64 electric); ammo_type is what it fits (1 a bow,
# 2 a crossbow, 4 an atlatl). Names are there to read by; the client
# matches on the ids.
# Regenerate with reference/scripts/data/fletching.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 10 {
    for (i = 8; i <= 10; i++) gsub(/,/, ";", $i)
    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
  }' |
  LC_ALL=C sort -t, -k1,1n -k2,2n
