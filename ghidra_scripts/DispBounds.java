// For every load through a given displacement (e.g. [reg + 0xd8]), lists the small-immediate
// compares (1..3) in the preceding instructions of the same function, with raw bytes and the jump.
// Used to find inline `idx < 2` guards on per-player arrays that are not indexed by IMUL.
//
// args: <outfile> <disp> [window=24] [after=0]   after>0 also scans that many instructions after the load
//
// @category ts3-online
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.scalar.*;
import ghidra.program.model.lang.*;
import java.io.*;
import java.util.*;

public class DispBounds extends GhidraScript {
  public void run() throws Exception {
    String[] a = getScriptArgs();
    PrintWriter out = new PrintWriter(new FileWriter(a[0]));
    long disp = Long.decode(a[1]);
    int window = a.length > 2 ? Integer.parseInt(a[2]) : 24;
    int after = a.length > 3 ? Integer.parseInt(a[3]) : 0;
    Set<String> seen = new HashSet<>();
    InstructionIterator ii = currentProgram.getListing().getInstructions(true);
    while (ii.hasNext() && !monitor.isCancelled()) {
      Instruction in = ii.next();
      if (!in.getMnemonicString().startsWith("MOV")) continue;
      boolean hit = false;
      if ((in.getOperandType(1) & OperandType.DYNAMIC) != 0)
        for (Object o : in.getOpObjects(1))
          if (o instanceof Scalar && ((Scalar) o).getUnsignedValue() == disp) hit = true;
      if (!hit) continue;
      Function f = getFunctionContaining(in.getAddress());
      if (f == null) continue;
      List<Instruction> cand = new ArrayList<>();
      Instruction q = in;
      for (int k = 0; k < window; k++) { q = q.getPrevious(); if (q == null || !f.getBody().contains(q.getAddress())) break; cand.add(q); }
      q = in;
      for (int k = 0; k < after; k++) { q = q.getNext(); if (q == null || !f.getBody().contains(q.getAddress())) break; cand.add(q); }
      for (Instruction p : cand) {
        if (!p.getMnemonicString().equals("CMP")) continue;
        Object[] last = p.getOpObjects(p.getNumOperands() - 1);
        if (last.length != 1 || !(last[0] instanceof Scalar)) continue;
        long v = ((Scalar) last[0]).getUnsignedValue();
        if (v < 1 || v > 3) continue;
        if (!seen.add(p.getAddress().toString())) continue;
        StringBuilder b = new StringBuilder();
        for (byte x : p.getBytes()) b.append(String.format("%02x", x));
        Instruction j = p.getNext();
        out.println(f.getName() + " " + p.getAddress() + " " + b + "  " + p + "  ; " + (j == null ? "" : j.toString())
            + "  -> load " + in.getAddress() + " " + in);
      }
    }
    out.close();
  }
}
