**RuffleVita** — play Flash games (`.swf`) on your PS Vita. Built on [Ruffle](https://ruffle.rs). Made by İbrahim Doğan.

### What's new in 1.3.0

**Home screen bubbles.**

- **Give any game its own bubble.** In a game's **Options → Home screen**, pick *Add to the home screen*. The bubble uses the game's cover as its round icon and LiveArea background, and **Start** goes straight into the game.
- **Installed for you.** With **Enable Unsafe Homebrew** on (Settings → HENkaku Settings), RuffleVita installs the bubble itself in a few seconds and comes right back. With it off, RuffleVita saves a VPK in `ux0:data/rufflevita/bubbles/` and shows how to install it with VitaShell.
- Bubbles follow dark mode, share your saves and settings with RuffleVita, and take under 10 MB each. Add them again after updating RuffleVita to bring them up to date.
- **Direct launch from other launchers.** HexFlow, RetroFlow and Custom Bubbles can open a game with `psgm:play?titleid=RUFFLEVIT&param=ux0:data/FlashGames/<game>.swf`.

**More games work.**

- **Space and letter keys in older games.** Games that wait for a key with `on (keyPress ...)` now react to the buttons bound to it, for example **Press SPACE** in Electric Man 2 and Murder. A letter the game listens for in the other case works too, like **C** for the car in Stick RPG.
- **Text boxes no longer break the picture.** Drawing a text box's border or cursor turned the rest of the frame into a wireframe, for example on Stick RPG's name, bank and stock screens.
- **PixelBender effects no longer stop a game.** It runs on without that effect.
- **Games packed with Apparat's Matryoshka** unpack in under a second instead of timing out.

**Smaller and steadier.**

- **Less memory.** Games are unpacked while they're read instead of being held twice, sounds no longer copy the game's data, and the fallback font only loads when a game needs it. Electric Man 2 needs 35 MB instead of 44, A Koopa's Revenge 2 58 MB instead of 73.
- **Running out of memory ends the game, not RuffleVita.** You're back in the library with a message instead of a crash.
- **A smaller download.** RuffleVita itself is now under 10 MB instead of 27.

**Library.**

- **New games show up by themselves.** Copy `.swf` files over FTP or USB while RuffleVita is open and they appear in the library within a few seconds; no refresh needed.
- **Options has a General tab** for what applies everywhere: V-Sync, dark mode and refreshing the Explore catalog.

### Install

1. Download **`rufflevita.vpk`** below and install it with **VitaShell**. It updates RuffleVita in place.
2. Put your `.swf` files in **`ux0:data/FlashGames/`** (subfolders work), or find some in Explore (**R** in the library).
3. Launch **RuffleVita** from the home screen.

Requires a Vita with HENkaku / Ensō; Explore also needs Wi-Fi. Saves, settings and `log.txt` live in `ux0:data/rufflevita/`. Please attach `log.txt` to bug reports.

Games offered in Explore belong to their authors and are downloaded from Silvergames' public archive. RuffleVita is not affiliated with Silvergames.
