//! RuffleVita: what AOT-compiled methods call. Every function does exactly
//! what the interpreter does for the same op (`Activation::op_*`): numeric
//! cases inline, everything else through the interpreter's own handler
//! (`delegate`), with the operands on the activation's operand stack.

use crate::avm2::activation::Activation;
use crate::avm2::function::FunctionArgs;
use crate::avm2::object::TObject;
use crate::avm2::op::Op;
use crate::avm2::value::Value;
use crate::avm2::Error;

/// Runs `op` in the interpreter with `operands` on the operand stack, and
/// returns what it left there (if `pushes`).
#[inline(never)]
pub fn delegate<'gc>(
    act: &mut Activation<'_, 'gc>,
    op: &Op<'gc>,
    operands: &[Value<'gc>],
    pushes: bool,
) -> Result<Value<'gc>, Error<'gc>> {
    for v in operands {
        act.push_stack(*v);
    }
    let handled = act.aot_execute(op)?;
    debug_assert!(handled, "AOT delegated an op the interpreter bridge doesn't run: {op:?}");
    Ok(if pushes { act.pop_stack() } else { Value::Undefined })
}

#[inline(always)]
fn delegate2<'gc>(
    act: &mut Activation<'_, 'gc>,
    op: &Op<'gc>,
    a: Value<'gc>,
    b: Value<'gc>,
) -> Result<Value<'gc>, Error<'gc>> {
    delegate(act, op, &[a, b], true)
}

// --- Arithmetic (same fast paths as op_add, op_subtract, op_multiply) ---

#[inline(always)]
pub fn add<'gc>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok((x + y).into()),
        (Value::Number(x), Value::Number(y)) => Ok((x + y).into()),
        _ => delegate2(act, op, a, b),
    }
}

#[inline(always)]
pub fn subtract<'gc>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok((x - y).into()),
        (Value::Number(x), Value::Number(y)) => Ok((x - y).into()),
        _ => delegate2(act, op, a, b),
    }
}

#[inline(always)]
pub fn multiply<'gc>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) if x.checked_mul(y).is_some() => Ok((x * y).into()),
        (Value::Number(x), Value::Number(y)) => Ok((x * y).into()),
        _ => delegate2(act, op, a, b),
    }
}

/// `op_divide`: both operands to Number, the right one first.
#[inline(always)]
pub fn divide<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    let b = b.coerce_to_number(act)?;
    let a = a.coerce_to_number(act)?;
    Ok((a / b).into())
}

#[inline(always)]
pub fn modulo<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    let b = b.coerce_to_number(act)?;
    let a = a.coerce_to_number(act)?;
    Ok((a % b).into())
}

#[inline(always)]
pub fn negate<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok((-a.coerce_to_number(act)?).into())
}

#[inline(always)]
pub fn increment<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok((a.coerce_to_number(act)? + 1.0).into())
}

#[inline(always)]
pub fn decrement<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok((a.coerce_to_number(act)? - 1.0).into())
}

#[inline(always)]
pub fn increment_i<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok(a.coerce_to_i32(act)?.wrapping_add(1).into())
}

#[inline(always)]
pub fn decrement_i<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok(a.coerce_to_i32(act)?.wrapping_sub(1).into())
}

#[inline(always)]
pub fn negate_i<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok(a.coerce_to_i32(act)?.wrapping_neg().into())
}

/// Integer ops take the right operand first, like the interpreter.
macro_rules! int_binop {
    ($name:ident, $left:ident, $right:ident, |$x:ident, $y:ident| $e:expr) => {
        #[inline(always)]
        pub fn $name<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
            let $y = b.$right(act)?;
            let $x = a.$left(act)?;
            Ok(Value::from($e))
        }
    };
}

int_binop!(bit_and, coerce_to_i32, coerce_to_i32, |x, y| x & y);
int_binop!(bit_or, coerce_to_i32, coerce_to_i32, |x, y| x | y);
int_binop!(bit_xor, coerce_to_i32, coerce_to_i32, |x, y| x ^ y);
int_binop!(lshift, coerce_to_i32, coerce_to_u32, |x, y| x << (y & 0x1F));
int_binop!(rshift, coerce_to_i32, coerce_to_u32, |x, y| x >> (y & 0x1F));
int_binop!(urshift, coerce_to_u32, coerce_to_u32, |x, y| x >> (y & 0x1F));
int_binop!(add_i, coerce_to_i32, coerce_to_i32, |x, y| x.wrapping_add(y));
int_binop!(subtract_i, coerce_to_i32, coerce_to_i32, |x, y| x.wrapping_sub(y));
int_binop!(multiply_i, coerce_to_i32, coerce_to_i32, |x, y| x.wrapping_mul(y));

