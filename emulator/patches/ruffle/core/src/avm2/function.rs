use crate::avm2::activation::Activation;
use crate::avm2::error::{make_mismatch_error, Error};
use crate::avm2::method::{Method, MethodKind, ParamConfig};
use crate::avm2::object::{ClassObject, FunctionObject};
use crate::avm2::scope::ScopeChain;
use crate::avm2::traits::TraitKind;
use crate::avm2::value::Value;
use crate::avm2::Multiname;
use crate::string::WString;
use gc_arena::{Collect, Gc};
use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;

/// Represents a bound method.
#[derive(Clone, Collect)]
#[collect(no_drop)]
pub struct BoundMethod<'gc> {
    /// The method code to execute from a given ABC file.
    method: Method<'gc>,

    /// The scope this method was defined in.
    scope: ScopeChain<'gc>,

    /// The receiver that this function is always called with.
    ///
    /// If `None`, then the receiver provided by the caller is used. A
    /// `Some` value indicates a bound executable.
    ///
    /// This should never be `Value::Null` or `Value::Undefined`.
    bound_receiver: Option<Value<'gc>>,

    /// The superclass of the bound class for this method.
    ///
    /// The `bound_superclass` is the superclass of the class that defined
    /// this method. If `None`, then there is no defining class and `super`
    /// operations should be invalid.
    bound_superclass: Option<ClassObject<'gc>>,
}

impl<'gc> BoundMethod<'gc> {
    pub fn from_method(
        method: Method<'gc>,
        scope: ScopeChain<'gc>,
        receiver: Option<Value<'gc>>,
        superclass: Option<ClassObject<'gc>>,
    ) -> Self {
        Self {
            method,
            scope,
            bound_receiver: receiver,
            bound_superclass: superclass,
        }
    }

    pub fn exec(
        &self,
        unbound_receiver: Value<'gc>,
        arguments: FunctionArgs<'_, 'gc>,
        activation: &mut Activation<'_, 'gc>,
        callee: Option<FunctionObject<'gc>>,
    ) -> Result<Value<'gc>, Error<'gc>> {
        let receiver = if let Some(receiver) = self.bound_receiver {
            receiver
        } else if matches!(unbound_receiver, Value::Null | Value::Undefined) {
            self.scope
                .get(0)
                .expect("No global scope for function call")
                .values()
        } else {
            unbound_receiver
        };

        exec(
            self.method,
            self.scope,
            receiver,
            self.bound_superclass,
            arguments,
            activation,
            callee,
        )
    }

    pub fn as_method(&self) -> Method<'gc> {
        self.method
    }

    pub fn debug_full_name(&self) -> WString {
        let mut output = WString::new();
        display_function(&mut output, self.as_method());
        output
    }

    pub fn signature(&self) -> &[ParamConfig<'gc>] {
        self.method.signature()
    }

    pub fn is_variadic(&self) -> bool {
        self.method.is_variadic()
    }

    pub fn return_type(&self) -> Option<Gc<'gc, Multiname<'gc>>> {
        self.method.return_type()
    }
}

#[derive(Clone, Copy)]
pub enum FunctionArgs<'a, 'gc> {
    AsCellArgs(&'a [Cell<Value<'gc>>]),
    AsArgs(&'a [Value<'gc>]),
}

impl<'a, 'gc> FunctionArgs<'a, 'gc> {
    pub fn empty() -> Self {
        FunctionArgs::AsArgs(&[])
    }

    pub fn from_slice(args: &'a [Value<'gc>]) -> Self {
        FunctionArgs::AsArgs(args)
    }

    pub fn from_cell_slice(args: &'a [Cell<Value<'gc>>]) -> Self {
        FunctionArgs::AsCellArgs(args)
    }

    pub fn to_slice(self) -> Cow<'a, [Value<'gc>]> {
        match self {
            FunctionArgs::AsCellArgs(arguments) => {
                Cow::Owned(arguments.iter().map(|o| o.get()).collect::<Vec<_>>())
            }
            FunctionArgs::AsArgs(arguments) => Cow::Borrowed(arguments),
        }
    }

    pub fn get_at(&self, index: usize) -> Value<'gc> {
        match self {
            FunctionArgs::AsCellArgs(arguments) => arguments[index].get(),
            FunctionArgs::AsArgs(arguments) => arguments[index],
        }
    }

    pub fn iter(&'a self) -> FunctionArgsIter<'a, 'gc> {
        FunctionArgsIter {
            args: self,
            next: 0,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            FunctionArgs::AsCellArgs(arguments) => arguments.len(),
            FunctionArgs::AsArgs(arguments) => arguments.len(),
        }
    }
}

