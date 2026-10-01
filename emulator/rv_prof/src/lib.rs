//! RuffleVita: a sampling profiler cheap enough to leave compiled in, made
//! for finding out where the Vita's time goes.
//!
//! The main thread only *publishes* where it is: a stack of coarse zones
//! (`Zone`), the AVM1 action or AVM2 op it's executing, and the innermost
//! ActionScript function. Each is a plain store. A separate thread
//! (`start_sampler`, on another core) looks at that state about once a
//! millisecond while `set_active(true)`, and `report` turns the samples into
//! a table: time per zone (inclusive and self), per op and per function.
//!
//! Function names are only registered when profiling was enabled at startup
//! (`enable`), so normal runs pay nothing but the stores.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

/// Where the main thread is, coarsely. Zones nest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Zone {
    Other = 0,
    /// The frontend's tick (input, timers, frames).
    Tick,
    /// Input events handed to the player.
    Input,
    /// Futures (loaders, navigator) run by the frontend.
    Executor,
    /// `Player::run_frame`: one movie frame.
    RunFrame,
    Preload,
    /// AVM2 frame phases (enter frame, construct, frame scripts, exit).
    Avm2Phases,
    /// AVM1 timeline: running each clip's frame.
    Avm1Timeline,
    /// Queued actions: frame scripts, clip events, AVM2 event handlers.
    QueuedActions,
    /// Executing AVM1 bytecode.
    Avm1Code,
    /// Executing AVM2 bytecode.
    Avm2Code,
    /// A native (built-in) AVM1 function.
    Avm1Native,
    /// Hover/hit testing after updates.
    Mouse,
    Gc,
    Timers,
    Audio,
    /// Building the frame's draw commands from the display list.
    RenderList,
    /// The renderer turning commands into GL calls.
    RenderGl,
    /// The frontend's overlay and buffer swap.
    Present,
    /// Drawing a batch: buffer uploads and the draw call.
    GlFlush,
    /// A shape drawn on its own (not batched).
    GlDraw,
    // Finer zones, only in `rv_prof_ops` builds of ruffle_core:
    /// Parsing the next AVM1 action from the SWF bytes.
    Avm1Decode,
    /// Resolving a name through the AVM1 scope chain.
    Avm1Resolve,
    /// An AVM1 property map lookup.
    Avm1PropMap,
    /// Finding a child display object by instance name.
    Avm1ChildByName,
    /// Setting up and tearing down an AVM1 frame/event script run.
    Avm1ScriptSetup,
    /// Setting up and tearing down an AVM1 function call.
    Avm1CallSetup,
    /// Setting up and tearing down an AVM2 method call.
    Avm2CallSetup,
    /// A native (built-in) AVM2 method.
    Avm2Native,
    /// An AOT-compiled AVM2 method.
    Avm2Aot,
}

const ZONE_NAMES: [&str; 30] = [
    "other",
    "tick",
    "input",
    "executor",
    "run_frame",
    "preload",
    "avm2 phases",
    "avm1 timeline",
    "queued actions",
    "avm1 code",
    "avm2 code",
    "avm1 native",
    "mouse",
    "gc",
    "timers",
    "audio",
    "render list",
    "render gl",
    "present",
    "gl flush",
    "gl draw",
    "avm1 decode",
    "avm1 resolve",
    "avm1 prop map",
    "avm1 child by name",
    "avm1 script setup",
    "avm1 call setup",
    "avm2 call setup",
    "avm2 native",
    "avm2 aot",
];

const MAX_DEPTH: usize = 32;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static STACK: [AtomicU8; MAX_DEPTH] = [const { AtomicU8::new(0) }; MAX_DEPTH];
static DEPTH: AtomicUsize = AtomicUsize::new(0);
/// The AVM1 action code being executed (0 = none).
static AVM1_OP: AtomicU8 = AtomicU8::new(0);
/// The AVM2 op being executed, as its discriminant + 1 (0 = none).
static AVM2_OP: AtomicU8 = AtomicU8::new(0);
/// The innermost ActionScript function: an id from `register_function`.
static FUNCTION: AtomicU16 = AtomicU16::new(0);

