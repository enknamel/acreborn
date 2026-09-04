// Export reference material from the acclient.exe Ghidra project.
//
// Writes, under the output directory given as the first script argument:
//   decomp/by_func/<addr>_<name>.c   one decompiled C file per function
//   decomp/failed.tsv                functions the decompiler gave up on
//   dumps/functions.tsv              addr, name, namespace, size, callers, callees, thunk
//   dumps/calls.tsv                  caller_addr, callee_addr
//   dumps/strings.tsv                addr, string, referencing function addrs
//   dumps/imports.tsv                library, name, addr, referencing function addrs
//   dumps/symbols.tsv                addr, name, namespace, type
//   dumps/vtables.tsv                vtable symbol, vtable addr, slot, function addr, function name
//
// Usage (Ghidra GUI must be closed):
//   analyzeHeadless ~/code acclient -process acclient.exe -noanalysis \
//       -scriptPath reference/scripts/ghidra -postScript ExportAll.java <out_dir> [max_functions]
//
// @category acreborn
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;
import java.util.function.Consumer;

import ghidra.app.decompiler.*;
import ghidra.app.decompiler.parallel.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.data.DataType;
import ghidra.program.model.data.Pointer;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.*;
import ghidra.util.task.TaskMonitor;

public class ExportAll extends GhidraScript {

