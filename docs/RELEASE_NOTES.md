# ts3-online 1.8.0

Online co-op for *Toy Story 3 Complete Edition* (Steam), with one setup program for Windows and
Linux/Steam Deck. Enemies now attack on every player's screen, not only the host's.

**1.8.0 does not play with 1.7.x.** Every player needs 1.8.0: the in-game window says so when the
host runs another version.

## Install

1. Download `ts3-online.zip` from this release and unzip it anywhere.
2. Run **`ts3-online-setup.exe`** on Windows, or **`ts3-online-setup`** on Linux/Steam Deck
   (Desktop Mode).
3. Choose **Host a game**, **Join a friend**, or **Choose each time**, fill in your name and the
   host details, then click **Install**.
4. Start the game from Steam. No launch options or scripts.

Already have ts3-online? With the game closed:

- **From 1.7.2:** run the setup, open **Updates, help and uninstall**, click **Check for updates**,
  then **Update to 1.8.0**.
- **From 1.7.0 or 1.7.1:** run the setup from this zip and click **Save**. The old setup's
  **Update** button brings only the mod and leaves the old setup program behind.

Your settings and saves are kept either way.

## What's new

- **Enemies attack on every screen.** A joined player's enemies used to slide around without
  swinging, and their hits landed anyway. The host now sends each enemy's current motion, so its
  charges, wind-ups, hit reactions and deaths play on every copy. Townspeople and companions are
  untouched.
- **Coming back as another character works.** A player who left and rejoined as another character
  stayed the old one on everyone else's screen. Now they get the character they picked; the old
  one stands idle until the level ends.
- **`local-coop.ps1` starts the second copy again.** Since 1.7.0 it looked for the host's log in
  the wrong folder and gave up after a minute.

## Requirements

- Windows 10/11 x64, or Linux/Steam Deck through Proton.
- The Steam version of *Toy Story 3 Complete Edition* on every PC.
- A controller or keyboard on every PC.
- The PCs on one home network, or [Tailscale](https://tailscale.com) (free for personal use). The
  setup offers to install it on Windows or opens its download page on Linux.
- ts3-online 1.8.0 on every PC.

The full guide is in `README.md` in the zip (docs/PLAYING.md).
