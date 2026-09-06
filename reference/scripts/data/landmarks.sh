#!/bin/sh
# Regenerate crates/ac-world/data/landmarks.csv: the fixed things in the
# world a player navigates to. Lifestones, vendors and the standing NPCs
# (non-attackable creatures) are server-side objects, so they are not in
# the client's own data files.
#
# Usage: reference/scripts/data/landmarks.sh > crates/ac-world/data/landmarks.csv
set -e

SQL='select case w.type when 25 then "lifestone" when 12 then "vendor"
    else "npc" end,
  s.value, hex(li.obj_Cell_Id), round(li.origin_X,1),
  round(li.origin_Y,1), round(li.origin_Z,1)
from landblock_instance li
join weenie w on w.class_Id = li.weenie_Class_Id
join weenie_properties_string s
  on s.object_Id = w.class_Id and s.type = 1
left join weenie_properties_bool b
  on b.object_Id = w.class_Id and b.type = 19
where w.type = 25 or w.type = 12
   or (w.type = 10 and b.value = 0);'

cat <<'HEADER'
# Landmarks of Dereth: the fixed things a player navigates to.
# kind,name,cell,x,y,z   (kind: lifestone, vendor, npc)
# Cells are hex; the x/y/z are local to the cell's landblock.
# Regenerate with reference/scripts/data/landmarks.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 6 {
    gsub(/,/, ";", $2)
    printf "%s,%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5, $6
  }' |
  LC_ALL=C sort -t, -k1,1 -k2,2 -k3,3
