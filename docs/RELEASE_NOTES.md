# ts3-online 1.7.0

Online co-op for *Toy Story 3 Complete Edition* (Steam), with one setup program for Windows and
Linux/Steam Deck.

## Install

1. Download `ts3-online.zip` from this release and unzip it anywhere.
2. Run **`ts3-online-setup.exe`** on Windows, or **`ts3-online-setup`** on Linux/Steam Deck
   (Desktop Mode).
3. Choose **Host a game**, **Join a friend**, or **Choose each time**, fill in your name and the
   host details, then click **Install**.
4. Start the game from Steam. No launch options or scripts.

The setup finds the game through Steam, backs up your saves into the game folder's
`ts3-online/save backups`, installs the mod and saves your settings. Run it again later to change
anything or to uninstall.

## What's new

- **One setup program** for Windows and Linux/Steam Deck replaces `Setup.cmd`, `Update.cmd`,
  `Uninstall.cmd` and `install.ps1`.
- **No launch options on Linux.** The mod is installed as `steam_api64.dll` as well as
  `dinput8.dll`, which Proton loads from the game folder by itself. Steam's own DLL is kept as
  `ts3-online/steam_api64_o.dll`.
- **Everything ts3-online writes lives in `<game>/ts3-online`**: settings, logs, backups, the
  per-copy saves and the setup program. Copy or move the game folder and the mod keeps working.
  Your own (copy 1) save stays in the game's normal save folder, and the setup backs it up into
  the game folder.
- **No runtime to install.** The Windows programs link the C runtime statically, so there is no
  Visual C++ redistributable to install. On Linux the setup needs only glibc 2.35+ (SteamOS 3,
  Ubuntu 22.04 or newer); if it cannot open a window (no desktop or GPU), it asks the same
  questions in the terminal (`--text` forces this).
- **Join at start.** The game joins the saved host by itself and remembers past hosts; the window
  opens only when that game does not answer.
- **Linux process check.** Setup and uninstall now refuse while the game is running, using the
  same check as Windows.

## Requirements

- Windows 10/11 x64, or Linux/Steam Deck through Proton.
- The Steam version of *Toy Story 3 Complete Edition* on every PC.
- A controller or keyboard on every PC.
- The PCs on one home network, or [Tailscale](https://tailscale.com) (free for personal use). The
  setup offers to install it on Windows or opens its download page on Linux.
- The same ts3-online version on every PC.

The full guide is in `README.md` in the zip (docs/PLAYING.md).
