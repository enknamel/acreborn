#!/bin/sh
# Regenerate crates/ac-world/data/vendor_buys.csv: what every vendor in
# the world will take off a player's hands, and at what rate.
#
# A vendor buys anything whose ItemType is in its MerchandiseItemTypes
# bitfield and whose value falls between MerchandiseMinValue and
# MerchandiseMaxValue; it pays the item's value times its buy rate and
# charges its sell rate for the stock in vendors.csv. All of that is
# server-side, on the vendor weenie, so a client that wants to know
# where to unload salvage needs a copy of it. This reads it out of the
# local ACE world database (the community's reconstruction of retail's
# server data) running in Docker, and is only needed when that data
# changes.
#
# One row per placed vendor, keyed by (vendor_wcid, cell) so it lines up
# with vendors.csv.
#
# Usage: reference/scripts/data/vendor_buys.sh > crates/ac-world/data/vendor_buys.csv
set -e

SQL='select distinct li.weenie_Class_Id, vn.value, hex(li.obj_Cell_Id),
  round(li.origin_X,1), round(li.origin_Y,1), round(li.origin_Z,1),
  hex(coalesce(mt.value, 0) & 4294967295),
  coalesce(mn.value, 0), coalesce(mx.value, 0),
  round(coalesce(bp.value, 0), 3), round(coalesce(sp.value, 0), 3)
from landblock_instance li
join weenie v on v.class_Id = li.weenie_Class_Id and v.type = 12
join weenie_properties_string vn
  on vn.object_Id = v.class_Id and vn.type = 1
left join weenie_properties_int mt
  on mt.object_Id = v.class_Id and mt.type = 74
left join weenie_properties_int mn
  on mn.object_Id = v.class_Id and mn.type = 75
left join weenie_properties_int mx
  on mx.object_Id = v.class_Id and mx.type = 76
left join weenie_properties_float bp
  on bp.object_Id = v.class_Id and bp.type = 37
left join weenie_properties_float sp
  on sp.object_Id = v.class_Id and sp.type = 38;'

cat <<'HEADER'
# Shops of Dereth: what each vendor buys, and the prices they keep.
# vendor_wcid,vendor_name,cell,x,y,z,item_types,min_value,max_value,buy_rate,sell_rate
# Cells are hex; the x/y/z are the vendor's, local to the cell's
# landblock. item_types is a hex ItemType mask (ac_world::item_type) of
# the kinds the vendor takes, 0 for one that buys nothing; a vendor also
# refuses anything worth less than min_value or more than max_value.
# The vendor pays value * buy_rate and charges value * sell_rate for the
# stock in vendors.csv.
# Regenerate with reference/scripts/data/vendor_buys.sh (reads the ACE
# world database, the community's reconstruction of retail's server
# data; the client has no copy of its own).
HEADER

docker exec ace-db sh -c \
  "mysql -uroot -p\"\$MYSQL_ROOT_PASSWORD\" -N --batch ace_world -e '$SQL'" \
  2>/dev/null |
  awk -F'\t' 'NF == 11 {
    gsub(/,/, ";", $2)
    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n",
      $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11
  }' |
  LC_ALL=C sort -t, -k2,2 -k3,3
