// Finds small player-count compares applied to a player index that does not come from a slot IMUL
// (those are SlotBounds.java's job): the result of a call to a player-index getter, or a direct
// read of the actor's player-index field.
//
// For each source it follows the value forward through register moves for a few instructions and
// prints every `CMP reg, 1|2|3` on it with the conditional jump that follows.
//
// args: <outfile> <getter-addr>[+<getter-addr>...] [field=0x484] [window=14]
//
// @category ts3-online
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.scalar.*;
import ghidra.program.model.lang.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;

public class IndexChecks extends GhidraScript {
  Listing listing;
  PrintWriter out;
  int window;

  String bytes(Instruction in) throws Exception {
    StringBuilder sb = new StringBuilder();
    for (byte b : in.getBytes()) sb.append(String.format("%02x", b));
    return sb.toString();
  }

  /** 64-bit base name of a register, or null. */
  String base(Object o) {
    if (!(o instanceof Register)) return null;
    Register r = (Register) o;
    while (r.getParentRegister() != null && r.getParentRegister().getBitLength() <= 64) r = r.getParentRegister();
    return r.getName();
  }

  void follow(Instruction start, String reg, String what) throws Exception {
    Set<String> live = new HashSet<>();
    live.add(reg);
    Function f = getFunctionContaining(start.getAddress());
    Instruction in = start.getNext();
    for (int i = 0; i < window && in != null && !live.isEmpty(); i++, in = in.getNext()) {
      if (f != null && !f.getBody().contains(in.getAddress())) break;
      String mn = in.getMnemonicString();
      if (mn.equals("CMP") && in.getNumOperands() == 2) {
        Object[] a = in.getOpObjects(0), b = in.getOpObjects(1);
        if (a.length == 1 && live.contains(base(a[0])) && b.length == 1 && b[0] instanceof Scalar) {
          long v = ((Scalar) b[0]).getValue();
          if (v >= 1 && v <= 3) {
            Instruction j = in.getNext();
            out.println(String.format("site %s %s in %s @ %s", what, start.getAddress(), f == null ? "?" : f.getName(),
                f == null ? "?" : f.getEntryPoint()));
            out.println(String.format("  cmp %s %s  %s   ; %s %s %s", in.getAddress(), bytes(in), in,
                j.getAddress(), bytes(j), j));
          }
        }
      }
      if (mn.equals("CALL") || mn.startsWith("RET") || mn.equals("JMP")) break;
      // track simple moves; any other write to a live register kills it
      Object[] dst = in.getNumOperands() > 0 ? in.getOpObjects(0) : new Object[0];
      String d = dst.length == 1 ? base(dst[0]) : null;
      if (d == null || (in.getOperandType(0) & OperandType.DYNAMIC) != 0) continue;
      if (mn.equals("MOV") || mn.equals("MOVSXD") || mn.equals("MOVZX")) {
        Object[] src = in.getOpObjects(1);
        boolean fromLive = src.length == 1 && live.contains(base(src[0])) && (in.getOperandType(1) & OperandType.DYNAMIC) == 0;
        if (fromLive) live.add(d); else live.remove(d);
      } else if (!mn.equals("CMP") && !mn.equals("TEST") && !mn.startsWith("J")) {
        live.remove(d);
      }
    }
  }

  @Override
  public void run() throws Exception {
    String[] args = getScriptArgs();
    listing = currentProgram.getListing();
    out = new PrintWriter(new FileWriter(args[0]));
    long field = args.length > 2 ? Long.decode(args[2]) : 0x484;
    window = args.length > 3 ? Integer.parseInt(args[3]) : 14;

    for (String g : args[1].split("[+~]")) {
      Address ga = toAddr(Long.parseUnsignedLong(g.replaceFirst("^0x", ""), 16));
      out.println("== getter " + ga);
      for (Reference r : getReferencesTo(ga)) {
        if (!r.getReferenceType().isCall()) continue;
        Instruction call = getInstructionAt(r.getFromAddress());
        if (call != null) follow(call, "RAX", "call");
      }
    }

    out.println("== field +0x" + Long.toHexString(field));
    InstructionIterator it = listing.getInstructions(true);
    while (it.hasNext() && !monitor.isCancelled()) {
      Instruction in = it.next();
      if (!in.getMnemonicString().equals("MOV") || in.getNumOperands() != 2) continue;
      if ((in.getOperandType(1) & OperandType.DYNAMIC) == 0) continue;
      boolean hit = false;
      for (Object o : in.getOpObjects(1))
        if (o instanceof Scalar && ((Scalar) o).getUnsignedValue() == field) hit = true;
      if (!hit) continue;
      Object[] dst = in.getOpObjects(0);
      String d = dst.length == 1 ? base(dst[0]) : null;
      if (d != null) follow(in, d, "field");
    }
    out.close();
  }
}