static AVM2_OP_NAMES: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());
static AVM2_OP_SEEN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
static FUNCTIONS: Mutex<Option<FunctionTable>> = Mutex::new(None);
static SAMPLES: Mutex<Option<Samples>> = Mutex::new(None);

#[derive(Default)]
struct FunctionTable {
    names: Vec<String>,
    by_key: HashMap<(usize, usize), u16>,
}

#[derive(Default)]
struct Samples {
    total: u64,
    zone_self: [u64; 32],
    zone_incl: [u64; 32],
    avm1_ops: HashMap<u8, u64>,
    avm2_ops: HashMap<u8, u64>,
    functions: HashMap<u16, u64>,
    /// Self samples by (parent zone, zone), for context.
    paths: HashMap<(u8, u8), u64>,
}

/// Turns on name registration. Call before the movie loads.
pub fn enable() {
    ENABLED.store(true, Relaxed);
}

#[inline(always)]
pub fn is_enabled() -> bool {
    ENABLED.load(Relaxed)
}

/// Samples are only taken while active (the timedemo's frame range).
pub fn set_active(active: bool) {
    ACTIVE.store(active, Relaxed);
}

/// Marks the current zone until dropped.
#[must_use]
pub struct ZoneGuard(usize);

#[inline(always)]
pub fn zone(zone: Zone) -> ZoneGuard {
    let depth = DEPTH.load(Relaxed);
    if depth < MAX_DEPTH {
        STACK[depth].store(zone as u8, Relaxed);
    }
    DEPTH.store(depth + 1, Relaxed);
    ZoneGuard(depth)
}

impl Drop for ZoneGuard {
    #[inline(always)]
    fn drop(&mut self) {
        DEPTH.store(self.0, Relaxed);
    }
}

#[inline(always)]
pub fn avm1_op(code: u8) {
    AVM1_OP.store(code, Relaxed);
}

#[inline(always)]
/// `index` is the op's discriminant (`std::intrinsics::discriminant_value`).
pub fn avm2_op<T: std::fmt::Debug>(index: u8, op: &T) {
    let index = index as usize;
    AVM2_OP.store((index + 1) as u8, Relaxed);
    if !AVM2_OP_SEEN[index & 255].load(Relaxed) {
        name_avm2_op(index, op);
    }
}

#[cold]
#[inline(never)]
fn name_avm2_op<T: std::fmt::Debug>(index: usize, op: &T) {
    AVM2_OP_SEEN[index & 255].store(true, Relaxed);
    let text = format!("{op:?}");
    let name = text[..text.find([' ', '{', '(']).unwrap_or(text.len())].to_owned();
    let mut names = AVM2_OP_NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if names.len() <= index {
        names.resize(index + 1, None);
    }
    names[index] = Some(name);
}

/// The innermost function; returns the previous one, to restore on exit.
#[inline(always)]
pub fn enter_function(id: u16) -> FunctionGuard {
    FunctionGuard(FUNCTION.swap(id, Relaxed))
}

#[must_use]
pub struct FunctionGuard(u16);

impl Drop for FunctionGuard {
    #[inline(always)]
    fn drop(&mut self) {
        FUNCTION.store(self.0, Relaxed);
    }
}

/// An id for the function whose code starts at `key` (e.g. a pointer and an
/// offset), naming it the first time. Returns 0 when profiling is off.
pub fn register_function(key: (usize, usize), name: impl FnOnce() -> String) -> u16 {
    if !is_enabled() {
        return 0;
    }
    let mut table = FUNCTIONS.lock().unwrap_or_else(|e| e.into_inner());
    let table = table.get_or_insert_with(|| FunctionTable {
        names: vec!["(frame and event scripts)".into()],
        by_key: HashMap::new(),
    });
    if let Some(&id) = table.by_key.get(&key) {
        return id;
    }
    if table.names.len() >= u16::MAX as usize {
        return 0;
    }
    let id = table.names.len() as u16;
    table.names.push(name());
    table.by_key.insert(key, id);
    id
}

