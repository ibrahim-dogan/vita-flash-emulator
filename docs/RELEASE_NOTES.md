**RuffleVita** — play Flash games (`.swf`) on your PS Vita. Built on [Ruffle](https://ruffle.rs). Made by İbrahim Doğan.

### What's new in 1.1.0

**Explore.** Press **R** in the library to browse about 6,000 Flash games from the [Silvergames](https://www.silvergames.com) archive and download them over Wi-Fi.

- The catalog downloads the first time you open Explore and is saved on the memory card. It refreshes once a week, or when you press **SELECT**.
- **△** searches with the Vita's keyboard, **○** clears the search and **□** sorts by name, newest or smallest.
- Rest on a game for a moment and RuffleVita reads its first 16 KB to show its ActionScript version, stage size and frame rate before you download it. Games that still have cover art on Silvergames show it; the rest get a cover made from their name.
- **✕** downloads the game into `ux0:data/FlashGames/` with a progress bar. Press **✕** again to cancel. Downloaded games are marked **In library**, and **✕** plays them right away.
- Games over 20 MB are marked as large, since some won't fit in the Vita's memory.

**A new look.** The whole interface was redrawn in the style of the old Flash game portals: bold outlines, hard shadows, flat colours, and the Lilita One and Nunito typefaces. The LiveArea artwork was updated to match.

**Controls.** **L/R** now switch between Library and Explore. Paging through a list moved to the D-pad's left and right.

### Install

1. Download **`rufflevita.vpk`** below and install it with **VitaShell**. It updates RuffleVita 1.0.1 in place.
2. Put your `.swf` files in **`ux0:data/FlashGames/`** (subfolders work), or find some in Explore.
3. Launch **RuffleVita** from the home screen.

Coming from FlashVita 1.0.0? RuffleVita installs next to it and copies your settings, profiles, saves and covers from `ux0:data/flashvita/` the first time it starts.

Requires a Vita with HENkaku / Ensō; Explore also needs Wi-Fi. Saves, settings and `log.txt` live in `ux0:data/rufflevita/`. Please attach `log.txt` to bug reports.

Games offered in Explore belong to their authors and are downloaded from Silvergames' public archive. RuffleVita is not affiliated with Silvergames.
