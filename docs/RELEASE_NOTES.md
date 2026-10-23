# ts3-online 1.8.1

Online co-op for *Toy Story 3 Complete Edition* (Steam), with one setup program for Windows and
Linux/Steam Deck. This release fixes what a review of the whole mod found: safer saves and
updates, a clearer setup, and steadier online play.

**1.8.1 plays with 1.8.0.** Players on 1.8.0 and 1.8.1 can join each other; players on 1.7.x still
need to update.

## Install

1. Download `ts3-online.zip` from this release and unzip it anywhere.
2. Run **`ts3-online-setup.exe`** on Windows, or **`ts3-online-setup`** on Linux/Steam Deck
   (Desktop Mode).
3. Choose **Host a game**, **Join a friend**, or **Choose each time**, fill in your name and the
   host details, then click **Install**.
4. Start the game from Steam. No launch options or scripts.

Already have ts3-online? With the game closed:

- **From 1.7.2 or 1.8.0:** run the setup, open **Updates, help and uninstall**, click **Check for
  updates**, then **Update to 1.8.1**.
- **From 1.7.0 or 1.7.1:** run the setup from this zip and click **Save**. The old setup's
  **Update** button brings only the mod and leaves the old setup program behind.

Your settings and saves are kept either way.

## What's fixed

- **The setup notices a running game.** On Windows it never did, so it could install, update or
  move old saves while Toy Story 3 was open. It now asks you to close the game first.
- **Your own save keeps your own gold.** A player who joins but plays their own save (the host had
  no save, the host's save took too long to arrive, or two automatic hosts merged) had the host's
  gold, imagination points and progress written into it. Nothing of the host's is written into a
  save that is not the host's any more.
- **No spending the same gold twice.** Right after buying something in a friend's game, the old
  gold could show for about a second and be spent again.
- **Save backups.** A backup that failed halfway counted as that day's backup, so a second try the
  same day went ahead with half a backup. A backup only counts once it is complete.
- **Setup screens.** **Install Tailscale** on the first page no longer jumps to "All set" without
  installing anything, and works before the game folder is found. Uninstall results are shown.
  The port box can be cleared and retyped.
- **Enemies in a friend's game.** An enemy attack that arrived before the enemy was ready to play
  it now plays as soon as it can, instead of waiting for the next one. A character that appears
  later in a level is no longer taken for an enemy that was there before.
- **Smoother when a player leaves.** On Windows, every "nobody is listening" reply from a departed
  player's PC briefly held up the other players' network messages until that player timed out.

## Requirements

- Windows 10/11 x64, or Linux/Steam Deck through Proton.
- The Steam version of *Toy Story 3 Complete Edition* on every PC.
- A controller or keyboard on every PC.
- The PCs on one home network, or [Tailscale](https://tailscale.com) (free for personal use). The
  setup offers to install it on Windows or opens its download page on Linux.
- ts3-online 1.8.0 or 1.8.1 on every PC.

The full guide is in `README.md` in the zip (docs/PLAYING.md).
