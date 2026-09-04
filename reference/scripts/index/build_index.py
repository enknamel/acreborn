#!/usr/bin/env python3
"""Build reference/index.sqlite from the Ghidra dumps and per-function C files.

    reference/scripts/index/build_index.py            # (re)build
    reference/scripts/index/q.py fn 0040a1b0           # see q.py for queries

Tables: functions, calls, strings, imports, symbols, vtables, and an FTS5
table `code` over each function's decompiled body.
"""
import csv, os, sqlite3, sys
from pathlib import Path

REF = Path(__file__).resolve().parents[2]
DUMPS = REF / "dumps"
BY_FUNC = REF / "decomp" / "by_func"
DB = REF / "index.sqlite"

csv.field_size_limit(1 << 30)

def load_tsv(name):
    with open(DUMPS / name, newline="", encoding="utf-8", errors="replace") as f:
        r = csv.reader(f, delimiter="\t", quoting=csv.QUOTE_NONE)
        header = next(r)
        for row in r:
            yield row + [""] * (len(header) - len(row))

def main():
    if DB.exists():
        DB.unlink()
    db = sqlite3.connect(DB)
    db.executescript("""
    create table functions(addr text primary key, name text, namespace text, size int, n_callers int, n_callees int, is_thunk int);
    create table calls(caller text, callee text);
    create table strings(addr text, string text, funcs text);
    create table imports(library text, name text, addr text, funcs text);
    create table symbols(addr text, name text, namespace text, type text);
    create table vtables(symbol text, vtable_addr text, slot int, func_addr text, func_name text);
    create virtual table code using fts5(addr unindexed, name, body, tokenize='unicode61 tokenchars ''_''');
    """)
    db.executemany("insert into functions values(?,?,?,?,?,?,?)",
                   ((a, n, ns, int(s or 0), int(c1 or 0), int(c2 or 0), 1 if t == "true" else 0)
                    for a, n, ns, s, c1, c2, t, *_ in load_tsv("functions.tsv")))
    db.executemany("insert into calls values(?,?)", (r[:2] for r in load_tsv("calls.tsv")))
    db.executemany("insert into strings values(?,?,?)", (r[:3] for r in load_tsv("strings.tsv")))
    db.executemany("insert into imports values(?,?,?,?)", (r[:4] for r in load_tsv("imports.tsv")))
    db.executemany("insert into symbols values(?,?,?,?)", (r[:4] for r in load_tsv("symbols.tsv")))
    db.executemany("insert into vtables values(?,?,?,?,?)",
                   ((s, v, int(sl or 0), fa, fn) for s, v, sl, fa, fn, *_ in load_tsv("vtables.tsv")))
    n = 0
    for p in sorted(BY_FUNC.glob("*.c")):
        addr, _, name = p.stem.partition("_")
        db.execute("insert into code values(?,?,?)", (addr, name, p.read_text(errors="replace")))
        n += 1
        if n % 5000 == 0:
            print(f"  {n} bodies", file=sys.stderr)
    db.executescript("""
    create index calls_caller on calls(caller);
    create index calls_callee on calls(callee);
    create index functions_name on functions(name);
    create index symbols_name on symbols(name);
    create index vtables_symbol on vtables(symbol);
    """)
    db.commit()
    print(f"indexed {db.execute('select count(*) from functions').fetchone()[0]} functions, {n} bodies -> {DB}")

if __name__ == "__main__":
    main()