    private Path out;
    private FunctionManager fm;
    private ReferenceManager rm;

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 1) {
            printerr("usage: ExportAll.java <out_dir> [max_functions]");
            return;
        }
        out = Paths.get(args[0]);
        int maxFunctions = args.length > 1 ? Integer.parseInt(args[1]) : Integer.MAX_VALUE;
        Files.createDirectories(out.resolve("decomp/by_func"));
        Files.createDirectories(out.resolve("dumps"));

        fm = currentProgram.getFunctionManager();
        rm = currentProgram.getReferenceManager();

        exportFunctionsAndCalls();
        exportStrings();
        exportImports();
        exportSymbols();
        exportVtables();
        decompileAll(maxFunctions);
        println("ExportAll: done -> " + out);
    }

    private static String tsv(Object... cells) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < cells.length; i++) {
            if (i > 0) sb.append('\t');
            sb.append(String.valueOf(cells[i]).replace('\t', ' ').replace('\n', ' ').replace('\r', ' '));
        }
        return sb.append('\n').toString();
    }

    private static String sanitize(String name) {
        String s = name.replaceAll("[^A-Za-z0-9_.]", "_");
        return s.length() > 120 ? s.substring(0, 120) : s;
    }

    private String funcAddrOf(Address a) {
        Function f = fm.getFunctionContaining(a);
        return f == null ? "" : f.getEntryPoint().toString();
    }

    private String referrers(Address a) {
        TreeSet<String> fs = new TreeSet<>();
        for (Reference r : rm.getReferencesTo(a)) {
            String f = funcAddrOf(r.getFromAddress());
            if (!f.isEmpty()) fs.add(f);
        }
        return String.join(",", fs);
    }

    private void exportFunctionsAndCalls() throws IOException {
        try (BufferedWriter fw = Files.newBufferedWriter(out.resolve("dumps/functions.tsv"));
             BufferedWriter cw = Files.newBufferedWriter(out.resolve("dumps/calls.tsv"))) {
            fw.write(tsv("addr", "name", "namespace", "size", "n_callers", "n_callees", "is_thunk"));
            cw.write(tsv("caller", "callee"));
            for (Function f : fm.getFunctions(true)) {
                Set<Function> callees = f.getCalledFunctions(monitor);
                Set<Function> callers = f.getCallingFunctions(monitor);
                fw.write(tsv(f.getEntryPoint(), f.getName(), f.getParentNamespace().getName(true),
                        f.getBody().getNumAddresses(), callers.size(), callees.size(), f.isThunk()));
                for (Function c : callees) {
                    cw.write(tsv(f.getEntryPoint(), c.getEntryPoint()));
                }
            }
        }
        println("functions/calls exported");
    }

    private void exportStrings() throws IOException {
        try (BufferedWriter w = Files.newBufferedWriter(out.resolve("dumps/strings.tsv"))) {
            w.write(tsv("addr", "string", "referencing_functions"));
            for (Data d : currentProgram.getListing().getDefinedData(true)) {
                if (!d.hasStringValue()) continue;
                Object v = d.getValue();
                if (v == null) continue;
                w.write(tsv(d.getAddress(), v.toString(), referrers(d.getAddress())));
            }
        }
        println("strings exported");
    }

    private void exportImports() throws IOException {
        try (BufferedWriter w = Files.newBufferedWriter(out.resolve("dumps/imports.tsv"))) {
            w.write(tsv("library", "name", "addr", "referencing_functions"));
            SymbolTable st = currentProgram.getSymbolTable();
            for (Symbol s : st.getExternalSymbols()) {
                ExternalLocation loc = currentProgram.getExternalManager().getExternalLocation(s);
                if (loc == null) continue;
                TreeSet<String> fs = new TreeSet<>();
                // References come in via the import-address-table slot and via thunks.
                for (Reference r : rm.getReferencesTo(s.getAddress())) {
                    String f = funcAddrOf(r.getFromAddress());
                    if (!f.isEmpty()) fs.add(f);
                    // Thunk functions: add their callers too.
                    Function thunk = fm.getFunctionAt(r.getFromAddress());
                    if (thunk != null && thunk.isThunk()) {
                        for (Function c : thunk.getCallingFunctions(monitor)) fs.add(c.getEntryPoint().toString());
                    }
                }
                Address a = loc.getAddress();
                if (a != null) {
                    for (Reference r : rm.getReferencesTo(a)) {
                        String f = funcAddrOf(r.getFromAddress());
                        if (!f.isEmpty()) fs.add(f);
                    }
                }
                w.write(tsv(loc.getLibraryName(), s.getName(), a == null ? "" : a.toString(), String.join(",", fs)));
            }
        }
        println("imports exported");
    }

    private void exportSymbols() throws IOException {
        try (BufferedWriter w = Files.newBufferedWriter(out.resolve("dumps/symbols.tsv"))) {
            w.write(tsv("addr", "name", "namespace", "type"));
            for (Symbol s : currentProgram.getSymbolTable().getAllSymbols(true)) {
                if (s.getSymbolType() == SymbolType.FUNCTION && s.getName().startsWith("FUN_")) continue;
                w.write(tsv(s.getAddress(), s.getName(), s.getParentNamespace().getName(true), s.getSymbolType()));
            }
        }
        println("symbols exported");
    }

    // vftables: MSVC names them ??_7Class@@6B@ ; Ghidra's RTTI analyzer labels them
    // "vftable" inside the class namespace. Walk pointer-sized slots until a
    // non-function pointer is hit.
    private void exportVtables() throws IOException {
        Memory mem = currentProgram.getMemory();
        try (BufferedWriter w = Files.newBufferedWriter(out.resolve("dumps/vtables.tsv"))) {
            w.write(tsv("symbol", "vtable_addr", "slot", "func_addr", "func_name"));
            for (Symbol s : currentProgram.getSymbolTable().getAllSymbols(true)) {
                String n = s.getName();
                if (!(n.contains("vftable") || n.startsWith("??_7"))) continue;
                Address a = s.getAddress();
                for (int slot = 0; slot < 512; slot++) {
                    Address p = a.add((long) slot * 4);
                    if (slot > 0 && currentProgram.getSymbolTable().getPrimarySymbol(p) != null) break;
                    long v;
                    try { v = mem.getInt(p) & 0xFFFFFFFFL; } catch (Exception e) { break; }
                    Address target;
                    try { target = a.getNewAddress(v); } catch (Exception e) { break; }
                    Function f = fm.getFunctionAt(target);
                    if (f == null) break;
                    w.write(tsv(s.getName(true), a, slot, target, f.getName(true)));
                }
            }
        }
        println("vtables exported");
    }

    private void decompileAll(int maxFunctions) throws Exception {
        List<Function> funcs = new ArrayList<>();
        for (Function f : fm.getFunctionsNoStubs(true)) {
            if (f.isThunk()) continue;
            funcs.add(f);
            if (funcs.size() >= maxFunctions) break;
        }
        println("decompiling " + funcs.size() + " functions");
        BufferedWriter failed = Files.newBufferedWriter(out.resolve("decomp/failed.tsv"));
        failed.write(tsv("addr", "name", "error"));

        DecompilerCallback<String[]> callback = new DecompilerCallback<>(currentProgram, new Cfg(currentProgram)) {
            @Override
            public String[] process(DecompileResults res, TaskMonitor m) {
                Function f = res.getFunction();
                String addr = f.getEntryPoint().toString();
                if (!res.decompileCompleted() || res.getDecompiledFunction() == null) {
                    return new String[] { addr, f.getName(true), "FAIL", String.valueOf(res.getErrorMessage()) };
                }
                return new String[] { addr, f.getName(true), "OK", res.getDecompiledFunction().getC() };
            }
        };
        callback.setTimeout(60);
        int[] counts = new int[2];
        Consumer<String[]> sink = r -> {
            try {
                if ("OK".equals(r[2])) {
                    Path p = out.resolve("decomp/by_func").resolve(r[0] + "_" + sanitize(r[1]) + ".c");
                    Files.write(p, ("// " + r[1] + " @ " + r[0] + "\n" + r[3]).getBytes(StandardCharsets.UTF_8));
                    counts[0]++;
                } else {
                    synchronized (failed) { failed.write(tsv(r[0], r[1], r[3])); }
                    counts[1]++;
                }
                if ((counts[0] + counts[1]) % 1000 == 0) println("  " + (counts[0] + counts[1]) + " done");
            } catch (IOException e) {
                printerr("write failed for " + r[0] + ": " + e);
            }
        };
        ParallelDecompiler.decompileFunctions(callback, currentProgram, funcs.iterator(), sink, monitor);
        callback.dispose();
        failed.close();
        println("decompiled ok=" + counts[0] + " failed=" + counts[1]);
    }

    static class Cfg implements DecompileConfigurer {
        private final Program p;
        Cfg(Program p) { this.p = p; }
        @Override
        public void configure(DecompInterface d) {
            d.toggleCCode(true);
            d.toggleSyntaxTree(false);
            d.setSimplificationStyle("decompile");
            DecompileOptions o = new DecompileOptions();
            o.grabFromProgram(p);
            d.setOptions(o);
        }
    }
}
