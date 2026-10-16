// Lists every Kernel::CommandLineArgument the game registers and whether any code reads it.
// A flag object referenced only by its registrar and atexit destructor is dead.
//
// args: <outfile> [ctorAddr]   ctorAddr defaults to the CommandLineArgument ctor in the
//                              build documented in docs/FINDINGS.md
//
// @category ts3-online
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import java.io.*;

public class FlagUse extends GhidraScript {
  public void run() throws Exception {
    String[] a = getScriptArgs();
    PrintWriter out = new PrintWriter(new FileWriter(a[0]));
    Address ctor = toAddr(a.length > 1 ? a[1] : "140248e50");
    for (Reference r : getReferencesTo(ctor)) {
      Function f = getFunctionContaining(r.getFromAddress());
      if (f == null) continue;
      String name = "?", obj = "?";
      InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
      while (ii.hasNext()) {
        Instruction in = ii.next();
        if (in.getAddress().compareTo(r.getFromAddress()) >= 0) break;
        if (!in.getMnemonicString().equals("LEA")) continue;
        Reference[] rs = in.getReferencesFrom();
        if (rs.length == 0) continue;
        String reg = in.getDefaultOperandRepresentation(0);
        Address t = rs[0].getToAddress();
        if (reg.equals("RCX")) obj = t.toString();
        if (reg.equals("RDX")) {
          Data d = getDataAt(t);
          name = d != null && d.hasStringValue() ? String.valueOf(d.getValue()) : t.toString();
        }
      }
      int uses = 0;
      StringBuilder sb = new StringBuilder();
      if (!obj.equals("?")) {
        for (Reference u : getReferencesTo(toAddr(obj))) {
          Function uf = getFunctionContaining(u.getFromAddress());
          if (uf == null || uf.equals(f)) continue;
          uses++;
          sb.append(" ").append(uf.getName());
        }
      }
      out.println(String.format("%-22s obj=%s uses=%d%s", name, obj, uses, sb));
    }
    out.close();
  }
}