pub struct FunctionArgsIter<'a, 'gc> {
    args: &'a FunctionArgs<'a, 'gc>,
    next: usize,
}

impl<'a, 'gc> Iterator for FunctionArgsIter<'a, 'gc> {
    type Item = Value<'gc>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.args.len() {
            None
        } else {
            self.next += 1;
            Some(self.args.get_at(self.next - 1))
        }
    }
}

/// Execute a method.
///
/// The function will either be called directly if it is a Rust builtin, or
/// executed on the same AVM2 instance as the activation passed in here.
/// The value returned in either case will be provided here.
///
/// It is a panicking logic error to attempt to execute user code while any
/// reachable object is currently under a write lock.
///
/// Passed-in arguments will be conformed to the set of method parameters
/// declared on the function.
///
/// It is the caller's responsibility to ensure that the `receiver` passed
/// to this method is not Value::Null or Value::Undefined.
/// RuffleVita: the optional cap on Box2D's velocity/position iteration counts,
/// from `RUFFLEVITA_PHYS_ITERS` (0 = off). A lower cap makes physics-heavy
/// games run faster with visibly looser physics; it changes behaviour, so it
/// is opt-in and read once. See `Method::is_box2d_step`.
static APP_PHYS_CAP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Set by the app from the per-game "Physics speed" setting (0 = no cap). The
/// `RUFFLEVITA_PHYS_ITERS` environment variable, when set, overrides it (used
/// in timedemos and A/B tests).
pub fn set_phys_iter_cap(cap: u32) {
    APP_PHYS_CAP.store(cap, std::sync::atomic::Ordering::Relaxed);
}

pub fn phys_iter_cap() -> u32 {
    use std::sync::OnceLock;
    static ENV: OnceLock<Option<u32>> = OnceLock::new();
    if let Some(v) = *ENV.get_or_init(|| {
        std::env::var("RUFFLEVITA_PHYS_ITERS").ok().and_then(|v| v.parse::<u32>().ok())
    }) {
        return v;
    }
    APP_PHYS_CAP.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn exec<'gc>(
    method: Method<'gc>,
    scope: ScopeChain<'gc>,
    receiver: Value<'gc>,
    bound_superclass: Option<ClassObject<'gc>>,
    arguments: FunctionArgs<'_, 'gc>,
    activation: &mut Activation<'_, 'gc>,
    callee: Option<FunctionObject<'gc>>,
) -> Result<Value<'gc>, Error<'gc>> {
    let mc = activation.gc();

    // RuffleVita: cap the iteration counts of b2World.Step. The first argument
    // is the time step (a small float, left alone); every later argument is an
    // iteration count (one in Box2D 2.0's `Step(dt, iterations)`, two in 2.1's
    // `Step(dt, velocityIterations, positionIterations)`).
    let cap = phys_iter_cap();
    let clamped_storage;
    let arguments = if cap > 0 && arguments.len() >= 2 && method.is_box2d_step() {
        let mut v = arguments.to_slice().into_owned();
        for slot in v.iter_mut().skip(1) {
            let over = match *slot {
                Value::Integer(x) => x > cap as i32,
                Value::Number(x) => x > cap as f64,
                _ => false,
            };
            if over {
                *slot = Value::Integer(cap as i32);
            }
        }
        clamped_storage = v;
        FunctionArgs::from_slice(&clamped_storage)
    } else {
        arguments
    };

    let ret = match method.method_kind() {
        MethodKind::Native { native_method, .. } => {
            crate::rv_deep_zone!(Avm2Native);
            let caller_domain = activation.caller_domain();
            let caller_movie = activation.caller_movie_handle();
            let mut activation = Activation::from_builtin(
                activation.context,
                bound_superclass,
                scope,
                caller_domain,
                caller_movie,
            );

            method.resolve_info(&mut activation)?;

            let signature = method.resolved_param_config();

            // Check for too many arguments
            if arguments.len() > signature.len() && !method.is_variadic() && !method.is_unchecked()
            {
                return Err(Error::avm_error(make_mismatch_error(
                    &mut activation,
                    method,
                    arguments.len(),
                )?));
            }

            let arguments = activation.resolve_parameters(method, arguments, signature)?;

            #[cfg(feature = "tracy_avm")]
            let _span = {
                let mut name = WString::new();
                display_function(&mut name, method);
                let span = tracy_client::Client::running()
                    .expect("tracy_client should be running")
                    .span_alloc(None, &name.to_utf8_lossy(), "rust", 0, 0);
                span.emit_color(0x2c4980);
                span
            };

            activation.context.avm2.push_call(mc, method);

            native_method(&mut activation, receiver, &arguments)
        }
        MethodKind::Bytecode { .. } => {
            crate::rv_deep_zone!(Avm2CallSetup);
            // RuffleVita: throw like Flash instead of overflowing the native stack.
            if crate::rv_stack::exhausted() {
                return Err(stack_overflow(activation));
            }
            // We must initialize the stack frame here so the lifetime works out
            let stack = activation.context.avm2.stack;
            let stack_frame = stack.get_stack_frame(method);

            // This used to be a one step called Activation::from_method,
            // but avoiding moving an Activation around helps perf
            let mut activation = Activation::from_nothing(activation.context);
            if let Err(e) = activation.init_from_method(
                method,
                scope,
                receiver,
                arguments,
                stack_frame,
                bound_superclass,
                callee,
            ) {
                // If an error is thrown during verification or argument coercion,
                // we still need to call cleanup to dispose of the stack frame
                activation.cleanup();
                return Err(e);
            }

            #[cfg(feature = "tracy_avm")]
            let _span = {
                let mut name = WString::new();
                display_function(&mut name, method);
                let option = tracy_client::Client::running();
                let span = option.expect("tracy_client should be running").span_alloc(
                    None,
                    &name.to_utf8_lossy(),
                    method.owner_movie().url(),
                    line!(),
                    0,
                );
                span.emit_color(0x425fa1);
                span
            };

            activation.context.avm2.push_call(mc, method);

            // RuffleVita: run the AOT-compiled version if there is one for
            // exactly this op stream (see avm2/aot.rs).
            let result = match crate::avm2::aot::lookup(method) {
                Some(compiled) => {
                    crate::rv_deep_zone!(Avm2Aot);
                    #[cfg(feature = "rv_prof_ops")]
                    let _function = crate::rv_prof::enter_function(method.prof_id());
                    compiled(&mut activation, &method.get_verified_info().parsed_code)
                }
                None => activation.run_actions(method),
            };

            activation.cleanup();

            result
        }
    };
    activation.context.avm2.pop_call(mc);
    ret
}

