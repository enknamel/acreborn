#!/bin/sh
# Regenerate crates/ac-world/data/vendors.csv: what every vendor in the
# world has for sale, and where that vendor stands.
#
# A vendor's stock is server-side: the weenie's create list, the rows
# whose destination is Shop (ACE DestinationType.Shop = 4), which is the
# same table that gives a creature its wielded gear (Wield = 2) and its
# corpse loot (Treasure = 8). None of it is in the client's own data
# files, so a client that wants to know who sells tapers needs a copy of
# the list. This reads it out of the local ACE world database (the
# community's reconstruction of retail's server data) running in Docker,
# and is only needed when that data changes.
#
# The vendor's position is repeated on every row so that finding a
# seller and walking to them is one lookup. A vendor weenie placed in
# two towns is two sets of rows; the vendor is keyed by (wcid, cell).
#
# Usage: reference/scripts/data/vendors.sh > crates/ac-world/data/vendors.csv
set -e

SQL='select distinct li.weenie_Class_Id, vn.value, hex(li.obj_Cell_Id),
  round(li.origin_X,1), round(li.origin_Y,1), round(li.origin_Z,1),
  cl.weenie_Class_Id, itn.value,
  hex(coalesce(it.value, 0) & 4294967295), coalesce(val.value, 0)
from landblock_instance li
join weenie v on v.class_Id = li.weenie_Class_Id and v.type = 12
join weenie_properties_string vn
  on vn.object_Id = v.class_Id and vn.type = 1
join weenie_properties_create_list cl
  on cl.object_Id = v.class_Id and cl.destination_Type = 4
join weenie_properties_string itn
  on itn.object_Id = cl.weenie_Class_Id and itn.type = 1
left join weenie_properties_int it
  on it.object_Id = cl.weenie_Class_Id and it.type = 1
left join weenie_properties_int val
  on val.object_Id = cl.weenie_Class_Id and val.type = 19;'

cat <<'HEADER'
# Shops of Dereth: one row per thing a vendor has for sale.
# vendor_wcid,vendor_name,cell,x,y,z,item_wcid,item_name,item_type,value
# Cells are hex; the x/y/z are the vendor's, local to the cell's
# landblock. item_type is hex ItemType bits (ac_world::item_type), so
# comps are 1000, missile weapons 100; value is the item's base pyreal
# worth, which the vendor marks up by the sell rate in vendor_buys.csv.
# A vendor's stock is endless: these are the kinds sold, not a count.
# Regenerate with reference/scripts/data/vendors.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 10 {
    gsub(/,/, ";", $2)
    gsub(/,/, ";", $8)
    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n",
      $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
  }' |
  LC_ALL=C sort -t, -k2,2 -k3,3 -k8,8
