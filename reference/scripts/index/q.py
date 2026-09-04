#!/usr/bin/env python3
"""Query reference/index.sqlite.

  q.py fn <addr|name>            function row + path of its C file
  q.py callers <addr>            functions calling addr
  q.py callees <addr>            functions called by addr
  q.py str <substring>           strings matching, with referencing functions
  q.py import <name>             functions referencing an import
  q.py class <Name>              methods and vtable of a recovered class
  q.py grep <fts5 query>         full-text search over decompiled bodies
  q.py subsystem <name> <seed>.. concatenate 2-hop neighbourhood of seeds into decomp/subsystems/<name>.c
"""
import sqlite3, sys
from pathlib import Path

REF = Path(__file__).resolve().parents[2]
db = sqlite3.connect(REF / "index.sqlite")
BY_FUNC = REF / "decomp" / "by_func"

def cfile(addr):
    m = list(BY_FUNC.glob(f"{addr}_*.c"))
    return m[0] if m else None

def show_fn(addr):
    row = db.execute("select * from functions where addr=?", (addr,)).fetchone()
    print("\t".join(map(str, row)) if row else f"{addr}: not a function", cfile(addr) or "")

def resolve(x):
    if db.execute("select 1 from functions where addr=?", (x,)).fetchone():
        return [x]
    return [r[0] for r in db.execute("select addr from functions where name=? or name like ?", (x, f"%::{x}"))]

def funcs_from_csv(s):
    return [a for a in s.split(",") if a]

cmd, *args = sys.argv[1:] or ["help"]
if cmd == "fn":
    for a in resolve(args[0]): show_fn(a)
elif cmd == "callers":
    for (a,) in db.execute("select caller from calls where callee=?", (args[0],)): show_fn(a)
elif cmd == "callees":
    for (a,) in db.execute("select callee from calls where caller=?", (args[0],)): show_fn(a)
elif cmd == "str":
    for addr, s, fs in db.execute("select * from strings where string like ?", (f"%{args[0]}%",)):
        print(f"{addr}\t{s!r}")
        for a in funcs_from_csv(fs): print("   ", end=""); show_fn(a)
elif cmd == "import":
    for lib, name, addr, fs in db.execute("select * from imports where name like ?", (f"%{args[0]}%",)):
        print(f"{lib}!{name} @ {addr}")
        for a in funcs_from_csv(fs): print("   ", end=""); show_fn(a)
elif cmd == "class":
    for row in db.execute("select addr,name,size from functions where namespace=? order by addr", (args[0],)):
        print("\t".join(map(str, row)))
    for row in db.execute("select slot,func_addr,func_name from vtables where symbol like ? order by vtable_addr,slot", (f"%{args[0]}%",)):
        print("vslot", *row)
elif cmd == "grep":
    for addr, name in db.execute("select addr,name from code where code match ? limit 200", (" ".join(args),)):
        print(addr, name, cfile(addr))
elif cmd == "subsystem":
    name, *seeds = args
    seen = set()
    frontier = set()
    for s in seeds:
        frontier |= set(resolve(s))
    for hop in range(2):
        nxt = set()
        for a in frontier:
            if a in seen: continue
            seen.add(a)
            nxt |= {r[0] for r in db.execute("select callee from calls where caller=?", (a,))}
            nxt |= {r[0] for r in db.execute("select caller from calls where callee=?", (a,))}
        frontier = nxt - seen
    seen |= frontier
    out = REF / "decomp" / "subsystems" / f"{name}.c"
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as f:
        f.write(f"// subsystem {name}: {len(seen)} functions from seeds {seeds}\n")
        for a in sorted(seen):
            p = cfile(a)
            if p: f.write(p.read_text(errors="replace") + "\n\n")
    print(out, len(seen), "functions")
else:
    print(__doc__)
