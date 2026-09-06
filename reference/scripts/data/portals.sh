#!/bin/sh
# Regenerate crates/ac-world/data/portals.csv: every portal placed in the
# world with where it stands and where it leads.
#
# Portals are server-side objects: they are not in the client's own data
# files, so a client that wants to plan a trip through them needs a copy
# of the list. This reads it out of the local ACE world database (the
# community's reconstruction of retail's server data) running in Docker,
# and is only needed when that data changes.
#
# Usage: reference/scripts/data/portals.sh > crates/ac-world/data/portals.csv
set -e

SQL='select s.value, hex(li.obj_Cell_Id), round(li.origin_X,1),
  round(li.origin_Y,1), round(li.origin_Z,1), hex(p.obj_Cell_Id),
  round(p.origin_X,1), round(p.origin_Y,1), round(p.origin_Z,1),
  coalesce(lo.value, 0), coalesce(hi.value, 0),
  coalesce(q.value, "")
from landblock_instance li
join weenie_properties_position p
  on p.object_Id = li.weenie_Class_Id and p.position_Type = 2
join weenie_properties_int i
  on i.object_Id = li.weenie_Class_Id and i.type = 1 and i.value = 65536
join weenie_properties_string s
  on s.object_Id = li.weenie_Class_Id and s.type = 1
left join weenie_properties_int lo
  on lo.object_Id = li.weenie_Class_Id and lo.type = 86
left join weenie_properties_int hi
  on hi.object_Id = li.weenie_Class_Id and hi.type = 87
left join weenie_properties_string q
  on q.object_Id = li.weenie_Class_Id and q.type = 37;'

cat <<'HEADER'
# Portals of Dereth: where each one stands and where it leads.
# name,from_cell,from_x,from_y,from_z,to_cell,to_x,to_y,to_z,min_level,max_level,quest
# Cells are hex; the x/y/z are local to the cell's landblock.
# Regenerate with reference/scripts/data/portals.sh (reads the
# ACE world database, which is the community's reconstruction of
# retail's server data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 12 {
    gsub(/,/, ";", $1)
    gsub(/,/, ";", $12)
    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n",
      $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12
  }' |
  LC_ALL=C sort -t, -k2,2 -k1,1
