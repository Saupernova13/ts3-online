// Decompiles every function referencing the player manager (pointer global or static object),
// plus, transitively, every callee passed it as param_1, for tools/scan_player_mgr.py.
//
// args: <outfile> [ptrGlobal] [objStart] [objLen]
//
// @category ts3-online
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;
import java.util.regex.*;

public class PlayerMgrDump extends GhidraScript {
  public void run() throws Exception {
    String[] a = getScriptArgs();
    PrintWriter out = new PrintWriter(new FileWriter(a[0]));
    Address ptr = toAddr(a.length > 1 ? a[1] : "140b2ef50");
    Address obj = toAddr(a.length > 2 ? a[2] : "1409582b0");
    int objLen = a.length > 3 ? Integer.decode(a[3]) : 0x400;
    String ptrName = "DAT_" + ptr.toString().replaceFirst("^0+", "");

    DecompInterface di = new DecompInterface();
    di.openProgram(currentProgram);

    // kind: G = reads the pointer global or the object, M = receives the manager as param_1
    Map<Function, String> kind = new LinkedHashMap<>();
    Deque<Function> work = new ArrayDeque<>();
    for (Reference r : getReferencesTo(ptr)) add(kind, work, getFunctionContaining(r.getFromAddress()), "G");
    for (int i = 0; i < objLen; i++)
      for (Reference r : getReferencesTo(obj.add(i))) add(kind, work, getFunctionContaining(r.getFromAddress()), "G");

    Pattern assign = Pattern.compile("(\\w+) = (?:\\(\\w+\\s*\\*?\\))?(" + Pattern.quote(ptrName) + "|&DAT_" + obj.toString().replaceFirst("^0+", "") + ");");
    int n = 0;
    while (!work.isEmpty() && n < 2500 && !monitor.isCancelled()) {
      Function f = work.poll();
      n++;
      DecompileResults res = di.decompileFunction(f, 180, monitor);
      if (!res.decompileCompleted()) {
        out.println("\n==== " + f.getName() + " @ " + f.getEntryPoint() + " kind=" + kind.get(f) + " mgr= FAILED");
        continue;
      }
      String c = res.getDecompiledFunction().getC();
      Set<String> names = new LinkedHashSet<>();
      if (kind.get(f).contains("G")) { names.add(ptrName); names.add("&DAT_" + obj.toString().replaceFirst("^0+", "")); }
      if (kind.get(f).contains("M")) names.add("param_1");
      Matcher m = assign.matcher(c);
      while (m.find()) names.add(m.group(1));
      out.println("\n==== " + f.getName() + " @ " + f.getEntryPoint() + " kind=" + kind.get(f) + " mgr=" + String.join(",", names));
      out.println(c);
      for (String nm : names) {
        Pattern call = Pattern.compile("(FUN_[0-9a-f]+)\\(\\s*(?:\\(\\w+\\s*\\*?\\)\\s*)?" + Pattern.quote(nm) + "\\s*[,)]");
        Matcher cm = call.matcher(c);
        while (cm.find()) {
          Function callee = getFunctionAt(toAddr(cm.group(1).substring(4)));
          add(kind, work, callee, "M");
        }
      }
    }
    out.println("\n#### functions=" + kind.size() + " decompiled=" + n);
    out.close();
  }

  void add(Map<Function, String> kind, Deque<Function> work, Function f, String k) {
    if (f == null) return;
    String old = kind.get(f);
    if (old == null) { kind.put(f, k); work.add(f); }
    else if (!old.contains(k)) { kind.put(f, old + k); work.add(f); }
  }
}
