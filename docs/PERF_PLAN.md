# Performance plan: where the Vita's time goes and what to do about it

RuffleVita / İbrahim Doğan. Written 2026-10-02 after profiling the current working tree (AOT for Box2D and Alternativa3D in place, nothing committed yet). This is a consultant's note: the measurements, the diagnosis, and an ordered list of tasks with the expected gain of each, so that the implementation can be done step by step by someone (or some model) who hasn't seen the profile. Record results in the table at the end as each task lands.

## 1. The numbers

Happy Wheels ride, desktop timedemo (`RUFFLEVITA_BENCH=800-1400`, the reference script from `docs/AOT.md` / the test notes). Instruction counts are the Vita proxy; desktop milliseconds are not.

| | Interpreter | AOT (now) |
|---|---|---|
| Desktop instructions per frame | 53.5M | 21.1M |
| Vita tick | 207 ms | 103 ms |

Sanity check on the proxy: 21M instructions in 103 ms is ~0.46 instructions per cycle on the Vita's 444 MHz Cortex-A9. That is normal for pointer-chasing code; nothing Vita-specific is obviously broken (but see task T4 about malloc, which is the one place the Vita likely differs a lot from macOS).

### 1.1 Where the frame goes (native profile)

`sample` on the macOS release binary during frames 800–3000 of the ride (1 ms interval, main thread, ~1215 samples inside RuffleVita code). Self time by category:

| Category | Share | What it is |
|---|---|---|
| AOT-generated code itself (`m_*`, `l_*`, inlined `rt::*`) | 52% | The compiled Box2D: slot reads/writes through `Value`, boxed numbers, `rt::get_slot`/`set_slot_no_coerce`, `t_mul` with a `Value` operand, array index access |
| Method call machinery | 15% | `function::exec` (highest single self-time symbol), `Avm2::push_call`/`pop_call`, `Method::get_verified_info`, `Value::call_method_with_args`, `coerce_to_type_slow` |
| Garbage collector | 8–9% | `gc_arena::Context::do_collection`, `Collect::trace` |
| malloc / free | 8% | Almost entirely `new b2Vec2`-style object churn in Box2D |
| Object / property lookups | 7% | `get_index_property` (Array `[i]`), `set_slot_no_coerce`, `PropertyMap::get_for_multiname`, `scriptobject_allocator` |
| Display list and render | 5% | `enter_frame`, `construct_frame`, command list execution |
| Interpreter (`run_actions`) | <2% | AOT coverage for this game is essentially complete |

The `prof_ops` build agrees: `avm2 aot` 48% self, `gc` 8.6%, `avm2 code` (interpreted) 2.6%. The `present` zone (26%) on the desktop is vsync waiting, not work; on the Vita render + present is 5–7 ms of the ~103 ms frame.

Hottest AS3 functions by self time (prof_ops): `b2Island/Solve` 7.6%, `b2ContactSolver/SolvePositionConstraints` 6.0%, `b2Collision/b2CollidePolygons` 3.9%, `b2BroadPhase/MoveProxy` 3.8%, `b2Distance/DistanceGeneric` 3.6%, `b2ContactSolver/SolveVelocityConstraints` 2.2%, `b2PolygonContact/Evaluate` 2.1%.

### 1.2 The call-graph shape that matters

The inclusive call graph shows every call between compiled methods going through the full interpreter entry:

```
m_b2World/Step
  l_… (leaf)                      ← direct, cheap
  Value::call_method_with_args    ← slow path
    function::exec                ← Activation::from_nothing + init_from_method
      m_b2World/Solve
        Value::call_method_with_args
          function::exec
            m_b2Island/Solve
              ClassObject::construct_with_args → call_init → function::exec   (new b2Vec2 …)
```

