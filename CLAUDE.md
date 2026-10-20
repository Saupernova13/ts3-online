# ts3-online

Reverse engineering *Toy Story 3 Complete Edition* toward 4-player online co-op.

## Rules

- **Document as you go.** Each discovery goes into `docs/FINDINGS.md` with its address and an
  [obs]/[inf]/[unk] tag, and each session gets an entry in `docs/PROGRESS.md`. Update
  `docs/PLAN.md` when the approach or phase status changes. Commit after each step.
- **Patches go through the generator.** Add hand-reviewed patches and hooks to
  `data/patches-manual.json`, then run `tools/gen_patches.py`; it rewrites `loader/patches.json`
  and `docs/PATCHES.md`. Update a group's status in `tools/patch_catalog.py` after testing it.
- **No game data in git.** No exe, DLLs, assets, Ghidra project or full decompiler dumps; those
  live in `work/` (gitignored). Short decompiled excerpts in docs are fine.
- **Never modify the game install.** Patches are applied at runtime by our loader or to a copy.
  The live exe already carries someone else's 7-byte patch; the analysed program is the
  original (`.exe.orig` in the install folder).
- Addresses are for the exe whose SHA-256 is in `docs/FINDINGS.md`. A Steam update invalidates
  them; patch code should locate code by byte signature, not hardcoded addresses.

## Code standards

These hold for every change to `loader/` and `tools/`. A refactor that breaks one is not done.

- **One responsibility per module and type.** A file is either pure logic (testable on any
  platform, no game memory, no Win32) or glue (`#[cfg(windows)] mod live`, `win.rs`, `game.rs`,
  `actor.rs`), never both in one function. New sync features follow the pattern of `wallet.rs`,
  `world.rs`, `npc.rs`: pure state machine on top, `mod live` at the bottom, `net::session`
  delegating to a small type (`WorldSync`, `HostWallet`) rather than growing fields.
- **One owner per fact.** Game offsets and addresses live in `game.rs` (functions, globals) and
  `actor.rs` (actor layout); wire bits in `net::proto` (`flags`, message layouts); geometry in
  `math.rs`; PE fields in `pe.rs`; hook names in `hooks::table`. Grep for an existing constant
  before writing a hex literal. The same rule applies to tooling: one generator
  (`tools/gen_patches.py`) writes `loader/patches.json` and `docs/PATCHES.md`.
- **Depend on the narrow thing.** Game-thread modules call the `net::` facade, never the session
  or its lock; `net` never calls back into the game. Keys cross module boundaries as `NetKey`,
  not `String`. Pass what a function needs, not a whole struct, unless three or more fields go
  together (then name the struct, as `coop::Frame`).
- **Fail loudly, in the log.** Every error path that changes what the player gets (a patch not
  applied, a save folder not redirected, a role not started) writes one line to `ts3online.log`
  saying what happened and what the game does instead. No `let _ =` or `.ok()` on a result that
  matters. No `assert!`/`unwrap` in code that runs inside the game: the build aborts on panic.
- **Behaviour-preserving refactors are separate commits** from behaviour changes, each with its
  own conventional-commit message. A structure change touches no test assertion; a behaviour
  change adds or changes a test that fails on the old code.
- **Comments say intent, not mechanics.** One line, only where the code does not say it:
  invariants, reverse-engineered facts, why a deviation exists. No narration, no history.
  Address and offset literals in code carry the name of what they are, as the tables in
  `FINDINGS.md` do.
- **Keep it as simple as the problem.** No trait where one implementation exists, no newtype
  for a value that never crosses a boundary, no module under 20 lines. Split when a file mixes
  two reasons to change, not when it is long.
- **CI is the compiler.** `cargo test` runs on every push and pull request. A change that was
  not built locally says so in its PR, and is merged only once CI is green.

## Analysis

- Ghidra project: `work/ghidra/ts3.gpr` (create it once with `tools/ghidra.sh import`). Needs
  Ghidra 12.x (`GHIDRA_INSTALL_DIR`) and a JDK 21+ (`JAVA_HOME`).
- Run queries with `tools/ghidra.sh query queries/NN-name.cmd`; add a new numbered query file
  per investigation so the evidence can be reproduced.
- Each headless run costs ~1 minute of startup: batch related commands into one query file.
