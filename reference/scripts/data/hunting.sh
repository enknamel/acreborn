#!/bin/sh
# Regenerate crates/ac-world/data/hunting.csv: the outdoor landblocks
# where monsters spawn, with the levels of what spawns there.
#
# Monsters are server-side: a landblock holds generator objects, each of
# which spawns creatures from its list (weenie_properties_generator),
# and a generator can spawn other generators, so the list is walked to
# a depth of four. A creature counts when it is attackable and has a
# level. Per landblock this gives how many spawn entries there are, the
# lowest and highest level among them, and the level and name of the
# creature that appears most often, which is what a hunter meets. The
# x/y are the average of the spawn points, local to the landblock.
#
# This reads the local ACE world database (the community's
# reconstruction of retail's server data) running in Docker, and is only
# needed when that data changes.
#
# Usage: reference/scripts/data/hunting.sh > crates/ac-world/data/hunting.csv
set -e

SQL='with recursive spawn(inst, wcid, depth) as (
  select li.guid, li.weenie_Class_Id, 0 from landblock_instance li
  where (li.obj_Cell_Id & 65535) < 256
  union all
  select s.inst, g.weenie_Class_Id, s.depth + 1 from spawn s
  join weenie_properties_generator g on g.object_Id = s.wcid
  where s.depth < 4
),
mon as (
  select distinct s.inst, s.wcid, li.obj_Cell_Id >> 16 lb,
    li.origin_X x, li.origin_Y y, lv.value lv, nm.value nm
  from spawn s
  join landblock_instance li on li.guid = s.inst
  join weenie c on c.class_Id = s.wcid and c.type = 10
  join weenie_properties_int lv on lv.object_Id = c.class_Id and lv.type = 25
  join weenie_properties_string nm on nm.object_Id = c.class_Id and nm.type = 1
  left join weenie_properties_bool b on b.object_Id = c.class_Id and b.type = 19
  where coalesce(b.value, 1) = 1
),
bynm as (select lb, nm, lv, count(*) n from mon group by lb, nm, lv)
select hex(m.lb), round(avg(m.x), 0), round(avg(m.y), 0), count(*),
  min(m.lv), max(m.lv),
  (select b.lv from bynm b where b.lb = m.lb order by b.n desc, b.lv limit 1),
  (select b.nm from bynm b where b.lb = m.lb order by b.n desc, b.lv limit 1)
from mon m group by m.lb;'

cat <<'HEADER'
# Hunting grounds of Dereth: the outdoor landblocks where monsters spawn.
# landblock,x,y,count,min_level,max_level,level,name
# The landblock is hex; x/y are the average spawn point, local to it.
# count is the number of spawn entries, min/max the level range among
# them, and level/name the creature that spawns most often there.
# Regenerate with reference/scripts/data/hunting.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 8 {
    gsub(/,/, ";", $8)
    printf "%s,%s,%s,%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5, $6, $7, $8
  }' |
  LC_ALL=C sort -t, -k1,1
