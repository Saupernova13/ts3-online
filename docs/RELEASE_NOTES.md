# ts3-online 1.7.2

Online co-op for *Toy Story 3 Complete Edition* (Steam), with one setup program for Windows and
Linux/Steam Deck. Updating now installs the whole release, the setup program included.

## Install

1. Download `ts3-online.zip` from this release and unzip it anywhere.
2. Run **`ts3-online-setup.exe`** on Windows, or **`ts3-online-setup`** on Linux/Steam Deck
   (Desktop Mode).
3. Choose **Host a game**, **Join a friend**, or **Choose each time**, fill in your name and the
   host details, then click **Install**.
4. Start the game from Steam. No launch options or scripts.

Already have ts3-online? With the game closed, run the setup from this zip on every PC and click
**Save**: it installs 1.7.2, the setup program in the game folder included, and keeps your settings
and saves. Do not use **Update** in a 1.7.0 or 1.7.1 setup: it brings only the mod and leaves the
old setup program behind, and doing this with 1.7.1's zip only changed the settings. From 1.7.2 on,
**Update** brings both. The online code is the same as 1.7.0's, so 1.7.0, 1.7.1 and 1.7.2 play
together.

## What's fixed

- **Updating installs the new setup too.** **Update** in the setup (and `--update`) replaced only
  the mod, so the setup program in the game folder kept the old release's bugs, 1.7.0's
  save-deleting uninstall among them. It now installs the mod and both setup programs from the
  release zip, and refuses a zip that does not hold all three.
- **A newer setup upgrades an installed game.** Running the setup from a newer zip and clicking
  **Save** (or `--apply`, or `--text`) changed only the settings; it now installs that release
  first. An older setup never replaces a newer install, and the settings page says which will
  happen.
- **Safer updates.** Settings are saved even when an update step fails, a repair never puts an
  older mod back, the mod's DLLs are never left half-written, a release unzipped straight into the
  game folder is installed, and saves from before 1.7 are moved in on update but never while the
  game runs.
- From 1.7.1, for anyone still on 1.7.0: uninstall keeps every copy's progress, upgrading from
  1.6.0 or older keeps it too, and uninstall removes the firewall rule 1.6.0 added.

If you ran 1.7.0's uninstall, or ran 1.7.0's setup on a PC that had an older version, the saves of
copies 2 to 4 and of the games you joined may be gone, and no later release can bring them back.
Your own (copy 1) save was never touched, and its backups are in `ts3-online/save backups`.

## Requirements

- Windows 10/11 x64, or Linux/Steam Deck through Proton.
- The Steam version of *Toy Story 3 Complete Edition* on every PC.
- A controller or keyboard on every PC.
- The PCs on one home network, or [Tailscale](https://tailscale.com) (free for personal use). The
  setup offers to install it on Windows or opens its download page on Linux.
- ts3-online on every PC, from releases that play together (1.7.0 to 1.7.2 all do; the in-game
  window says when the host's does not).

The full guide is in `README.md` in the zip (docs/PLAYING.md).