/// Starts the sampling thread (once). `setup` runs on it first, e.g. to
/// move it to a core other than the main thread's.
pub fn start_sampler(setup: fn()) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Relaxed) {
        return;
    }
    enable();
    let _ = std::thread::Builder::new().name("rv_prof".into()).spawn(move || {
        setup();
        sample_loop()
    });
}

fn sample_loop() {
    loop {
        std::thread::sleep(std::time::Duration::from_micros(1000));
        if !ACTIVE.load(Relaxed) {
            continue;
        }
        let depth = DEPTH.load(Relaxed).min(MAX_DEPTH);
        let mut stack = [0u8; MAX_DEPTH];
        for (i, s) in stack.iter_mut().enumerate().take(depth) {
            *s = STACK[i].load(Relaxed);
        }
        let (op1, op2, function) = (AVM1_OP.load(Relaxed), AVM2_OP.load(Relaxed), FUNCTION.load(Relaxed));
        let mut samples = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
        let s = samples.get_or_insert_with(Samples::default);
        s.total += 1;
        let top = if depth > 0 { stack[depth - 1] } else { 0 };
        let parent = if depth > 1 { stack[depth - 2] } else { 0 };
        s.zone_self[top as usize & 31] += 1;
        let mut seen = 0u32;
        for &z in &stack[..depth] {
            if seen & (1 << (z & 31)) == 0 {
                seen |= 1 << (z & 31);
                s.zone_incl[z as usize & 31] += 1;
            }
        }
        *s.paths.entry((parent, top)).or_default() += 1;
        let in_zone = |z: Zone| seen & (1 << z as u8) != 0;
        if in_zone(Zone::Avm1Code) {
            *s.avm1_ops.entry(op1).or_default() += 1;
        }
        if in_zone(Zone::Avm2Code) {
            *s.avm2_ops.entry(op2).or_default() += 1;
        }
        if in_zone(Zone::Avm1Code) || in_zone(Zone::Avm2Code) {
            *s.functions.entry(function).or_default() += 1;
        }
    }
}

/// The samples so far as a table, then starts over.
pub fn report(top: usize) -> String {
    let Some(s) = SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return "PROF: no samples".into();
    };
    let total = s.total.max(1) as f64;
    let pct = |n: u64| n as f64 * 100.0 / total;
    let mut out = format!("PROF {} samples (~1 ms each)\nPROF zone            incl%   self%\n", s.total);
    let mut zones: Vec<usize> = (0..ZONE_NAMES.len()).filter(|&z| s.zone_incl[z] > 0).collect();
    zones.sort_by_key(|&z| std::cmp::Reverse(s.zone_incl[z]));
    for z in zones {
        out += &format!("PROF {:<16} {:6.1}  {:6.1}\n", ZONE_NAMES[z], pct(s.zone_incl[z]), pct(s.zone_self[z]));
    }
    let mut paths: Vec<_> = s.paths.iter().collect();
    paths.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    out += "PROF self time by (parent > zone):\n";
    for ((p, z), n) in paths.iter().take(12) {
        let name = |z: u8| ZONE_NAMES.get(z as usize).copied().unwrap_or("?");
        out += &format!("PROF   {:5.1}%  {} > {}\n", pct(**n), name(*p), name(*z));
    }
    let mut ops: Vec<_> = s.avm1_ops.iter().collect();
    if !ops.is_empty() {
        ops.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        out += "PROF AVM1 actions (time, including what they call):\n";
        for (op, n) in ops.iter().take(top) {
            out += &format!("PROF   {:5.1}%  {}\n", pct(**n), avm1_action_name(**op));
        }
    }
    let mut ops: Vec<_> = s.avm2_ops.iter().collect();
    if !ops.is_empty() {
        ops.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let names = AVM2_OP_NAMES.lock().unwrap_or_else(|e| e.into_inner());
        out += "PROF AVM2 ops (time, including what they call):\n";
        for (op, n) in ops.iter().take(top) {
            let name = (**op as usize)
                .checked_sub(1)
                .and_then(|i| names.get(i).cloned().flatten())
                .unwrap_or_else(|| "(none)".into());
            out += &format!("PROF   {:5.1}%  {name}\n", pct(**n));
        }
    }
    let mut functions: Vec<_> = s.functions.iter().collect();
    if !functions.is_empty() {
        functions.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let table = FUNCTIONS.lock().unwrap_or_else(|e| e.into_inner());
        out += "PROF ActionScript functions (self, innermost function):\n";
        for (id, n) in functions.iter().take(top) {
            let name = table
                .as_ref()
                .and_then(|t| t.names.get(**id as usize).cloned())
                .unwrap_or_else(|| "(frame and event scripts)".into());
            out += &format!("PROF   {:5.1}%  {name}\n", pct(**n));
        }
    }
    out
}