#[inline(always)]
pub fn bit_not<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    Ok(Value::from(!a.coerce_to_i32(act)?))
}

// --- Comparisons ---

/// `a < b` (abstract relational comparison): `None` when a NaN is involved.
#[inline(always)]
pub fn lt<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<Option<bool>, Error<'gc>> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(Some(x < y)),
        (Value::Number(x), Value::Number(y)) => Ok(if x.is_nan() || y.is_nan() { None } else { Some(x < y) }),
        _ => a.abstract_lt(&b, act),
    }
}

#[inline(always)]
pub fn eq<'gc>(act: &mut Activation<'_, 'gc>, a: Value<'gc>, b: Value<'gc>) -> Result<bool, Error<'gc>> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x == y),
        (Value::Number(x), Value::Number(y)) => Ok(x == y),
        _ => a.abstract_eq(&b, act),
    }
}

// --- Calls and properties ---

/// op_call_method, without going through the operand stack. A compiled
/// leaf callee runs right here, on this activation (see `aot::LeafFn`).
#[inline(always)]
pub fn call_method<'gc>(
    act: &mut Activation<'_, 'gc>,
    op: &Op<'gc>,
    receiver: Value<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let Op::CallMethod { index, .. } = op else { unreachable!("not CallMethod") };
    let receiver = receiver.null_check(act, None)?;
    if let Value::Object(object) = receiver {
        if let Some(result) = call_leaf(act, object, *index, receiver, args) {
            return result;
        }
    }
    receiver.call_method_with_args(*index, FunctionArgs::from_slice(args), act)
}

/// What `call_method_with_args` and `exec` do for a method compiled as a
/// leaf, minus the activation: `None` if the method isn't one, or the call
/// needs anything else (a bound method, default or rest arguments).
#[inline(always)]
fn call_leaf<'gc>(
    act: &mut Activation<'_, 'gc>,
    object: crate::avm2::object::Object<'gc>,
    index: u32,
    receiver: Value<'gc>,
    args: &[Value<'gc>],
) -> Option<Result<Value<'gc>, Error<'gc>>> {
    const MAX_ARGS: usize = 7;
    if args.len() > MAX_ARGS || crate::rv_stack::exhausted() || object.get_bound_method(index).is_some() {
        return None;
    }
    let full_method = object.vtable().get_full_method(index)?;
    let method = full_method.method;
    let leaf = super::leaf(method)?;
    let signature = method.resolved_param_config();
    if signature.len() != args.len() {
        return None;
    }
    // Like init_from_method: `this`, then each argument coerced to its type.
    let mut locals = [Value::Undefined; MAX_ARGS + 1];
    locals[0] = receiver;
    for (k, (arg, param)) in args.iter().zip(signature).enumerate() {
        locals[k + 1] = match param.param_type {
            Some(class) => match arg.coerce_to_type(act, class) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            },
            None => *arg,
        };
    }
    let ops = &method.get_verified_info().parsed_code;
    let mc = act.gc();
    act.context.avm2.push_call(mc, method);
    let result = act.aot_run_leaf(method, full_method.scope(), |act| leaf(act, ops, &locals[..=args.len()]));
    act.context.avm2.pop_call(mc);
    Some(result)
}

/// op_coerce.
#[inline(always)]
pub fn coerce<'gc>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>, value: Value<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    let Op::Coerce { class } = op else { unreachable!("not Coerce") };
    value.coerce_to_type(act, *class)
}

/// op_construct_slot, without going through the operand stack.
#[inline(always)]
pub fn construct_slot<'gc>(
    act: &mut Activation<'_, 'gc>,
    op: &Op<'gc>,
    source: Value<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let Op::ConstructSlot { index, .. } = op else { unreachable!("not ConstructSlot") };
    let source = source
        .null_check(act, None)?
        .as_object()
        .expect("Cannot get_slot on primitive");
    let ctor = source.get_slot(*index);
    ctor.construct(act, FunctionArgs::from_slice(args))
}

