# ts3-online 1.8.2

Online co-op for *Toy Story 3 Complete Edition* (Steam), with one setup program for Windows and
Linux/Steam Deck. This release fixes what the 2026-10-23 playtest found in story mode and Toy Box.

**1.8.2 plays with 1.8.0 and 1.8.1.** Players on 1.8.x can join each other; players on 1.7.x still
need to update. The fixes below work fully once every player has 1.8.2.

## Install

1. Download `ts3-online.zip` from this release and unzip it anywhere.
2. Run **`ts3-online-setup.exe`** on Windows, or **`ts3-online-setup`** on Linux/Steam Deck
   (Desktop Mode).
3. Choose **Host a game**, **Join a friend**, or **Choose each time**, fill in your name and the
   host details, then click **Install**.
4. Start the game from Steam. No launch options or scripts.

Already have ts3-online? With the game closed:

- **From 1.7.2 or 1.8.x:** run the setup, open **Updates, help and uninstall**, click **Check for
  updates**, then **Update to 1.8.2**.
- **From 1.7.0 or 1.7.1:** run the setup from this zip and click **Save**. The old setup's
  **Update** button brings only the mod and leaves the old setup program behind.

Your settings and saves are kept either way.

## What's fixed

- **Story mode together.** Friends waiting on the story board now follow the host into the
  chapter it picks, and the host's level waits for them to load. A friend who finishes a level
  first waits on the board for the host's next one.
- **Switching characters.** Switching between Woody, Buzz and Jessie in a story level no longer
  fills the level with copies of your character on your friends' screens, and no longer blocks
  anyone from switching.
- **Coins and points count.** Coins and imagination points a friend picks up are no longer taken
  back by the host's counter a second later. A crate a friend breaks gives its coins once.
- **Lifting your partner.** Picking up a co-op partner no longer fails on another screen after
  character switches.

Still open: a townsperson a friend carries can still look left behind on other screens (#68),
and a friend's menus can still open on your screen (#69).

## Requirements

- Windows 10/11 x64, or Linux/Steam Deck through Proton.
- The Steam version of *Toy Story 3 Complete Edition* on every PC.
- A controller or keyboard on every PC.
- The PCs on one home network, or [Tailscale](https://tailscale.com) (free for personal use). The
  setup offers to install it on Windows or opens its download page on Linux.
- ts3-online 1.8.x on every PC (1.8.2 on every PC for the fixes above).

The full guide is in `README.md` in the zip (docs/PLAYING.md).