fn avm1_action_name(code: u8) -> String {
    let name = match code {
        0x00 => "End",
        0x04 => "NextFrame",
        0x05 => "PreviousFrame",
        0x06 => "Play",
        0x07 => "Stop",
        0x08 => "ToggleQuality",
        0x09 => "StopSounds",
        0x0A => "Add",
        0x0B => "Subtract",
        0x0C => "Multiply",
        0x0D => "Divide",
        0x0E => "Equals",
        0x0F => "Less",
        0x10 => "And",
        0x11 => "Or",
        0x12 => "Not",
        0x13 => "StringEquals",
        0x14 => "StringLength",
        0x15 => "StringExtract",
        0x17 => "Pop",
        0x18 => "ToInteger",
        0x1C => "GetVariable",
        0x1D => "SetVariable",
        0x20 => "SetTarget2",
        0x21 => "StringAdd",
        0x22 => "GetProperty",
        0x23 => "SetProperty",
        0x24 => "CloneSprite",
        0x25 => "RemoveSprite",
        0x26 => "Trace",
        0x27 => "StartDrag",
        0x28 => "EndDrag",
        0x29 => "StringLess",
        0x2A => "Throw",
        0x2B => "CastOp",
        0x2C => "ImplementsOp",
        0x30 => "RandomNumber",
        0x31 => "MBStringLength",
        0x32 => "CharToAscii",
        0x33 => "AsciiToChar",
        0x34 => "GetTime",
        0x35 => "MBStringExtract",
        0x36 => "MBCharToAscii",
        0x37 => "MBAsciiToChar",
        0x3A => "Delete",
        0x3B => "Delete2",
        0x3C => "DefineLocal",
        0x3D => "CallFunction",
        0x3E => "Return",
        0x3F => "Modulo",
        0x40 => "NewObject",
        0x41 => "DefineLocal2",
        0x42 => "InitArray",
        0x43 => "InitObject",
        0x44 => "TypeOf",
        0x45 => "TargetPath",
        0x46 => "Enumerate",
        0x47 => "Add2",
        0x48 => "Less2",
        0x49 => "Equals2",
        0x4A => "ToNumber",
        0x4B => "ToString",
        0x4C => "PushDuplicate",
        0x4D => "StackSwap",
        0x4E => "GetMember",
        0x4F => "SetMember",
        0x50 => "Increment",
        0x51 => "Decrement",
        0x52 => "CallMethod",
        0x53 => "NewMethod",
        0x54 => "InstanceOf",
        0x55 => "Enumerate2",
        0x60 => "BitAnd",
        0x61 => "BitOr",
        0x62 => "BitXor",
        0x63 => "BitLShift",
        0x64 => "BitRShift",
        0x65 => "BitURShift",
        0x66 => "StrictEquals",
        0x67 => "Greater",
        0x68 => "StringGreater",
        0x69 => "Extends",
        0x81 => "GotoFrame",
        0x83 => "GetUrl",
        0x87 => "StoreRegister",
        0x88 => "ConstantPool",
        0x8A => "WaitForFrame",
        0x8B => "SetTarget",
        0x8C => "GotoLabel",
        0x8D => "WaitForFrame2",
        0x8E => "DefineFunction2",
        0x8F => "Try",
        0x94 => "With",
        0x96 => "Push",
        0x99 => "Jump",
        0x9A => "GetUrl2",
        0x9B => "DefineFunction",
        0x9D => "If",
        0x9E => "Call",
        0x9F => "GotoFrame2",
        _ => return format!("0x{code:02X}"),
    };
    name.to_owned()
}