/// op_get_property_fast: array-like objects by index inline, the rest in
/// the interpreter.
#[inline(always)]
pub fn get_property_fast<'gc>(
    act: &mut Activation<'_, 'gc>,
    op: &Op<'gc>,
    object: Value<'gc>,
    name: Value<'gc>,
) -> Result<Value<'gc>, Error<'gc>> {
    use crate::avm2::object::TObject;
    use crate::avm2::Object;
    if let (Value::Object(o), Value::Integer(_) | Value::Number(_)) = (object, name) {
        if let Some(index) = name.try_as_index() {
            // The common types call their own method: no dispatch on `Object`.
            let found = match o {
                Object::ArrayObject(a) => a.get_index_property(index),
                Object::VectorObject(v) => v.get_index_property(index),
                o => o.get_index_property(index),
            };
            if let Some(value) = found {
                return Ok(value);
            }
        }
    }
    delegate(act, op, &[object, name], true)
}

/// op_set_property_fast.
#[inline(always)]
pub fn set_property_fast<'gc>(
    act: &mut Activation<'_, 'gc>,
    op: &Op<'gc>,
    object: Value<'gc>,
    name: Value<'gc>,
    value: Value<'gc>,
) -> Result<(), Error<'gc>> {
    if let (Value::Object(o), Value::Integer(_) | Value::Number(_)) = (object, name) {
        if let Some(index) = name.try_as_index() {
            if let Some(result) = o.set_index_property(act, index, value) {
                return result;
            }
        }
    }
    delegate(act, op, &[object, name, value], false).map(|_| ())
}

// --- Slots and scopes ---

#[inline(always)]
pub fn get_slot<'gc>(act: &mut Activation<'_, 'gc>, object: Value<'gc>, slot: u32) -> Result<Value<'gc>, Error<'gc>> {
    // Plain class instances: straight to the slots, no dispatch on `Object`.
    if let Value::Object(crate::avm2::Object::ScriptObject(so)) = object {
        return Ok(crate::avm2::object::ScriptObjectWrapper(so.0).get_slot(slot));
    }
    let object = object
        .null_check(act, None)?
        .as_object()
        .expect("Cannot get_slot on primitive");
    Ok(object.get_slot(slot))
}

#[inline(always)]
pub fn set_slot<'gc>(act: &mut Activation<'_, 'gc>, object: Value<'gc>, value: Value<'gc>, slot: u32) -> Result<(), Error<'gc>> {
    let object = object
        .null_check(act, None)?
        .as_object()
        .expect("Cannot set_slot on primitive");
    object.set_slot(slot, value, act)
}

#[inline(always)]
pub fn set_slot_no_coerce<'gc>(act: &mut Activation<'_, 'gc>, object: Value<'gc>, value: Value<'gc>, slot: u32) -> Result<(), Error<'gc>> {
    if let Value::Object(crate::avm2::Object::ScriptObject(so)) = object {
        crate::avm2::object::ScriptObjectWrapper(so.0).set_slot(slot, value, act.gc());
        return Ok(());
    }
    let object = object
        .null_check(act, None)?
        .as_object()
        .expect("Cannot set_slot on primitive");
    object.set_slot_no_coerce(slot, value, act.gc());
    Ok(())
}

#[inline(always)]
pub fn script_globals<'gc>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>) -> Result<Value<'gc>, Error<'gc>> {
    let Op::GetScriptGlobals { script } = op else { unreachable!("not GetScriptGlobals") };
    Ok(script.globals(act.context)?.into())
}

#[inline(always)]
pub fn outer_scope<'gc>(act: &mut Activation<'_, 'gc>, index: usize) -> Value<'gc> {
    act.outer().get_unchecked(index).values().into()
}

// --- Unboxed numbers -------------------------------------------------------
//
// A numeric `Value` without the box: the number, and whether the interpreter
// would hold it as `Value::Integer` (which some ops treat differently, e.g.
// int * int stays an int and can't produce -0). The flag is usually a
// compile-time constant in generated code, so its checks fold away.

pub type Num = (f64, bool);

const INT_RANGE: std::ops::Range<i32> = -(1 << 28)..(1 << 28);

/// `Value::from(i32)`: an int inside the 29-bit range, else a Number.
#[inline(always)]
pub fn from_i32(r: i32) -> Num {
    (f64::from(r), INT_RANGE.contains(&r))
}