Of the ten hottest compiled methods, **only `b2World/Step` has a leaf variant**. `Solve`, `b2Island/Solve`, `SolvePositionConstraints`, `SolveVelocityConstraints`, `b2CollidePolygons`, `DistanceGeneric`, `b2PolygonShape/Support`, `Evaluate` are all non-leaves, so each call to them pays `exec`: stack-overflow check, `get_stack_frame`, `Activation::from_nothing`, `init_from_method` (which calls `resolve_info` and `verify` every time, even though both are cached), argument coercion through `FunctionArgs`, `push_call`/`pop_call`, `cleanup`. `b2PolygonShape/Support` is called from the innermost loop of GJK distance and pays all of that per call.

Why they aren't leaves (`translate.rs`, `let leaf = …`): a method is a leaf only if it has no `GetPropertyFast`/`SetPropertyFast` (array indexing like `m_vertices[i]`), no `LookupSwitch`/`Throw`, and no op that is delegated to an interpreter handler. Box2D indexes arrays everywhere, so nearly all of it is non-leaf. Counts in `generated.rs`: 1318 `rt::call_method` sites, 1006 `rt::delegate` sites, 438 `rt::get_property_fast`, 379 `rt::construct_slot`.

## 2. Diagnosis

1. **The remaining cost is structural, not coverage.** Interpreted code is under 3% of the frame. Adding more methods to the AOT table will not help Happy Wheels. Making the compiled code and its calls cheaper will.
2. **Call overhead is the cheapest big win.** 15% of the frame is call setup that a direct-call path can mostly skip, and part of the 52% "AOT self" is also entry/exit work (`act.local_register(k)` reads, stack frame init).
3. **Object churn costs ~20% in three places at once**: `malloc`/`free` (8%), GC (8.6%), `scriptobject_allocator` + constructor calls through `exec`. Each `new b2Vec2()` is a `Gc::new` of a large `ScriptObjectData` (a `RefLock<DynamicMap>`, a boxed slots slice, a `RefLock<Vec>` of bound methods, proto, class, vtable: two heap allocations) plus a constructor call through the full `exec` path.
4. **Slot access is generic.** `rt::get_slot(act, value, k)` does a null check, a `Value → Object` match, a bounds check and a `Lock` read, and `t_mul` with a `Value` operand must inspect the tag. The verifier knows the declared type of every slot trait (`Number`, `int`, `b2Vec2`…), and the translator doesn't use it.
5. **Rendering is not the bottleneck on the Vita** (5–7 ms of 103). Don't spend time there for FPS. The texture-memory issue on level load is a crash problem, listed separately in §5.

Realistic expectation: tasks T1–T4 together should take Happy Wheels from ~21M to roughly 12–15M instructions per frame (Vita ~60–75 ms, 13–16 FPS). 30 FPS in Happy Wheels needs either T5 (typed object layout, a big job) or the game-specific lever in T7. Say so to the user rather than promising 30 FPS.

## 3. Tasks, in order

Each task: what, why, how, expected gain, how to verify. Do them in this order; T1 and T2 are independent of each other but both touch `function.rs`/`rt.rs`, so land one before starting the other.

### T1. Direct calls into non-leaf compiled methods (a "light frame")

**Gain: ~10–15% of the frame.** Largest, lowest risk.

Today `rt::call_method` tries `call_leaf` and otherwise falls back to `receiver.call_method_with_args(...)`, which reaches `function::exec`. Add a middle path, `call_compiled`, for a callee whose method has an AOT entry but no leaf:

