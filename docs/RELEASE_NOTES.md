# ts3-online 1.7.1

Online co-op for *Toy Story 3 Complete Edition* (Steam), with one setup program for Windows and
Linux/Steam Deck. This release fixes the 1.7.0 setup, which could delete saves.

## Install

1. Download `ts3-online.zip` from this release and unzip it anywhere.
2. Run **`ts3-online-setup.exe`** on Windows, or **`ts3-online-setup`** on Linux/Steam Deck
   (Desktop Mode).
3. Choose **Host a game**, **Join a friend**, or **Choose each time**, fill in your name and the
   host details, then click **Install**.
4. Start the game from Steam. No launch options or scripts.

Already on 1.7.0? Run the setup from this zip on every PC, not **Update to 1.7.1** in the old
setup: that button replaces only the mod and leaves 1.7.0's setup, the one that deletes saves, in
the game folder. Your settings are kept. The online code is unchanged, so 1.7.0 and 1.7.1 still
play together.

## What's fixed

- **Uninstall keeps your progress.** 1.7.0's uninstall deleted the saves of copies 2 to 4
  (`ts3-online/copy2` to `copy4`) while the window said your saves stay. Those folders and
  `save backups` now always stay; your settings and the games you joined stay unless you tick
  **Also delete my settings and the games I joined**.
- **Upgrading from 1.6.0 or older keeps your progress.** 1.7.0 deleted `%LOCALAPPDATA%\ts3online`
  with the copies' saves and the joined game's save still inside. The setup now copies them into
  `<game>/ts3-online` first and removes the old folder only once that worked. If a folder of the
  same name is already there, it copies nothing and leaves the old folder alone.
- **Uninstall removes the old firewall rule.** 1.6.0 could add a Windows Firewall rule
  "ts3-online (UDP)" that 1.7.0 left behind. Uninstall now removes it, asking for administrator
  rights.
- **The guide no longer promises a firewall option** in the setup: Windows asks the first time you
  host.
- **No more pointers to removed scripts.** The game's message for a version mismatch, its log and
  `local-coop.ps1` told players to run `Update.cmd` or `Setup.cmd`, which 1.7.0 removed; they now
  point at the setup.

If you already ran 1.7.0's uninstall, or ran 1.7.0's setup on a PC that had an older version, the
saves of copies 2 to 4 and of the games you joined may be gone, and 1.7.1 cannot bring them back.
Your own (copy 1) save was never touched, and its backups are in `ts3-online/save backups`.

## Requirements

- Windows 10/11 x64, or Linux/Steam Deck through Proton.
- The Steam version of *Toy Story 3 Complete Edition* on every PC.
- A controller or keyboard on every PC.
- The PCs on one home network, or [Tailscale](https://tailscale.com) (free for personal use). The
  setup offers to install it on Windows or opens its download page on Linux.
- The same ts3-online version on every PC.

The full guide is in `README.md` in the zip (docs/PLAYING.md).