/// `Value::from(u32)`.
#[inline(always)]
pub fn from_u32(r: u32) -> Num {
    (f64::from(r), r < (1 << 28))
}

#[inline(always)]
pub fn boxn<'gc>((x, int): Num) -> Value<'gc> {
    if int {
        Value::Integer(x as i32)
    } else {
        Value::Number(x)
    }
}

/// A value an op guarantees to be numeric (or a parameter of a numeric type).
#[inline(always)]
pub fn unbox(value: Value<'_>) -> Num {
    match value {
        Value::Number(n) => (n, false),
        Value::Integer(i) => (f64::from(i), true),
        other => unreachable!("AOT: expected a number, got {other:?}"),
    }
}

/// An operand of a typed op: an unboxed number, or a `Value` of any type.
pub trait Operand<'gc>: Copy {
    fn num(self) -> Option<Num>;
    fn value(self) -> Value<'gc>;
}

impl<'gc> Operand<'gc> for Num {
    #[inline(always)]
    fn num(self) -> Option<Num> {
        Some(self)
    }
    #[inline(always)]
    fn value(self) -> Value<'gc> {
        boxn(self)
    }
}

impl<'gc> Operand<'gc> for Value<'gc> {
    #[inline(always)]
    fn num(self) -> Option<Num> {
        match self {
            Value::Number(n) => Some((n, false)),
            Value::Integer(i) => Some((f64::from(i), true)),
            _ => None,
        }
    }
    #[inline(always)]
    fn value(self) -> Value<'gc> {
        self
    }
}

/// `coerce_to_number`.
#[inline(always)]
pub fn to_f64<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<f64, Error<'gc>> {
    match a.num() {
        Some((x, _)) => Ok(x),
        None => a.value().coerce_to_number(act),
    }
}

/// `coerce_to_i32` (for an int, its value; for a Number, ToInt32).
#[inline(always)]
pub fn to_i32<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<i32, Error<'gc>> {
    match a.num() {
        Some((x, _)) => Ok(crate::ecma_conversions::f64_to_wrapping_i32(x)),
        None => a.value().coerce_to_i32(act),
    }
}

#[inline(always)]
pub fn to_u32<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<u32, Error<'gc>> {
    match a.num() {
        Some((x, _)) => Ok(crate::ecma_conversions::f64_to_wrapping_u32(x)),
        None => a.value().coerce_to_u32(act),
    }
}

/// `+` on two numbers (op_add's int and Number fast paths, and its
/// mixed int/Number case, which goes through ToNumber).
#[inline(always)]
pub fn t_add(a: Num, b: Num) -> Num {
    if a.1 && b.1 {
        from_i32(a.0 as i32 + b.0 as i32)
    } else {
        (a.0 + b.0, false)
    }
}

/// op_subtract.
#[inline(always)]
pub fn t_sub<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>, a: A, b: B) -> Result<Num, Error<'gc>> {
    match (a.num(), b.num()) {
        (Some(x), Some(y)) if x.1 && y.1 => Ok(from_i32(x.0 as i32 - y.0 as i32)),
        (Some(x), Some(y)) => Ok((x.0 - y.0, false)),
        _ => Ok(unbox(subtract(act, op, a.value(), b.value())?)),
    }
}

/// op_multiply.
#[inline(always)]
pub fn t_mul<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, op: &Op<'gc>, a: A, b: B) -> Result<Num, Error<'gc>> {
    match (a.num(), b.num()) {
        (Some(x), Some(y)) if x.1 && y.1 => Ok(match (x.0 as i32).checked_mul(y.0 as i32) {
            Some(r) => from_i32(r),
            None => (x.0 * y.0, false),
        }),
        (Some(x), Some(y)) => Ok((x.0 * y.0, false)),
        _ => Ok(unbox(multiply(act, op, a.value(), b.value())?)),
    }
}

/// op_divide / op_modulo: ToNumber of the right operand, then the left.
#[inline(always)]
pub fn t_div<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A, b: B) -> Result<Num, Error<'gc>> {
    let y = to_f64(act, b)?;
    let x = to_f64(act, a)?;
    Ok((x / y, false))
}

#[inline(always)]
pub fn t_mod<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A, b: B) -> Result<Num, Error<'gc>> {
    let y = to_f64(act, b)?;
    let x = to_f64(act, a)?;
    Ok((x % y, false))
}

#[inline(always)]
pub fn t_neg<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok((-to_f64(act, a)?, false))
}

