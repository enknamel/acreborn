#!/bin/sh
# Regenerate crates/ac-world/data/creatures.csv: how much damage each
# kind of creature takes from each element.
#
# Every creature has a multiplier per damage type. Higher means it takes
# more: an Ice Golem's cold multiplier is 0 (it is immune) and its fire
# multiplier is 1. Choosing what to hit something with is choosing the
# type with the highest number, which is why the client wants this.
#
# This is server data, not in the client's own files, so it is read out
# of the local ACE world database (the community's reconstruction of
# retail's) and only needs regenerating when that data changes.
#
# The key is the weenie class id, which the server sends in every
# object description, so a creature in view is matched exactly. The name
# is kept beside it: it is what the rules are written in, and it catches
# a creature whose weenie is not in this copy of the data.
#
# Usage: reference/scripts/data/creatures.sh > crates/ac-world/data/creatures.csv
set -e

SQL='select w.class_Id, s.value, coalesce(max(h.current_Level), 0),
  max(case when f.type=64 then round(f.value,3) end),
  max(case when f.type=65 then round(f.value,3) end),
  max(case when f.type=66 then round(f.value,3) end),
  max(case when f.type=68 then round(f.value,3) end),
  max(case when f.type=67 then round(f.value,3) end),
  max(case when f.type=69 then round(f.value,3) end),
  max(case when f.type=70 then round(f.value,3) end),
  max(case when f.type=166 then round(f.value,3) end)
from weenie w
join weenie_properties_string s
  on s.object_Id = w.class_Id and s.type = 1
join weenie_properties_float f
  on f.object_Id = w.class_Id and f.type in (64,65,66,67,68,69,70,166)
left join weenie_properties_attribute_2nd h
  on h.object_Id = w.class_Id and h.type = 1
where w.type = 10 and s.value <> ""
group by w.class_Id, s.value
having count(case when f.type in (64,65,66,67,68,69,70) then 1 end) > 0;'

cat <<'HEADER'
# What each kind of creature takes from each element: a damage
# multiplier, higher meaning it is hurt more. 0 is immune.
# wcid,name,health,slash,pierce,bludgeon,cold,fire,acid,electric,nether
# health is how much it has at full, 0 when not recorded: what tells a
# thing worth spending a vulnerability on from one that dies first.
# An empty column means the creature has no figure for that element.
# Regenerate with reference/scripts/data/creatures.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 11 {
    gsub(/,/, ";", $2)
    for (i = 4; i <= 11; i++) if ($i == "NULL") $i = ""
    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n",
      $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11
  }' |
  LC_ALL=C sort -t, -k1,1n
