**RuffleVita** — play Flash games (`.swf`) on your PS Vita. Built on [Ruffle](https://ruffle.rs). Made by İbrahim Doğan.

### What's new in 1.2.0

**Faster heavy games.**

- **Physics and 3D engines are compiled ahead of time.** The Box2D physics engine and the Alternativa3D 3D engine now run as native code instead of being interpreted. Happy Wheels takes 103 ms per frame instead of 207 ms, about twice as fast, and 3D games built on Alternativa3D such as 3D Taxi Racing speed up too. Other games that use the same engine versions get the same boost automatically.
- **Physics speed.** A new per-game setting, **Physics speed: Fast**, lets physics games run a little looser in exchange for speed (Happy Wheels: from about 8 to 9–10 FPS on top of the above).
- **Much less drawing work.** Shapes with solid colours, bitmaps and gradients are now drawn in batches. For example, Bloons Tower Defense 3 went from 627 draw calls per frame to 85, and A Koopa's Revenge 2 from 233 to 59.
- **A faster interpreter.** In our tests, ActionScript 3 games do about 15% less work per frame and ActionScript 1/2 games about 6% less. Number conversions that went through a slow library call on the Vita are now a single instruction.

**More games work.**

- **Games that call dead web services keep going.** Many games call Kongregate, MindJolt, ad or score servers that no longer exist. Before, the failed call could stop a script halfway, so a button never reacted or a screen never moved on (for example the second **Continue** in Mega Drill). RuffleVita now answers the Kongregate and MindJolt APIs itself, and skips calls into missing objects instead of stopping the script.
- **Buttons inside mask layers can be clicked.** Some games draw a button as a mask, like the **Start** on Stick War's mission scroll. Flash lets you click those, and now RuffleVita does too.
- **Happy Wheels no longer crashes when a level loads.** Huge images that don't fit in the Vita's graphics memory are scaled down instead.
- **Very deep recursion no longer closes RuffleVita.** The game gets Flash's "stack overflow" error instead, which it can handle.
- **A damaged SWF in your library no longer crashes RuffleVita** while the library reads it.

**Saves.**

- Each game now keeps its saves in its own folder, `ux0:data/rufflevita/saves/<Game> [id]/`, instead of a deep tree named after its file path. Games that used the same save name no longer overwrite each other's saves.
- Your existing saves move over automatically the first time each game loads them. Nothing to do.
- Saves are also written every 30 seconds while you play, so turning the Vita off keeps your progress. A save is written to a temporary file first, so a power cut can't leave a half-written one.

**Library.**

- **Dark mode.** Turn it on in any game's settings under **Display → Dark mode (launcher)**. It applies to the library, Explore and the menus, not to games.
- **Better game names.** Many SWFs carry a placeholder title such as "Adobe Flex 4 Application", "Untitled-1" or the publisher's name. Those are now ignored and the file name is shown instead.

### Install

1. Download **`rufflevita.vpk`** below and install it with **VitaShell**. It updates RuffleVita in place.
2. Put your `.swf` files in **`ux0:data/FlashGames/`** (subfolders work), or find some in Explore (**R** in the library).
3. Launch **RuffleVita** from the home screen.

Requires a Vita with HENkaku / Ensō; Explore also needs Wi-Fi. Saves, settings and `log.txt` live in `ux0:data/rufflevita/`. Please attach `log.txt` to bug reports.

Games offered in Explore belong to their authors and are downloaded from Silvergames' public archive. RuffleVita is not affiliated with Silvergames.
