#!/bin/sh
# Regenerate crates/ac-world/data/gates.csv: the vendors a character
# cannot simply walk up to, and what stands in the way.
#
# Two things gate a counter. A society vendor asks which society you
# belong to (PropertyInt.Faction1Bits = 281 on the vendor, matched
# against the same property on the player: 1 Celestial Hand, 2 Eldrytch
# Web, 4 Radiant Blood). And a vendor can stand somewhere every portal
# into which is quest-restricted (PropertyString.QuestRestriction = 37
# on the portal), in which case reaching it at all is the quest.
#
# Neither is in the client's own data files. This reads them out of the
# local ACE world database running in Docker, and is only needed when
# that data changes.
#
# Usage: reference/scripts/data/gates.sh > crates/ac-world/data/gates.csv
set -e

SQL='
select v.class_Id, hex(li.obj_Cell_Id), "society", wpi.value
from weenie_properties_int wpi
join weenie v on v.class_Id = wpi.object_Id and v.type = 12 and v.class_Id <> 6826
join landblock_instance li on li.weenie_Class_Id = v.class_Id
where wpi.type = 281
union all
select v.class_Id, hex(li.obj_Cell_Id), "quest", g.quest
from (
  select hex(pp.obj_Cell_Id >> 16) lb,
         count(*) portals,
         sum(case when ws.value is null then 0 else 1 end) gated,
         min(ws.value) quest
  from weenie w
  join weenie_properties_position pp
    on pp.object_Id = w.class_Id and pp.position_Type = 2
  left join weenie_properties_string ws
    on ws.object_Id = w.class_Id and ws.type = 37
  where w.type = 7
  group by lb
) g
join landblock_instance li on hex(li.obj_Cell_Id >> 16) = g.lb
join weenie v on v.class_Id = li.weenie_Class_Id and v.type = 12 and v.class_Id <> 6826
where g.gated = g.portals;'

cat <<'HEADER'
# Vendors you cannot simply walk up to, and what stands in the way.
# A society row's requirement is the Faction1Bits the character must
# carry; a quest row's is the flag the portals in ask for.
# vendor_wcid,cell,kind,requirement
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 4 { gsub(/,/, ";", $4); printf "%s,%s,%s,%s\n", $1, $2, $3, $4 }' |
  LC_ALL=C sort -u -t, -k1,1n -k2,2
