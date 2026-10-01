//! RuffleVita: hot-path ahead-of-time compilation for AVM2.
//!
//! The translator (`translate`) turns chosen methods into Rust functions
//! (`generated.rs`), working from the op stream Ruffle's verifier and
//! optimizer produce, so slot numbers, dispatch ids and fused ops are
//! already resolved. When such a method is called, `exec` runs the compiled
//! function instead of the interpreter, but only if the method's op stream
//! at runtime is exactly the one that was translated (see `canonical`):
//! anything else, and every method that wasn't translated, is interpreted
//! as before.
//!
//! Compiled code reads the method's runtime handles (scripts, multinames,
//! classes) from its own op stream (`ops[i]`), so it embeds no pointers, and
//! hands the ops it doesn't compile to the interpreter's own handlers
//! (`Activation::aot_execute`), which keeps their behaviour identical.
//! See docs/AOT.md.

mod generated;
#[allow(dead_code)] // Only what the generated code uses is used.
pub mod rt;
pub mod translate;

use crate::avm2::activation::Activation;
use crate::avm2::method::{Method, NativeMethodImpl};
use crate::avm2::op::Op;
use crate::avm2::value::Value;
use crate::avm2::Error;
use std::sync::OnceLock;

/// A compiled method: runs with the activation `exec` set up (locals hold
/// `this` and the coerced arguments) and the method's op stream.
pub type AotFn =
    for<'a, 'gc> fn(&mut Activation<'a, 'gc>, &[Op<'gc>]) -> Result<Value<'gc>, Error<'gc>>;

/// A compiled leaf method: one that runs no interpreter handlers, so all it
/// needs of its own activation is its outer scope and caller domain/movie,
/// and a compiled caller can run it on its own activation instead (see
/// `Activation::aot_run_leaf`). Takes `this` and the coerced arguments.
pub type LeafFn = for<'a, 'gc> fn(
    &mut Activation<'a, 'gc>,
    &[Op<'gc>],
    &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>>;

#[allow(dead_code)]
pub struct Entry {
    /// Index of the method in its ABC file, and its number of ops: a cheap
    /// first check before `fingerprint`.
    pub abc_method: u32,
    pub num_ops: u32,
    /// `fingerprint(canonical(...))` of the op stream it was compiled from.
    pub fingerprint: u64,
    pub name: &'static str,
    pub run: AotFn,
    pub leaf: Option<LeafFn>,
}

/// Builtins the translator inlines: their `NativeMethodImpl` and the name
/// the canonical form uses.
pub fn known_native(method: NativeMethodImpl) -> Option<&'static str> {
    use crate::avm2::globals::math;
    let table: [(NativeMethodImpl, &'static str); 13] = [
        (math::sin, "Math.sin"),
        (math::cos, "Math.cos"),
        (math::tan, "Math.tan"),
        (math::sqrt, "Math.sqrt"),
        (math::abs, "Math.abs"),
        (math::atan, "Math.atan"),
        (math::asin, "Math.asin"),
        (math::acos, "Math.acos"),
        (math::exp, "Math.exp"),
        (math::log, "Math.log"),
        (math::floor, "Math.floor"),
        (math::ceil, "Math.ceil"),
        (math::atan2, "Math.atan2"),
    ];
    table.iter().find(|(f, _)| std::ptr::fn_addr_eq(*f, method)).map(|(_, n)| *n)
}

/// The op stream as text, without anything that differs between runs
/// (pointers), plus what the generated code depends on.
pub fn canonical(name: &str, num_locals: u32, params: &[ParamKind], ops: &[Op<'_>]) -> String {
    let mut out = format!("{name}\nlocals {num_locals}\nparams {params:?}\n");
    for op in ops {
        let text = format!("{op:?}");
        // Drop "0x…" addresses (Script pointers, native function pointers).
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '0' && chars.peek() == Some(&'x') {
                chars.next();
                while chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                    chars.next();
                }
                out.push('@');
            } else {
                out.push(c);
            }
        }
        if let Op::CallNative { method, .. } = op {
            out.push_str(" = ");
            out.push_str(known_native(*method).unwrap_or("?"));
        }
        out.push('\n');
    }
    out
}

/// 64-bit FNV-1a.
pub fn fingerprint(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let on = std::env::var_os("RUFFLEVITA_NO_AOT").is_none() && !generated::TABLE.is_empty();
        if on {
            tracing::info!("AOT: {} compiled methods available", generated::TABLE.len());
        }
        on
    })
}

/// The compiled version of `method`, if there is one for exactly this op
/// stream. Decided on the first call and remembered on the method.
#[cfg_attr(feature = "rv_outline", inline(never))]
#[cfg_attr(not(feature = "rv_outline"), inline)]
pub fn lookup(method: Method<'_>) -> Option<AotFn> {
    match method.aot_slot() {
        0 => resolve(method),
        1 => None,
        slot => Some(generated::TABLE[slot as usize - 2].run),
    }
}

/// The leaf version of `method`, once `lookup` has resolved it.
#[inline(always)]
pub fn leaf(method: Method<'_>) -> Option<LeafFn> {
    match method.aot_slot() {
        0 | 1 => None,
        slot => generated::TABLE[slot as usize - 2].leaf,
    }
}

#[cold]
#[inline(never)]
fn resolve(method: Method<'_>) -> Option<AotFn> {
    let found = enabled().then(|| find(method)).flatten();
    method.set_aot_slot(found.map_or(1, |i| i as u16 + 2));
    found.map(|i| generated::TABLE[i].run)
}

fn find(method: Method<'_>) -> Option<usize> {
    let ops = &method.get_verified_info().parsed_code;
    let abc_method = method.abc_method_index();
    let candidates: Vec<usize> = (0..generated::TABLE.len())
        .filter(|&i| {
            let e = &generated::TABLE[i];
            e.abc_method == abc_method && e.num_ops as usize == ops.len()
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let name = method_name(method);
    let num_locals = method.body().map_or(0, |b| b.num_locals);
    let fp = fingerprint(&canonical(&name, num_locals, &param_kinds(method), ops));
    let found = candidates.into_iter().find(|&i| generated::TABLE[i].fingerprint == fp);
    match found {
        Some(_) => tracing::info!("AOT: using compiled {name}"),
        None => tracing::warn!("AOT: {name} changed since it was compiled; interpreting it"),
    }
    found
}

pub fn method_name(method: Method<'_>) -> String {
    let mut name = crate::string::WString::new();
    crate::avm2::function::display_function(&mut name, method);
    name.to_utf8_lossy().into_owned()
}

/// What the generated code may assume about a parameter at entry: the
/// interpreter has already coerced it to its declared type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// `Number`, `int` or `uint`: always a numeric `Value`.
    Numeric,
    Other,
    /// After the declared parameters: the method takes `...rest` or
    /// `arguments` (the next local).
    Rest,
}

/// One entry per declared parameter (locals 1..=n), then `Rest` if the
/// method is variadic.
pub fn param_kinds(method: Method<'_>) -> Vec<ParamKind> {
    let mut kinds: Vec<ParamKind> = method
        .resolved_param_config()
        .iter()
        .map(|p| match p.param_type {
            Some(c) if c.is_builtin_number() || c.is_builtin_int() || c.is_builtin_uint() => ParamKind::Numeric,
            _ => ParamKind::Other,
        })
        .collect();
    if method.is_variadic() {
        kinds.push(ParamKind::Rest);
    }
    kinds
}
