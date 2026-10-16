// Finds instruction-level patch candidates for raising MAX_PLAYERS.
//
// For every `IMUL r, r, <stride>` whose result feeds a memory operand with a displacement in
// the player-slot window within the next few instructions, prints the nearby compares against
// small player-count immediates (1, 2, 3) together with their conditional jump and raw bytes.
// Also lists every instruction whose displacement is one of the fields after the slot array.
//
// args: <outfile> [stride=0x58] [slotLo=0x198] [slotHi=0x248] [tailOffsets=0x248+0x250+0x258]
//
// @category ts3-online
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.scalar.*;
import ghidra.program.model.lang.*;
import java.io.*;
import java.util.*;

public class SlotBounds extends GhidraScript {
  Listing listing;

  long disp(Instruction in) {
    // largest scalar in any memory operand, used as the displacement
    long best = -1;
    for (int op = 0; op < in.getNumOperands(); op++) {
      if ((in.getOperandType(op) & OperandType.DYNAMIC) == 0) continue;
      for (Object o : in.getOpObjects(op))
        if (o instanceof Scalar) best = Math.max(best, ((Scalar) o).getUnsignedValue());
    }
    return best;
  }

  String bytes(Instruction in) throws Exception {
    StringBuilder sb = new StringBuilder();
    for (byte b : in.getBytes()) sb.append(String.format("%02x", b));
    return sb.toString();
  }

  boolean isSmallCmp(Instruction in) {
    String mn = in.getMnemonicString();
    if (!mn.equals("CMP")) return false;
    Object[] o = in.getOpObjects(in.getNumOperands() - 1);
    if (o.length != 1 || !(o[0] instanceof Scalar)) return false;
    long v = ((Scalar) o[0]).getUnsignedValue();
    return v >= 1 && v <= 3;
  }

  public void run() throws Exception {
    String[] a = getScriptArgs();
    PrintWriter out = new PrintWriter(new FileWriter(a[0]));
    long stride = a.length > 1 ? Long.decode(a[1]) : 0x58;
    long lo = a.length > 2 ? Long.decode(a[2]) : 0x198;
    long hi = a.length > 3 ? Long.decode(a[3]) : 0x248;
    Set<Long> tail = new HashSet<>();
    for (String s : (a.length > 4 ? a[4] : "0x248+0x250+0x258").split("[+]")) tail.add(Long.decode(s));
    listing = currentProgram.getListing();

    Map<Function, List<String>> hits = new LinkedHashMap<>();
    Set<String> cmpSeen = new HashSet<>();
    int accesses = 0;
    InstructionIterator ii = listing.getInstructions(true);
    while (ii.hasNext() && !monitor.isCancelled()) {
      Instruction in = ii.next();
      if (!in.getMnemonicString().equals("IMUL")) continue;
      Object[] last = in.getOpObjects(in.getNumOperands() - 1);
      if (last.length != 1 || !(last[0] instanceof Scalar) || ((Scalar) last[0]).getUnsignedValue() != stride) continue;
      // does a slot-window displacement follow?
      Instruction n = in;
      long hitDisp = -1;
      for (int k = 0; k < 8 && n != null; k++) {
        n = n.getNext();
        if (n == null) break;
        long d = disp(n);
        if (d >= lo && d < hi) { hitDisp = d; break; }
      }
      if (hitDisp < 0) continue;
      accesses++;
      Function f = getFunctionContaining(in.getAddress());
      List<String> lines = hits.computeIfAbsent(f, k -> new ArrayList<>());
      lines.add("  access " + in.getAddress() + " " + in + "  -> " + n.getAddress() + " " + n);
      // scan this function only for small compares near this access
      AddressSetView body = f == null ? null : f.getBody();
      Instruction p = in;
      for (int k = 0; k < 40; k++) {
        Instruction q = p.getPrevious();
        if (q == null || body == null || !body.contains(q.getAddress())) break;
        p = q;
      }
      for (int k = 0; k < 90 && p != null && body != null && body.contains(p.getAddress()); k++, p = p.getNext()) {
        if (!isSmallCmp(p) || !cmpSeen.add(f.getEntryPoint() + ":" + p.getAddress())) continue;
        Instruction j = p.getNext();
        lines.add("    cmp " + p.getAddress() + " " + bytes(p) + "  " + p + "   ; " + (j == null ? "" : j.getAddress() + " " + j));
      }
    }
    out.println("indexed slot accesses: " + accesses + " in " + hits.size() + " functions");
    for (Map.Entry<Function, List<String>> e : hits.entrySet()) {
      Function f = e.getKey();
      out.println("\n" + (f == null ? "?" : f.getName() + " @ " + f.getEntryPoint()));
      for (String s : e.getValue()) out.println(s);
    }

    out.println("\n== tail field accesses");
    ii = listing.getInstructions(true);
    while (ii.hasNext() && !monitor.isCancelled()) {
      Instruction in = ii.next();
      long d = disp(in);
      if (!tail.contains(d)) continue;
      Function f = getFunctionContaining(in.getAddress());
      out.println("  " + in.getAddress() + " " + bytes(in) + "  " + in + "   in " + (f == null ? "?" : f.getName()));
    }
    out.close();
  }
}
