#!/bin/sh
# Regenerate crates/ac-world/data/gems.csv: the portal gems a character
# can carry, and where using one puts them.
#
# A gem is a weenie of type 38 that links to a portal (LinkedPortalOne,
# DataId 31); the portal carries the Destination position. Using the gem
# summons that portal, so for a journey a carried gem is a hop straight
# to the portal's exit.
#
# Reads the local ACE world database (the community's reconstruction of
# retail's server data) running in Docker; only needed when that changes.
#
# Usage: reference/scripts/data/gems.sh > crates/ac-world/data/gems.csv
set -e

SQL='select w.class_Id, replace(gs.value, ",", ";"), hex(p.obj_Cell_Id),
       round(p.origin_X, 1), round(p.origin_Y, 1), round(p.origin_Z, 1), d.value
from weenie w
join weenie_properties_string gs on gs.object_Id = w.class_Id and gs.type = 1
join weenie_properties_d_i_d l on l.object_Id = w.class_Id and l.type = 31
join weenie_properties_d_i_d d on d.object_Id = w.class_Id and d.type = 28
join weenie_properties_position p on p.object_Id = l.value and p.position_Type = 2
where w.type = 38
group by w.class_Id, gs.value, p.obj_Cell_Id, p.origin_X, p.origin_Y, p.origin_Z, d.value;'

cat <<'HEADER'
# Portal gems: what a carried gem does when it is used.
# wcid,name,cell,x,y,z,spell
# Cells are hex; the x/y/z are local to the cell's landblock. Using the
# Nearly every gem casts Summon Portal (157): it puts a portal in front
# of you which you then walk into, so using one costs the use and the
# portal. The handful casting something else teleport on use. `spell` is
# what it casts, so the two can be told apart.
# Regenerate with reference/scripts/data/gems.sh (reads the ACE world
# database, the community's reconstruction of retail's server data).
HEADER

docker exec ace-db sh -c "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  | awk -F'\t' '{printf "%s,%s,%s,%s,%s,%s,%s\n", $1, $2, $3, $4, $5, $6, $7}' \
  | LC_ALL=C sort -t, -k2