impl fmt::Debug for BoundMethod<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("BoundMethod")
            .field("method", &self.method)
            .field("scope", &self.scope)
            .field("receiver", &self.bound_receiver)
            .finish()
    }
}

pub fn display_function<'gc>(output: &mut WString, method: Method<'gc>) {
    let bound_class = method.bound_class();

    if let Some(bound_class) = bound_class {
        let name = bound_class.name().to_qualified_name_no_mc();
        output.push_str(&name);
    }

    // NOTE: The name of a bytecode method refers to the name of the trait that contains the method,
    // rather than the name of the method itself.
    if let Some(bound_class) = bound_class {
        if bound_class.instance_init() == Some(method) {
            if bound_class.is_c_class() {
                // If the associated class is a c_class, its initializer
                // method is a class initializer.
                output.push_utf8("cinit");
            }
            // We purposely do nothing for instance initializers
        } else {
            let mut method_trait = None;

            for t in bound_class.traits() {
                if t.as_method().is_some_and(|tm| tm == method) {
                    method_trait = Some(t);
                    break;
                }
            }

            if let Some(method_trait) = method_trait {
                output.push_char('/');
                match method_trait.kind() {
                    TraitKind::Setter { .. } => output.push_utf8("set "),
                    TraitKind::Getter { .. } => output.push_utf8("get "),
                    _ => (),
                }
                if method_trait.name().namespace().is_namespace() {
                    output.push_str(&method_trait.name().to_qualified_name_no_mc());
                } else {
                    output.push_str(&method_trait.name().local_name());
                }
            } else if !method.method_name().is_empty() {
                // Last resort if we can't find a name anywhere else.
                // SWF's with debug information will provide a method name attached
                // to the method definition, so we can use that.
                output.push_char('/');
                output.push_utf8(&method.method_name());
            }
            // TODO: What happens if we can't find the trait?
        }
    } else if method.is_function() && !method.method_name().is_empty() {
        output.push_utf8("Function/");
        output.push_utf8(&method.method_name());
    } else {
        output.push_utf8("MethodInfo-");
        output.push_utf8(&method.abc_method_index().to_string());
    }

    output.push_utf8("()");
}

/// RuffleVita: "Error #1023: Stack overflow occurred." (see `rv_stack`).
#[cold]
#[inline(never)]
pub(crate) fn stack_overflow<'gc>(activation: &mut Activation<'_, 'gc>) -> Error<'gc> {
    match crate::avm2::error::error(activation, "Error #1023: Stack overflow occurred.", 1023) {
        Ok(err) => Error::avm_error(err),
        Err(err) => err,
    }
}