#[inline(always)]
pub fn t_inc<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok((to_f64(act, a)? + 1.0, false))
}

#[inline(always)]
pub fn t_dec<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok((to_f64(act, a)? - 1.0, false))
}

#[inline(always)]
pub fn t_inc_i<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok(from_i32(to_i32(act, a)?.wrapping_add(1)))
}

#[inline(always)]
pub fn t_dec_i<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok(from_i32(to_i32(act, a)?.wrapping_sub(1)))
}

#[inline(always)]
pub fn t_neg_i<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok(from_i32(to_i32(act, a)?.wrapping_neg()))
}

#[inline(always)]
pub fn t_bit_not<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok(from_i32(!to_i32(act, a)?))
}

#[inline(always)]
pub fn t_coerce_d<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok((to_f64(act, a)?, false))
}

#[inline(always)]
pub fn t_coerce_i<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok(from_i32(to_i32(act, a)?))
}

#[inline(always)]
pub fn t_coerce_u<'gc, A: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A) -> Result<Num, Error<'gc>> {
    Ok(from_u32(to_u32(act, a)?))
}

/// Integer binary ops: the right operand is converted first.
macro_rules! t_int_binop {
    ($name:ident, $left:ident, $right:ident, $wrap:ident, |$x:ident, $y:ident| $e:expr) => {
        #[inline(always)]
        pub fn $name<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A, b: B) -> Result<Num, Error<'gc>> {
            let $y = $right(act, b)?;
            let $x = $left(act, a)?;
            Ok($wrap($e))
        }
    };
}

t_int_binop!(t_bit_and, to_i32, to_i32, from_i32, |x, y| x & y);
t_int_binop!(t_bit_or, to_i32, to_i32, from_i32, |x, y| x | y);
t_int_binop!(t_bit_xor, to_i32, to_i32, from_i32, |x, y| x ^ y);
t_int_binop!(t_lshift, to_i32, to_u32, from_i32, |x, y| x << (y & 0x1F));
t_int_binop!(t_rshift, to_i32, to_u32, from_i32, |x, y| x >> (y & 0x1F));
t_int_binop!(t_urshift, to_u32, to_u32, from_u32, |x, y| x >> (y & 0x1F));
t_int_binop!(t_add_i, to_i32, to_i32, from_i32, |x, y| x.wrapping_add(y));
t_int_binop!(t_sub_i, to_i32, to_i32, from_i32, |x, y| x.wrapping_sub(y));
t_int_binop!(t_mul_i, to_i32, to_i32, from_i32, |x, y| x.wrapping_mul(y));

/// `a < b`: `None` when a NaN is involved.
#[inline(always)]
pub fn t_lt<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A, b: B) -> Result<Option<bool>, Error<'gc>> {
    match (a.num(), b.num()) {
        (Some((x, _)), Some((y, _))) => Ok(if x.is_nan() || y.is_nan() { None } else { Some(x < y) }),
        _ => lt(act, a.value(), b.value()),
    }
}

/// `a == b`.
#[inline(always)]
pub fn t_eq<'gc, A: Operand<'gc>, B: Operand<'gc>>(act: &mut Activation<'_, 'gc>, a: A, b: B) -> Result<bool, Error<'gc>> {
    match (a.num(), b.num()) {
        (Some((x, _)), Some((y, _))) => Ok(x == y),
        _ => eq(act, a.value(), b.value()),
    }
}

/// `a === b`.
#[inline(always)]
pub fn t_strict_eq<'gc, A: Operand<'gc>, B: Operand<'gc>>(a: A, b: B) -> bool {
    match (a.num(), b.num()) {
        (Some((x, _)), Some((y, _))) => x == y,
        _ => a.value().strict_eq(&b.value()),
    }
}

/// `coerce_to_boolean`.
#[inline(always)]
pub fn t_truthy<'gc, A: Operand<'gc>>(a: A) -> bool {
    match a.num() {
        Some((x, _)) => !x.is_nan() && x != 0.0,
        None => a.value().coerce_to_boolean(),
    }
}

/// `as_f64`, what the Math builtins read their arguments with.
#[inline(always)]
pub fn t_as_f64<'gc, A: Operand<'gc>>(a: A) -> f64 {
    match a.num() {
        Some((x, _)) => x,
        None => a.value().as_f64(),
    }
}
