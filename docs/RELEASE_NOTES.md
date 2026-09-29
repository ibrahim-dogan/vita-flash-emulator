**RuffleVita** — play Flash games (`.swf`) on your PS Vita. Built on [Ruffle](https://ruffle.rs). Made by İbrahim Doğan.

### FlashVita is now RuffleVita

Version 1.0.1 renames the app from FlashVita to RuffleVita. It installs as a new app, and the first time it starts it copies your settings, per-game profiles, game saves and covers from `ux0:data/flashvita/` into `ux0:data/rufflevita/`. After that you can delete the old FlashVita bubble. Your games stay in `ux0:data/FlashGames/`.

### What's new in 1.0.1

- **Big games no longer crash on start.** Games with thousands of shapes (such as A Koopa's Revenge 2) ran the Vita out of graphics memory while loading.
- **Much faster ActionScript 1/2 games with many objects.** Looking up objects by name, building object references and property lookups are much cheaper. A Koopa's Revenge 2 went from about 3 to about 12 FPS.
- **Games that draw into huge bitmaps no longer crash when a level starts.** Happy Wheels now loads and plays. Bitmaps a game draws into use about half the memory, and drawing into them no longer piles up graphics memory.
- **Long level setups finish.** A script can now run for up to two minutes on the Vita before it is stopped, so games that build a whole level in one go work.
- **Less work when the mouse isn't used.** The game only looks for what's under the cursor while you're using the touch screen, a stick cursor or a mouse button.
- **More detailed FPS counter.** It now also shows how long each frame spends in the game's code, drawing and waiting for the GPU, plus free memory. While it's on, the same line is written to `log.txt` every 10 seconds.
- Running out of memory is written to `log.txt` before the app closes.

Heavy ActionScript 3 games, especially physics games such as Happy Wheels, are still slow: the Vita's CPU is many times slower than a PC's.

### Install

1. Download **`rufflevita.vpk`** below and install it with **VitaShell**.
2. Create the folder **`ux0:data/FlashGames/`** and copy your `.swf` files into it (subfolders work).
3. Launch **RuffleVita** from the home screen and pick a game.

If a game needs extra files (levels, external `.swf`s, `.xml`), keep them in the same folder as the game.

Requires a Vita with HENkaku / Ensō. Saves, settings and `log.txt` live in `ux0:data/rufflevita/`. Please attach `log.txt` to bug reports.
