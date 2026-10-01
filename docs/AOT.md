# Hot-path AOT for AS3

RuffleVita / İbrahim Doğan. Status: prototype. Happy Wheels' 30 hottest Box2D methods are compiled and give the same results as the interpreter.

Ruffle runs ActionScript 3 in an interpreter. In physics-heavy games almost all of the Vita's time goes into a few dozen methods. For example, in Happy Wheels 30 Box2D methods take about 70% of the frame. The AOT compiles exactly those methods into Rust that is built into RuffleVita. Every other method, and every method whose code doesn't match, is interpreted as before.

## How it works

- **The input is the optimized op stream.** The translator reads `parsed_code`, which Ruffle's verifier and type-aware optimizer have already processed. Slot numbers, vtable dispatch ids, fused branches and direct native calls (`CallNative`) are resolved there, so the generated code needs no lookups of its own.
- **Locals and the stack become Rust variables.** Locals are `l0…` and the operand stack is `s0…`. The stack depth at every op is known statically. Basic blocks run from a `loop { match block { … } }`. Backward branches still check the script timeout.
- **Hot ops are compiled natively.** These are slots, locals, arithmetic, comparisons, branches, `Math.sin/cos/sqrt/…` (as `f64::sin` and so on) and returns. They use the same fast paths as the interpreter (`aot/rt.rs`).
- **Other ops go to the interpreter's own handlers.** For calls, property access, `Coerce` to classes, construction and the like, the generated code pushes the operands onto the activation's stack and calls the same `op_*` handler (`Activation::aot_execute`). Behaviour is therefore identical by construction.
- **Runtime handles come from the method's own ops.** Scripts, multinames and classes are read from `ops[i]` at runtime. Generated code embeds no pointers.
- **The op stream must match exactly.** Each compiled method records a fingerprint of the op stream it was made from (`aot::canonical`): the op stream as text, without pointers, with natives named. The first time a method is called, its runtime op stream must produce the same fingerprint, or the method is interpreted. A method with the same code in another game (for example the same Box2D version) also matches, and is still correct.
- **Some methods are skipped.** These are methods with try/catch, and methods with an op the translator doesn't handle.

## Regenerating `generated.rs`

1. Find the hot methods on the Vita with a profiling build:
   - Build with `RUFFLEVITA_FEATURES=prof_ops ./build.sh`.
   - Run with `RUFFLEVITA_PROF=1` plus a timedemo; the method list is in the log.
2. Run the game on the desktop with the translator:

```bash
RUFFLEVITA_AOT_EMIT=$PWD/emulator/patches/ruffle/core/src/avm2/aot/generated.rs \
RUFFLEVITA_AOT_METHODS='Box2D.Dynamics.Contacts::b2ContactSolver/SolvePositionConstraints()|…' \
RUFFLEVITA_GAMES=… cargo run --release
```

Each listed method is translated when the game first runs it, and the file is rewritten each time. The log says which methods were compiled and why others were skipped.

3. Rebuild. `RUFFLEVITA_NO_AOT=1` turns the compiled code off, for A/B comparisons.

## Checking it

Use the timedemo (`RUFFLEVITA_BENCH`, `at <frame>` script steps), with and without `RUFFLEVITA_NO_AOT`. Screenshots of the same frames must be identical: any difference in a physics step shows up within a few frames.
