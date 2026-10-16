// Batch query runner for headless Ghidra. Reads one command per line from a file so
// shells never have to quote addresses, regexes or commas.
//
// args: <cmdfile> <outfile>
//
// Commands:
//   maxlen <n>          truncate each decompiled function to n chars (default 20000)
//   func <addr>         decompile the function containing addr (creates one if missing)
//   refs <addr>         list references to addr and decompile each referencing function
//   refsonly <addr>     list references to addr only
//   callers <addr>      list callers of the function containing addr
//   callersdec <addr>   decompile every caller of the function containing addr
//   sym <name>          find symbols by exact name, list refs (following import thunks), decompile referrers
//   symonly <name>      same as sym without decompiling
//   vt <Class>          find Class::vftable, decompile referrers (ctors/dtors), dump the first slots
//   str <text>          find defined strings equal to text, decompile referrers
//   strre <regex>       find defined strings matching regex, list referrers only
//   scalar <hex> [mn]   list instructions with that scalar operand (optionally only mnemonic mn)
//
// @category ts3-online
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.scalar.*;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.regex.*;

public class Multi extends GhidraScript {
  DecompInterface di;
  PrintWriter out;
  Set<Function> done = new HashSet<>();
  int maxLen = 20000;

  void dec(Function f) {
    if (f == null || !done.add(f)) return;
    DecompileResults res = di.decompileFunction(f, 180, monitor);
    out.println("\n==== " + f.getName() + " @ " + f.getEntryPoint() + " size=" + f.getBody().getNumAddresses()
        + " callers=" + f.getCallingFunctions(monitor).size() + " ====");
    String c = res.decompileCompleted() ? res.getDecompiledFunction().getC() : ("decompile failed: " + res.getErrorMessage());
    out.println(c.length() > maxLen ? c.substring(0, maxLen) + "\n...TRUNCATED" : c);
  }

  List<Function> refs(Address ad, boolean print) {
    List<Function> fs = new ArrayList<>();
    for (Reference r : getReferencesTo(ad)) {
      Function f = getFunctionContaining(r.getFromAddress());
      if (print) out.println("   " + ad + " <- " + r.getFromAddress() + " " + r.getReferenceType() + " in "
          + (f == null ? "?" : f.getName() + "@" + f.getEntryPoint()));
      if (f != null && !fs.contains(f)) fs.add(f);
    }
    return fs;
  }

  Function containing(String arg) throws Exception {
    Address ad = toAddr(arg);
    Function f = getFunctionContaining(ad);
    return f != null ? f : createFunction(ad, null);
  }

  public void run() throws Exception {
    String[] a = getScriptArgs();
    out = new PrintWriter(new FileWriter(a[1]));
    di = new DecompInterface();
    di.openProgram(currentProgram);
    for (String line : Files.readAllLines(Paths.get(a[0]))) {
      line = line.trim();
      if (line.isEmpty() || line.startsWith("#")) continue;
      String[] parts = line.split("\\s+", 2);
      String cmd = parts[0], arg = parts.length > 1 ? parts[1] : "";
      out.println("\n######## " + line);
      try {
        switch (cmd) {
          case "maxlen": maxLen = Integer.parseInt(arg); break;
          case "func": dec(containing(arg)); break;
          case "refs": for (Function f : refs(toAddr(arg), true)) dec(f); break;
          case "refsonly": refs(toAddr(arg), true); break;
          case "callers":
            for (Function f : containing(arg).getCallingFunctions(monitor))
              out.println("   caller " + f.getName() + "@" + f.getEntryPoint());
            break;
          case "callersdec": for (Function f : containing(arg).getCallingFunctions(monitor)) dec(f); break;
          case "sym": case "symonly":
            for (Symbol s : currentProgram.getSymbolTable().getSymbols(arg)) {
              out.println(" sym " + s.getName(true) + " @" + s.getAddress() + " type=" + s.getSymbolType());
              List<Function> fs = refs(s.getAddress(), true);
              for (Reference r : getReferencesTo(s.getAddress()))
                if (getFunctionContaining(r.getFromAddress()) == null) fs.addAll(refs(r.getFromAddress(), true));
              if (cmd.equals("sym")) for (Function f : fs) dec(f);
            }
            break;
          case "vt": {
            SymbolIterator it = currentProgram.getSymbolTable().getSymbolIterator(true);
            while (it.hasNext()) {
              Symbol s = it.next();
              if (!s.getName().equals("vftable")) continue;
              Namespace ns = s.getParentNamespace();
              if (ns == null || !ns.getName().equals(arg)) continue;
              out.println(" vftable " + s.getName(true) + " @" + s.getAddress());
              for (Function f : refs(s.getAddress(), true)) dec(f);
              for (int i = 0; i < 64; i++) {
                Address slot = s.getAddress().add(i * 8L);
                if (i > 0 && getSymbolAt(slot) != null) break;
                long p = getLong(slot);
                Function vf = getFunctionAt(toAddr(p));
                if (vf == null && i > 0) break;
                out.println("   slot[" + i + "] +0x" + Long.toHexString(i * 8L) + " -> " + Long.toHexString(p) + " " + (vf == null ? "" : vf.getName()));
              }
            }
            break;
          }
          case "str": case "strre": {
            Pattern p = cmd.equals("strre") ? Pattern.compile(arg) : null;
            DataIterator it = currentProgram.getListing().getDefinedData(true);
            while (it.hasNext()) {
              Data d = it.next();
              if (!d.hasStringValue()) continue;
              String s = String.valueOf(d.getValue());
              if (p == null ? !s.equals(arg) : !p.matcher(s).find()) continue;
              out.println(" str @" + d.getAddress() + " \"" + s + "\"");
              List<Function> fs = refs(d.getAddress(), true);
              if (p == null) for (Function f : fs) dec(f);
            }
            break;
          }
          case "scalar": {
            String[] sp = arg.split("\\s+");
            long v = Long.decode(sp[0]);
            String mn = sp.length > 1 ? sp[1] : null;
            Set<Function> fs = new LinkedHashSet<>();
            InstructionIterator ii = currentProgram.getListing().getInstructions(true);
            while (ii.hasNext()) {
              Instruction in = ii.next();
              if (mn != null && !in.getMnemonicString().equalsIgnoreCase(mn)) continue;
              boolean hit = false;
              for (int op = 0; op < in.getNumOperands() && !hit; op++)
                for (Object o : in.getOpObjects(op))
                  if (o instanceof Scalar && ((Scalar) o).getUnsignedValue() == v) { hit = true; break; }
              if (!hit) continue;
              Function f = getFunctionContaining(in.getAddress());
              fs.add(f);
              out.println("   " + in.getAddress() + " " + in + " in " + (f == null ? "?" : f.getName()));
            }
            out.println(" total functions: " + fs.size());
            break;
          }
          default: out.println("ERR unknown command " + cmd);
        }
      } catch (Exception e) {
        out.println("ERR " + e);
      }
      out.flush();
    }
    out.close();
  }
}
