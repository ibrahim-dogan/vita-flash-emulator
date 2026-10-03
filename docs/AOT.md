# Hot-path AOT for AS3

RuffleVita / İbrahim Doğan. Status: working. The Box2D physics in Happy Wheels (336 methods) and the Alternativa3D engine in 3D Taxi Racing (215 methods) are compiled, and both games give results identical to the interpreter.

Ruffle runs ActionScript 3 in an interpreter. In physics-heavy and 3D games almost all of the Vita's time goes into one library of methods. The AOT compiles chosen methods into Rust that is built into RuffleVita. Every other method, and every method whose code doesn't match, is interpreted as before.

| Timedemo (desktop instructions per frame) | Interpreter | AOT |
|---|---|---|
| Happy Wheels ride | 53.5M | 21.3M |
| 3D Taxi Racing race | 142M | 58.6M |

On the Vita, the Happy Wheels tick went from 207 ms to 103 ms per frame.

## How it works

- **The input is the optimized op stream.** The translator reads `parsed_code`, which Ruffle's verifier and type-aware optimizer have already processed. Slot numbers, vtable dispatch ids, fused branches and direct native calls are already resolved there.
- **Locals and the stack become Rust variables, typed where possible.**
  - A dataflow pass types every stack slot and local as a number or as anything.
  - Numbers stay unboxed as `(f64, is_int)` and are boxed only where a `Value` is needed. The flag mirrors exactly when the interpreter would hold a `Value::Integer`.
  - Basic blocks run from a `loop { match block { … } }`. Backward branches still check the script timeout.
- **Hot ops are compiled natively.** These include slots, locals, arithmetic, comparisons, branches, `Math.*`, `Coerce`, array index access and `ConstructSlot`. They use the same fast paths as the interpreter (`aot/rt.rs`).
- **Calls between compiled methods are direct.** `CallMethod` calls the callee without the operand stack. Callees that need nothing of their own activation ("leaves", such as `b2Math.b2Max`) run on the caller's activation, without a new stack frame or argument setup.
- **Other ops go to the interpreter's own handlers.** For property lookup by name, calls by name, construction and the like, the generated code calls the same `op_*` handler (`Activation::aot_execute`), so behaviour is identical by construction.
- **The op stream must match exactly.** Each compiled method records a fingerprint of the op stream and parameter types it was made from (`aot::canonical`). The first time a method is called, its runtime op stream must produce the same fingerprint, or the method is interpreted. The same library in another game (for example the same Box2D or Alternativa3D version) also matches.
- **Some methods are skipped.** These are methods with try/catch, and methods with an op the translator doesn't handle.

## Regenerating `generated.rs`

1. Find the hot methods with a profiling build:
   - Vita: `RUFFLEVITA_FEATURES=prof_ops ./build.sh`.
   - Desktop: `cargo build --release --features prof_ops`.
   - Run with `RUFFLEVITA_PROF=1` plus a timedemo (`RUFFLEVITA_BENCH`). The method list is in the log.
2. Run each game on the desktop with the translator. `RUFFLEVITA_AOT_METHODS` is `|`-separated, and `prefix*` matches every method starting with `prefix`:

```bash
RUFFLEVITA_AOT_EMIT=/tmp/gen_game.rs \
RUFFLEVITA_AOT_METHODS='Box2D.*|alternativa.*' \
RUFFLEVITA_GAMES=… cargo run --release
```

3. Merge the files from all games: `emulator/aot/merge.pl emulator/patches/ruffle/core/src/avm2/aot/generated.rs /tmp/gen_*.rs`. The union is keyed by fingerprint, so a method compiled for two games appears once.
4. Rebuild. `RUFFLEVITA_NO_AOT=1` turns the compiled code off, for A/B comparisons.

## Checking it

Use the timedemo (`RUFFLEVITA_BENCH`, `at <frame>` script steps), with and without `RUFFLEVITA_NO_AOT`. In timedemo mode `getTimer`, `Date` and `Math.random` are deterministic, so screenshots of the same frames must be identical. Any difference in a physics step shows up within a few frames.

## Second reference: 3D Taxi Racing

Timedemo script (single race, track 1, driving on the first lap; 57.8M instructions per frame with AOT, 140.6M with `RUFFLEVITA_NO_AOT=1` over frames 2300-2700, screenshots at 2500 and 2700 identical):

```
RUFFLEVITA_AUTOSTART="3d Taxi Racing.swf" RUFFLEVITA_BENCH=2300-2700
RUFFLEVITA_SCRIPT="at 200; tap 680 345; at 600; tap 755 343; at 700; tap 755 297; at 900; tap 233 320; at 1500; tap 650 500; at 2100; hold Up 2800; at 2500; shot a.png; at 2700; shot b.png; at 2710; quit"
```