- Resolve the callee exactly as `call_leaf` does (`object.get_bound_method(index).is_none()`, `vtable().get_full_method(index)`, `aot::lookup(method)`), with the same bail-outs (`rv_stack::exhausted()`, more than `MAX_ARGS` args, arity mismatch, rest args; bail to the slow path, never error).
- Take a stack frame with `stack.get_stack_frame(method)` (a pointer bump plus clearing `num_locals` values; this is needed because delegated ops use the operand stack of the activation).
- Instead of `Activation::from_nothing` + `init_from_method`, swap the fields of the **current** activation the way `aot_run_leaf` does, plus `stack`, `num_locals`, `scope_depth`, `bound_superclass_object` (= `full_method`'s class if the method can `CallSuper`; simplest: bail to the slow path when the method contains `CallSuper`/`GetSuper`/`SetSuper`, found once at table-generation time and stored as a flag in `Entry`), and `is_interpreter = false`. Write `this` and the coerced arguments into the new frame's locals (same coercion loop as `call_leaf`). Call `compiled(act, ops)`. Restore everything, including the stack pointer (what `cleanup` does), also on the error path. Keep `push_call`/`pop_call` so stack traces and the script timeout behave the same.
- Skip `resolve_info` and `verify`: a method with an AOT entry has already been verified. `aot::resolve` (called from `aot::lookup` the first time `exec` runs the method, i.e. after `init_from_method` verified it) reads `get_verified_info()` and stores the table index in `aot_slot`. So in the light path only use `aot::lookup` when `method.aot_slot() != 0`; a method with `aot_slot() == 0` has never been called and must go through the slow path once (which verifies it and resolves the slot). Never call `get_verified_info()` on a method that may be unverified.
- Do the same for constructors: `rt::construct_slot` → `ClassObject::construct_with_args` → `call_init` → `exec`. The allocation stays; the constructor call should go through the same light path when the init method is compiled. (`b2Vec2`'s constructor is a tiny method called millions of times.)

Also reduce fixed entry cost in generated non-leaf methods: they start with `act.local_register(k)` for every local; with a direct path the caller can pass the argument slice like leaves do (`args: &[Value]`), so the generated `m_*` can take `(act, ops, args)` and read parameters from `args` and only *other* locals from the frame. Measure before doing this second part; it changes the `AotFn` type everywhere.

Verify: Happy Wheels instr/frame down, screenshots byte-identical with and without `RUFFLEVITA_NO_AOT`; also run 3D Taxi Racing and two non-AOT games to be sure nothing else changed. Check a method that throws inside a light frame unwinds correctly (the `?` paths must restore the activation; write it as a guard struct or a closure with a single restore point).

### T2. Cheaper slot access and typed slots in the translator

**Gain: ~5–10%.** Medium effort, in `translate.rs` and `rt.rs` only (fingerprints don't change: they hash the op stream, not the output).

- **Typed slot reads.** When translating `GetSlot { index }` on a receiver whose class is known (the optimizer resolved the slot, so the receiver's `Class` is known at translation time; `Class::instance_vtable().slot_class(index)` or the trait's declared type gives the slot type), and the declared type is `Number`/`int`/`uint`/`Boolean`, read it as a typed value: `rt::get_slot_num(act, obj, k) -> Num` doing the unbox inline, and keep the result in `(n, i)` registers instead of a `Value`. For `SetSlot` with a numeric declared type, the coercion is known and `set_slot_no_coerce` with `boxn` is already right; make sure the translator picks that path. Check first how many `GetSlot` sites would qualify: `grep -c 'rt::get_slot(' generated.rs` versus how many feed `t_mul`/`t_add` directly.
- **Known-object receivers.** `l0` (`this`) is never null and always an `Object` in an instance method; locals that come straight from `Coerce` to a class type are `Object` or `Null`. Give the dataflow a `Ty::Object` kind and emit `rt::get_slot_obj(obj: Object, k)` (no `null_check`, no `as_object` match) when the receiver is `this`. Null receivers must still throw TypeError 1009 exactly as the interpreter does, so only skip the check where nullness is impossible.
- **Array indexing.** `GetPropertyFast` with an integer index on a known `Array` ends in `get_index_property`. Add a fast path in `rt::get_property_fast` for `Value::Object(Array)` + integer key that reads the dense storage directly (there is already an array fast path in the interpreter's `op_get_property_fast`; make sure the AOT uses it and that bounds/holes return `Undefined` the same way).

Verify as T1. This is where semantic slips hide (an `int` slot holding `Value::Integer` vs. `Value::Number`; the `is_int` flag must mirror the interpreter exactly). The screenshot comparison catches it.

### T3. Allocation: make `new b2Vec2()` cheap

**Gain: ~5–8%** on macOS, probably more on the Vita where newlib's malloc is slower.

- **Smaller `ScriptObjectData`.** Make `values` (the dynamic property map) lazily allocated (`RefLock<Option<Box<DynamicMap>>>` or an empty map that doesn't allocate until first insert; check that `DynamicMap::new()` doesn't allocate) and `bound_methods` lazy too. Then a sealed-class instance is one `Gc::new` plus the slots box. Going further, store slots inline for small slot counts (`SmallVec<[Lock<Value>; 4]>` or a tail-allocated layout) so a `b2Vec2` is a single allocation.
- **GC pacing.** `Player::run_frame` calls `collect_debt()` once per frame (`player.rs` near line 2420). First measure: `gc_arena.metrics().total_gc_count()` and `total_gc_allocation()` before and after the bench window give collections per frame and bytes allocated per frame; print them on the BENCH line. Then tune `gc_arena.metrics().set_pacing(Pacing { .. })` (gc-arena 0.5.3, git rev `08e0841`; `Pacing` is in its `src/metrics.rs`): a larger `sleep_factor` means the collector sleeps longer after each cycle, so fewer collections per frame; `min_sleep` prevents restarts on a small heap. Trade-off on the Vita is memory: Happy Wheels sits at ~150 MB of 240 MB heap mid-level. Double `sleep_factor` first and watch the "heap A/240 MB" line in the Vita log and Bloons TD5 (239 MB peak).
- Don't attempt escape analysis or stack-allocating `b2Vec2` in the translator; it's a large project with a small chance of landing cleanly.

### T4. Vita only: the allocator

**Gain: unknown, possibly large on the Vita; zero on macOS.** Cheap to try.

8% of the macOS frame is in `libsystem_malloc`, which is fast. The Vita binary uses newlib's malloc through `NEWLIB_HEAP_SIZE_USER` (240 MB), which takes a lock on every call and has a slow free-list implementation. Try a `#[global_allocator]` on the Vita target: the `dlmalloc` crate configured for a fixed region (the current 240 MB newlib heap can be shrunk and the region given to dlmalloc), or a simple size-class bump/free-list allocator for small sizes (≤128 bytes) on top of the existing malloc. Measure with the Happy Wheels Vita script with and without; the autorun/`RUFFLEVITA_BENCH` line gives tick ms/frame directly. If the gain is under 5%, revert; don't keep an allocator for its own sake.

Related: `main.rs` has a `heapcount` feature that wraps the global allocator; it is a ready place to count allocations per frame.

### T5. 3D Taxi Racing on the Vita

**Gain: unknown; this is first a measurement.** The Alternativa3D AOT table (215 methods, 142M → 58.6M on desktop) has not been run on the Vita.

1. Write a timedemo script for Taxi (menu taps to reach a race, then `hold Up`), make it reproducible on desktop with identical screenshots AOT on/off, and add it to `docs/AOT.md` as the second reference measurement.
2. Run it on the Vita with and without `RUFFLEVITA_NO_AOT=1`, write both tick numbers in the table below.
3. Profile with `sample` the same way as above. Expect the same shape (call overhead and per-vertex `Vector3D`-style object churn), so T1–T3 should carry over. If the profile shows something different (e.g. `Vector.<Number>` access or `BitmapData` upload), note it here before working on it.

### T6. Keep `check_gpu_room` and the texture budget separate from FPS work

Not an FPS task, but it's the open crash: Happy Wheels level load creates eight textures of ~20–27 MB and exhausts all three vitaGL pools (log: `VRAM 5 · RAM 5 · PHY 3 free`). Two cheap moves: give vitaGL more by cutting `NEWLIB_HEAP_SIZE_USER` now that the heap needs are smaller (measure the peak heap of Bloons TD5 first, which is the largest at 239 MB), and downscale any texture over 2048 px per side when memory is short (Ruffle never frees them). Verify `check_gpu_room` actually fires by faking a low `gpu_free`.

### T7. Game-specific lever (ask the user before doing this)

Box2D's `b2World.Step(dt, velocityIterations, positionIterations)`: `SolvePositionConstraints` + `SolveVelocityConstraints` + `b2Island/Solve` are ~16% self time and scale linearly with the iteration counts. A "performance mode" setting that clamps the iteration counts passed to `Step` (e.g. 10/10 → 4/3) would cut the frame noticeably with visibly looser physics. It changes game behaviour, so it must be opt-in per game and off in timedemos. Only do this if the user wants it.

## 4. Working method (for whoever implements this)

- Build on the Mac (`emulator/`): `export PATH=$HOME/.rustup/toolchains/nightly-aarch64-apple-darwin/bin:$PATH; DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo build --release`. Profiling build: add `--features prof_ops --target-dir target/profops`.
- Always `RUFFLEVITA_HIDDEN=1`. Reference run (Happy Wheels):
  `RUFFLEVITA_GAMES=<swf dir> RUFFLEVITA_DATA=<empty dir> RUFFLEVITA_AUTOSTART=HappyWheels.swf RUFFLEVITA_BENCH=800-1400 RUFFLEVITA_SCRIPT="at 330; tap 770 328; at 480; tap 670 430; at 630; tap 560 160; at 750; hold Up 1350; at 1410; quit"`. Read `M instr/frame` from the BENCH line. Run three times; the count is stable to ±0.5%.
- Correctness gate for every change under `avm2/`: `shot` at two frames (e.g. `at 900; shot a.png; at 1400; shot b.png`) with and without `RUFFLEVITA_NO_AOT=1`, compare with `cmp`. Any byte differs → bug, stop.
- Native profile: `sample <pid> 3 1 -mayDie`. Note that `shot` writes its file at the end of the run, so use a log line as the trigger (the last `Drawing into 2297x2000 texture` line marks the end of level load) rather than waiting for a screenshot file.
- Vita A/B: `autorun.txt` with `RUFFLEVITA_AUTOSTART`, `RUFFLEVITA_BENCH`, `RUFFLEVITA_SCRIPT`, optional `RUFFLEVITA_NO_AOT=1`; `nosleep on; screen on` first; read the BENCH line from `ux0:data/rufflevita/log.txt`. A Docker build takes ~20 minutes, so batch Vita runs: only go to the Vita after a desktop win of ≥3% instructions, or for T4/T5 which can only be measured there.
- Don't commit unless asked. Don't add Claude attribution anywhere.

## 5. Results log

| Date | Task | Happy Wheels M instr/frame | Vita tick ms | Taxi M instr/frame | Notes |
|---|---|---|---|---|---|
| 2026-10-02 | baseline (AOT) | 21.1 | 103 | 58.6 (desktop only) | profile in §1 |
| 2026-10-02 | T1 light frame (reverted) | 20.69 (−1.8%) | not measured | not measured | Implemented as designed: `exec` and `rt::call_method` run a compiled non-leaf on the caller's activation (field swap + sized stack frame, `LightInfo` cached on the method so no `body()`/`is_variadic()` lookups per call; hit ~7M times in the ride). Screenshots identical. Gain is under the 3% bar, so reverted. Dropping `push_call`/`pop_call` on the direct path was worth only another 0.6%. The 15% "call machinery" in the native profile is time (cache misses, dependent loads), not instructions: the old `exec` path was already only ~2% more instructions than the swap path. The args-slice change (`AotFn` taking `args`) would save a few loads per call, not 10%; not worth the type change. Call cost is now a floor; the leverage is in what the callee does (T2 slot access, T3 allocation). |
| 2026-10-02 | T2 (partial): direct paths in `rt::get_slot`/`set_slot_no_coerce` for `Object::ScriptObject`, and in `rt::get_property_fast` for `ArrayObject`/`VectorObject` (each type calls its own method, no dispatch on the `Object` enum) | 20.43 (−3.0%) | not measured | not measured | Screenshots identical to `RUFFLEVITA_NO_AOT=1`. Small, safe change in `rt.rs` only. The typed-slot part of T2 (reading `Number` slots straight into `(f64, is_int)`) was not done: `t_mul`/`t_lt` already take a `Value` operand and unbox it inline, so the expected gain is small against a translator change plus a regeneration of `generated.rs`. The same trick in `call_leaf` (`get_bound_method`/`vtable`) made it 0.4% worse and was dropped. |
| 2026-10-02 | T3 GC pacing (not applied) | 20.43 → 20.37 / 20.06 / 19.98 | not measured | not measured | `sleep_factor` 1 / 2 / 4 (default 0.5): −0.3% / −1.8% / −2.2%. The GC is only about 3% of the instructions here, and live heap in the ride is 30-48 MB, so a factor of 2 can add tens of MB of garbage on a device that sits at 150 of 240 MB. Not worth the memory risk. Allocation itself (one `Gc::new` plus one slots `Box` per object) stays as is: shrinking it means changing `ScriptObjectData` for every object type, and the gain is probably only visible on the Vita (see T4). |

| 2026-10-03 | Taxi reference script | – | – | 57.8 AOT / 140.6 no AOT | Script in `docs/AOT.md`; screenshots identical AOT on/off. |
| 2026-10-03 | first Vita run of the T2 build | – | crashed at level load | – | Happy Wheels hit the T6 crash: three huge offscreen textures left `PHY 0 free`, then a small bitmap registration was refused and Ruffle panicked (`Failed to register bitmap`, `bitmap_data.rs:864`). Fix: textures of 4 MB or more now need 40 MB free after them (was 12 MB). |
| 2026-10-03 | T6 crash fix (applied) | – | HW now loads & plays | – | GPU memory exhaustion no longer panics: `register_bitmap` downscales a bitmap that won't fit (`clamp_bitmap_memory`), and `BitmapData::bitmap_handle` falls back to a 1x1 texture instead of `expect`-panicking. Heap kept at 240 MB (192 broke Bloons; vitaGL's RAM pool is separate from newlib's heap, so shrinking the heap didn't help). HappyWheels went from "crashes at level load" to playable with correct graphics (the blanked huge textures are off-screen/redundant caches; the visible level renders). |
| 2026-10-03 | T7 physics cap (applied, opt-in) | 20.54 → 17.23 (cap 3) / 16.16 (cap 1) desktop | HW 113.9 → 99.6 ms (cap 3) | – | `RUFFLEVITA_PHYS_ITERS=N` caps b2World.Step's iteration args (this Box2D is 2.0's 2-arg `Step(dt, iterations)`). Detected by method name, cached on `Method`. Off by default; behaviour identical when off (screenshot gate passes). Vita HW ~8 → ~9-10 FPS. Gain is bounded because much of HW's physics cost is collision detection, which doesn't scale with iterations. Still an env var; needs a per-game UI toggle to be user-facing. |
| 2026-10-03 | Broad library survey (Vita) | – | see notes | – | Light/medium 2D games run at their full intended SWF framerate with large headroom: 2048 50 FPS (tick 0.2 ms), bubble_trouble 60 (tick 0.4), Learn to Fly 3 60 (tick 1.0), aqua-energizer 24 (its rate). Menu FPS of physics games (Bike Champ, Max Dirt Bike, stick-war) also full rate (tick 0.3-0.5; real gameplay will be heavier). **Conclusion: the emulator already runs the bulk of the library at full speed; only heavy physics (HappyWheels ~9 FPS) and software-3D (3D Taxi ~4 FPS) are slow, and those are hardware/architecture-bound.** |


Extra Comments:
Kısa cevap: "her türlü Flash oyunu yüksek FPS'te" Vita'da mümkün değil, ama "2D oyunların büyük çoğunluğu 30-60 FPS, ağır fizik ve 3D oyunlar 20-30 FPS" ulaşılabilir bir hedef. Fark donanımdan geliyor: Flash Player 2010'da 2-3 GHz masaüstü çekirdeğinde ve bir JIT ile çalışıyordu. Vita'nın 444 MHz Cortex-A9'u bunun onda biri bile değil. O dönemde masaüstünde bile zorlayan oyunlar (yazılım rasterizasyonlu 3D motorlar, binlerce cisimli fizik) burada hiçbir mimariyle 60 FPS'e çıkmaz.

Bunu bilerek, gerçekten gerekenler şunlar olurdu, önem sırasıyla:

1. **Tüm oyun kodunu derleyen bir AOT, sıcak yol AOT değil.** Bugünkü çevirmen yalnızca seçilmiş kütüphane metotlarını çeviriyor ve derlenmiş kod hâlâ yorumlayıcının veri yapılarıyla (kutulanmış `Value`, genel slot erişimi) konuşuyor. Asıl sıçrama, SWF içindeki tüm ABC'nin PC'de tip çıkarımıyla Rust'a çevrilip ARM için derlenmesinde. Bunun tarihsel kanıtı var: Adobe AIR, iOS'ta JIT yasak olduğu için ActionScript'i tam olarak böyle AOT derliyordu ve iPhone 4 sınıfı ARM'de (Vita ile benzer güç) oyunlar akıcı çalışıyordu. Mevcut çevirmen bunun tohumu.

2. **Tipli nesne yerleşimi.** Sealed AS3 sınıfları gerçek struct'lara dönüşmeli: `Number` alanı ham f64, `int` alanı i32, nesne alanı doğrudan işaretçi. Bugün bir `b2Vec2.x` okuması null kontrolü, enum eşleme, sınır kontrolü ve kilit okuması demek. Buna metot içinde ömrü biten geçici nesnelerin tahsis edilmeden skaler değişkenlere açılması (escape analysis) eklenince Box2D ve 3D motorlardaki nesne çalkantısı büyük ölçüde kaybolur. Bu, Ruffle'ın çekirdeğine derin müdahale gerektirir; Ruffle yorumlayıcı odaklı tasarlanmış.

3. **Ucuz genç nesil tahsisi ve GC.** Kısa ömürlü vektör nesneleri için bump allocator ve nesil tabanlı toplama. Bugün GC ve malloc birlikte karenin yaklaşık beşte biri.

4. **Çok çekirdek.** Vita'nın kullanılabilir 3 çekirdeği var ve Flash tek iş parçacıklı. Render komut listesi yürütme, MP3 çözme, şekil üçgenleme ve GC işaretleme başka çekirdeklere alınabilir. Bu tek başına 1.3-1.6x verir ve oyun mantığına dokunmaz.

5. **Render tarafında** vitaGL yerine doğrudan GXM, doku bütçesi ve akışı (bugün dokular hiç serbest bırakılmıyor), filtre ve blend modlarının tamamen GPU'da olması. 2D oyunların çoğunda darboğaz bu değil ama bellek çökmelerinin kaynağı bu.

6. **Oyun başına kaçış yolları.** Sabit zaman adımlı fizik için kare atlama, düşük iç çözünürlük, `StageQuality` düşürme, fizik iterasyon sayısını kısan isteğe bağlı performans modu. Mimari değil ama kullanıcıya en hızlı hissedilen kazançlar bunlar.

Pratik bir not: Vita'da homebrew JIT teknik olarak mümkün (emülatörler VM domain API'sini kullanıyor) ama ARMv7 için hazır bir kod üreteci yok; Cranelift ve dynasm 32-bit ARM'i desteklemiyor. Bu yüzden derlemeyi PC'de yapan AOT yolu doğru yol. Genelleştirmek için oyun başına derlenmiş kodu ayrı bir modül (`.suprx`) olarak yükleyebilirsiniz; böylece her şey tek ikili dosyaya sığmak zorunda kalmaz ve fat LTO bellek sınırları da ortadan kalkar.

Kaba yol haritası: PERF_PLAN'daki görevler haftalar içinde 1.5x verir. Tam ABC AOT'si artı tipli yerleşim bir-iki aylık ciddi bir iş ve 2-3x daha getirir; bu noktada 2D oyunların çoğu 30 FPS üstüne çıkar. Çok çekirdek ve GXM üçüncü aşama. Alternativa3D ve Flare3D gibi yazılım 3D motorları için tavan, bütün bunlardan sonra bile 20-30 FPS civarıdır.