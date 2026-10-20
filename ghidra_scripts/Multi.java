// Headless Ghidra query runner: one command per line from a file, so shells never quote
// addresses, regexes or commas.
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
//   rangerefs <addr> <len>  list every reference into [addr, addr+len), grouped by target offset
//   riprefs <addr> <len>    one line per instruction referencing [addr, addr+len):
//                           RIPREF va len bytes target_offset  (input for tools/gen_patches.py)
//   asm <addr>          disassemble the function containing addr with raw bytes
//   ctx <addr> [before] [after]  disassemble around one instruction (default 8 / 12)
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

  static String hex(byte[] b) {
    StringBuilder sb = new StringBuilder();
    for (byte x : b) sb.append(String.format("%02x", x));
    return sb.toString();
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
          case "rangerefs": {
            String[] sp = arg.split("\\s+");
            Address start = toAddr(sp[0]);
            long len = Long.decode(sp[1]);
            Set<Function> fs = new LinkedHashSet<>();
            for (long i = 0; i < len; i++) {
              Address t = start.add(i);
              for (Reference r : getReferencesTo(t)) {
                Function f = getFunctionContaining(r.getFromAddress());
                fs.add(f);
                out.println("   +0x" + Long.toHexString(i) + " <- " + r.getFromAddress() + " " + r.getReferenceType()
                    + " in " + (f == null ? "?" : f.getName() + "@" + f.getEntryPoint()));
              }
            }
            out.println(" total functions: " + fs.size());
            break;
          }
          case "riprefs": {
            String[] sp = arg.split("\\s+");
            Address start = toAddr(sp[0]);
            long len = Long.decode(sp[1]);
            Set<Address> seen = new TreeSet<>();
            for (long i = 0; i < len; i++)
              for (Reference r : getReferencesTo(start.add(i))) {
                Instruction in = getInstructionAt(r.getFromAddress());
                if (in == null || !seen.add(in.getAddress())) continue;
                out.println("RIPREF " + in.getAddress() + " " + in.getLength() + " " + hex(in.getBytes()) + " 0x"
                    + Long.toHexString(i) + " ; " + in);
              }
            break;
          }
          case "asm": {
            Function f = containing(arg);
            out.println(" " + f.getName() + " @ " + f.getEntryPoint());
            InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
              Instruction in = ii.next();
              out.println(String.format("   %s %-24s %s", in.getAddress(), hex(in.getBytes()), in));
            }
            break;
          }
          case "ctx": {
            String[] sp = arg.split("\\s+");
            Instruction at = getInstructionAt(toAddr(sp[0]));
            int before = sp.length > 1 ? Integer.parseInt(sp[1]) : 8, after = sp.length > 2 ? Integer.parseInt(sp[2]) : 12;
            if (at == null) { out.println("ERR no instruction at " + sp[0]); break; }
            Function f = getFunctionContaining(at.getAddress());
            out.println(" in " + (f == null ? "?" : f.getName()));
            Instruction p = at;
            for (int k = 0; k < before && p.getPrevious() != null; k++) p = p.getPrevious();
            for (int k = 0; k < before + after + 1 && p != null; k++, p = p.getNext())
              out.println(String.format("  %s %s %-24s %s", p.getAddress().equals(at.getAddress()) ? ">" : " ",
                  p.getAddress(), hex(p.getBytes()), p));
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
