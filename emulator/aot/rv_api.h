/*
 * rv_api.h: the C ABI between RuffleVita and ahead-of-time (AOT) compiled
 * ActionScript 3 methods.
 *
 * RuffleVita / İbrahim Doğan. Status: reference draft, API version 0.1.
 * Nothing implements this yet. It is the contract the AOT phase builds on;
 * see docs/PERFORMANCE_PLAN.md, section 3.
 *
 * How the pieces fit
 * ------------------
 *   1. The game is played on a PC build of RuffleVita with capture turned on.
 *      Each AVM2 method that runs is dumped *after* Ruffle's verifier and
 *      type-aware optimizer, together with everything the optimizer
 *      resolved (classes, slot numbers, dispatch ids).
 *   2. A translator turns every method whose ops it supports into one C
 *      function with the signature `rv_method_fn`, and emits one
 *      `rv_module` that describes them.
 *   3. The C is compiled for the Vita, either linked into the VPK (the
 *      prototype) or as a per-game .suprx that RuffleVita loads next to the
 *      .swf.
 *   4. The first time the interpreter is about to run a method, it looks the
 *      method up in the module and compares fingerprints. If they match it
 *      calls the C function instead of `Activation::run_actions`; if not, or
 *      if the method was never captured, it interprets as usual.
 *
 * Ground rules for generated code
 * -------------------------------
 *  - Every value that is not a plain number, int or bool is an opaque
 *    runtime handle. C code may copy handles and compare them for identity,
 *    and must do everything else through `rv_api`.
 *  - Every store into the heap goes through `rv_api`. The runtime owns
 *    gc-arena's write barriers.
 *  - Handles held in C locals stay valid for the duration of the call.
 *    Ruffle only collects garbage between frames, when no script is running.
 *    Handles must never be stored in C globals or statics.
 *  - Every `rv_api` function that can run ActionScript (getters, setters,
 *    valueOf, toString, proxies, calls) returns an `rv_status`. Generated code
 *    checks it after every such call (see RV_TRY and RV_TRY_CATCH).
 *  - Operand-stack slots become C locals. Local registers become C locals:
 *    `double`/`int32_t` when the optimizer proved a type, `rv_value`
 *    otherwise.
 *  - Backward branches call RV_POLL so the script timeout keeps working.
 *
 * Portability: plain C99 (no anonymous unions); builds with the VitaSDK's
 * arm-vita-eabi-gcc and with clang/gcc on desktop (where the prototype
 * statically links the same generated C for testing).
 */

#ifndef RV_API_H
#define RV_API_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------------ */
/* Versioning                                                               */
/* ------------------------------------------------------------------------ */

#define RV_API_VERSION_MAJOR 0
#define RV_API_VERSION_MINOR 1
#define RV_API_VERSION ((RV_API_VERSION_MAJOR << 16) | RV_API_VERSION_MINOR)

/* "RVAM" in memory order: first word of every rv_module and rv_handshake. */
#define RV_MODULE_MAGIC 0x4D415652u

/*
 * Compatibility rules:
 *  - A module built for major M loads only into a runtime with major M.
 *  - A runtime with minor N accepts modules built for minor <= N. New minor
 *    versions only append members to the end of rv_api, and the runtime
 *    fills `rv_api.size` so a module can check a member exists
 *    (RV_API_HAS).
 *  - Changing the meaning or signature of an existing member bumps major.
 */
#define RV_API_HAS(api, member) \
    ((api)->size >= offsetof(rv_api, member) + sizeof((api)->member))

/* ------------------------------------------------------------------------ */
/* Compiler helpers                                                         */
/* ------------------------------------------------------------------------ */

#if defined(__GNUC__) || defined(__clang__)
#define RV_LIKELY(x) __builtin_expect(!!(x), 1)
#define RV_UNLIKELY(x) __builtin_expect(!!(x), 0)
#define RV_INLINE static inline __attribute__((always_inline, unused))
#define RV_COLD __attribute__((cold, noinline))
#define RV_EXPORT __attribute__((visibility("default")))
#else
#define RV_LIKELY(x) (x)
#define RV_UNLIKELY(x) (x)
#define RV_INLINE static inline
#define RV_COLD
#define RV_EXPORT
#endif

#if defined(__cplusplus)
#define RV_STATIC_ASSERT(cond, msg) static_assert(cond, #msg)
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define RV_STATIC_ASSERT(cond, msg) _Static_assert(cond, #msg)
#else
#define RV_STATIC_ASSERT(cond, msg) typedef char rv_static_assert_##msg[(cond) ? 1 : -1]
#endif

/* ------------------------------------------------------------------------ */
/* Status codes                                                             */
/* ------------------------------------------------------------------------ */

typedef int32_t rv_status;

/* Success. */
#define RV_OK 0
/*
 * An ActionScript exception is pending in the context. It can be caught by
 * the method's own handlers (see rv_api.exc_matches / exc_take). If no
 * handler matches, return RV_THROW unchanged and the caller sees it.
 */
#define RV_THROW 1
/*
 * An error ActionScript can't catch: the script timeout, an internal
 * Ruffle error, a failed allocation. Return it immediately without running
 * any more ActionScript (not even `finally` blocks, which is also what the
 * interpreter does).
 */
#define RV_FATAL 2
/*
 * Only as the return value of an rv_method_fn, and only before the method
 * has had any side effect: "don't use me this time, interpret instead". A
 * generated function returns it when an entry guard fails.
 */
#define RV_DEOPT 3

/* ------------------------------------------------------------------------ */
/* Values                                                                   */
/* ------------------------------------------------------------------------ */

/*
 * Tags, matching the variants of Ruffle's avm2::Value. These numbers are
 * part of the ABI.
 */
#define RV_UNDEFINED 0u
#define RV_NULL 1u
#define RV_BOOL 2u
#define RV_NUMBER 3u /* u.num, an IEEE double */
#define RV_INT 4u    /* u.i; must be in [-2^28, 2^28), see rv_from_i32 */
#define RV_STRING 5u /* u.ptr, opaque AvmString handle */
#define RV_OBJECT 6u /* u.ptr + aux, opaque Object handle */

/*
 * A boxed AS3 value, 16 bytes, 8-byte aligned (the same size as Ruffle's
 * Value). Ruffle's Value is repr(Rust), so the runtime converts at the
 * boundary; that is a tag remap and a 16-byte copy.
 *
 *   tag  one of the RV_* tags above
 *   aux  RV_OBJECT: the runtime's object-kind number (opaque, but stable
 *        within one runtime build, so identity is `aux` + `u.ptr`). 0
 *        otherwise.
 *   u    the payload
 *
 * Two handles refer to the same object or interned string exactly when
 * tag, aux and u.ptr are all equal. Strings that compare equal as text may
 * still have different handles; use rv_api.strict_eq for string equality.
 */
typedef struct rv_value {
    uint32_t tag;
    uint32_t aux;
    union {
        double num;
        int32_t i;
        uint32_t b; /* RV_BOOL: 0 or 1 */
        void *ptr;
        uint64_t bits;
    } u;
} rv_value;

RV_STATIC_ASSERT(sizeof(rv_value) == 16, rv_value_is_16_bytes);

/* Integers stored as RV_INT stay inside 29 bits, as in Ruffle. */
#define RV_INT_MIN (-(1 << 28))
#define RV_INT_MAX ((1 << 28) - 1)

RV_INLINE rv_value rv_undefined(void) {
    rv_value v;
    v.tag = RV_UNDEFINED;
    v.aux = 0;
    v.u.bits = 0;
    return v;
}

RV_INLINE rv_value rv_null(void) {
    rv_value v;
    v.tag = RV_NULL;
    v.aux = 0;
    v.u.bits = 0;
    return v;
}

RV_INLINE rv_value rv_bool(int b) {
    rv_value v;
    v.tag = RV_BOOL;
    v.aux = 0;
    v.u.bits = 0;
    v.u.b = b ? 1u : 0u;
    return v;
}

/* A double is always RV_NUMBER, even when it's integral (Ruffle's From<f64>). */
RV_INLINE rv_value rv_from_f64(double d) {
    rv_value v;
    v.tag = RV_NUMBER;
    v.aux = 0;
    v.u.num = d;
    return v;
}

/* Ruffle's From<i32>: RV_INT inside 29 bits, RV_NUMBER outside. */
RV_INLINE rv_value rv_from_i32(int32_t i) {
    rv_value v;
    v.aux = 0;
    if (RV_LIKELY(i >= RV_INT_MIN && i <= RV_INT_MAX)) {
        v.tag = RV_INT;
        v.u.bits = 0;
        v.u.i = i;
    } else {
        v.tag = RV_NUMBER;
        v.u.num = (double)i;
    }
    return v;
}

/* Ruffle's From<u32>. */
RV_INLINE rv_value rv_from_u32(uint32_t u) {
    rv_value v;
    v.aux = 0;
    if (RV_LIKELY(u <= (uint32_t)RV_INT_MAX)) {
        v.tag = RV_INT;
        v.u.bits = 0;
        v.u.i = (int32_t)u;
    } else {
        v.tag = RV_NUMBER;
        v.u.num = (double)u;
    }
    return v;
}

RV_INLINE int rv_is_numeric(const rv_value *v) {
    return v->tag == RV_NUMBER || v->tag == RV_INT;
}

RV_INLINE int rv_is_nullish(const rv_value *v) {
    return v->tag <= RV_NULL;
}

/* Only valid when rv_is_numeric(v). */
RV_INLINE double rv_num(const rv_value *v) {
    return v->tag == RV_INT ? (double)v->u.i : v->u.num;
}

/* Identity: the same primitive bits or the same handle. Not AS3 `===`
 * (that needs rv_api.strict_eq for strings, XML and NaN). */
RV_INLINE int rv_same(const rv_value *a, const rv_value *b) {
    return a->tag == b->tag && a->aux == b->aux && a->u.bits == b->u.bits;
}

/*
 * ECMAScript ToUint32/ToInt32 on a double, bit-identical to Ruffle's
 * ecma_conversions::f64_to_wrapping_u32/_i32. The fast path is one VCVT;
 * only values outside +-2^31 take the slow path.
 */
RV_COLD uint32_t rv_f64_to_u32_slow(double d);

RV_INLINE int32_t rv_f64_to_i32(double d) {
    if (RV_LIKELY(d > -2147483649.0 && d < 2147483648.0)) {
        return (int32_t)d; /* truncates toward zero; NaN fails the test */
    }
    return (int32_t)rv_f64_to_u32_slow(d);
}

RV_INLINE uint32_t rv_f64_to_u32(double d) {
    if (RV_LIKELY(d > -1.0 && d < 4294967296.0)) {
        return (uint32_t)d;
    }
    return rv_f64_to_u32_slow(d);
}

#ifdef RV_API_IMPLEMENT_HELPERS
#include <math.h>
/* Define RV_API_IMPLEMENT_HELPERS in exactly one C file per module. */
RV_COLD uint32_t rv_f64_to_u32_slow(double d) {
    double m;
    if (!isfinite(d)) {
        return 0;
    }
    m = fmod(trunc(d), 4294967296.0);
    if (m < 0.0) {
        m += 4294967296.0;
    }
    return (uint32_t)m;
}
#endif

/* ------------------------------------------------------------------------ */
/* Runtime handles                                                          */
/* ------------------------------------------------------------------------ */

/*
 * Resolved runtime objects that aren't AS3 values. The C code never looks
 * inside them; it gets them from the module's link tables (rv_env) and
 * passes them back to rv_api.
 */
typedef const struct rv_multiname_s *rv_mname; /* avm2::Multiname */
typedef const struct rv_class_s *rv_class;     /* avm2::Class */
typedef const struct rv_method_s *rv_method;   /* avm2::Method (bytecode or native) */
typedef const struct rv_namespace_s *rv_ns;    /* avm2::Namespace */
typedef const struct rv_script_s *rv_script;   /* avm2::Script */

/*
 * The name operand of a multiname with runtime parts. RTQName takes `ns`,
 * MultinameL takes `name`, RTQNameL takes both; a static multiname takes no
 * rv_rtname at all (pass NULL). This one parameter covers Ruffle's
 * *Static / *Fast / *Slow op variants.
 */
typedef struct rv_rtname {
    const rv_value *name; /* NULL when the multiname has a static name */
    const rv_value *ns;   /* NULL when the multiname has static namespaces */
} rv_rtname;

/* ------------------------------------------------------------------------ */
/* The execution context                                                    */
/* ------------------------------------------------------------------------ */

/*
 * One rv_ctx per AOT call: the runtime's stand-in for an Activation. The
 * first members are public so hot checks can be inlined; the runtime's
 * private state follows. Never copy an rv_ctx or take sizeof of it.
 */
typedef struct rv_ctx {
    /*
     * Decremented by RV_POLL on every backward branch. When it reaches 0 the
     * generated code calls rv_api.poll, which checks the script timeout and
     * refills it.
     */
    uint32_t poll_budget;
    uint32_t flags; /* RV_CTX_* */
    /*
     * The current domain memory (ApplicationDomain.domainMemory) for
     * li8..lf64 / si8..sf64. The runtime keeps these current whenever control
     * is in generated code, including after any rv_api call returns; reload
     * them after each call instead of caching them across calls.
     * mem_base is NULL when there is no domain memory.
     */
    uint8_t *mem_base;
    uint32_t mem_len;
    uint32_t reserved0;
    /* runtime-private data follows */
} rv_ctx;

/* flags */
#define RV_CTX_DEBUG_BUILD 0x1u /* runtime was built with debug assertions */

/* ------------------------------------------------------------------------ */
/* Link tables                                                              */
/* ------------------------------------------------------------------------ */

/*
 * The module instance's resolved constants, filled in by the runtime when
 * the module loads (see rv_module's link descriptors). Every generated
 * function receives it. Indices are module-wide.
 */
typedef struct rv_env {
    const rv_value *strings;  /* interned; tag RV_STRING, rooted for the module's life */
    const rv_mname *mnames;
    const rv_class *classes;
    const rv_method *methods;
    const rv_ns *namespaces;
    const rv_script *scripts;
    void *module_data; /* whatever rv_module.init stored, else NULL */
} rv_env;

/* ------------------------------------------------------------------------ */
/* The method interface                                                     */
/* ------------------------------------------------------------------------ */

typedef struct rv_api rv_api;

/*
 * One compiled AS3 method.
 *
 *   ctx   the call's context
 *   rt    the runtime's function table
 *   env   the module's link tables
 *   args  args[0] is `this`. args[1..nargs] are the declared parameters,
 *         already coerced to their declared types, with default values
 *         filled in (what Activation::init_from_method does). If the method
 *         is variadic (`...rest` or uses `arguments`), the last entry is
 *         the rest Array or the arguments object.
 *   nargs number of entries in args (always >= 1)
 *   ret   receives the return value on RV_OK. For a method with a declared
 *         return type the generated code has already coerced it (Ruffle's
 *         ReturnValue { return_type }); for `void` it is rv_undefined().
 *
 * Returns RV_OK, RV_THROW, RV_FATAL, or RV_DEOPT (entry guard failed, no
 * side effects yet; the runtime interprets the method instead).
 *
 * The runtime has already pushed the call-stack entry, checked the script
 * timeout, set up the method's scope chain, and will pop it all afterwards
 * whatever the status. The scope stack depth on return must equal the depth
 * at entry (generated code pops its own pushscope/pushwith; on RV_THROW or
 * RV_FATAL the runtime truncates it).
 */
typedef rv_status (*rv_method_fn)(rv_ctx *ctx, const rv_api *rt, const rv_env *env,
                                  const rv_value *args, uint32_t nargs, rv_value *ret);

/* The name generated functions get, from the method's fingerprint. */
#define RV_METHOD_NAME(fp_hex) rv_m_##fp_hex

/* ------------------------------------------------------------------------ */
/* The runtime's function table                                             */
/* ------------------------------------------------------------------------ */

/* Hints for to_primitive. */
#define RV_HINT_NONE 0u
#define RV_HINT_NUMBER 1u
#define RV_HINT_STRING 2u

/* Results of lt (ECMA-262 abstract relational comparison). */
#define RV_CMP_FALSE 0
#define RV_CMP_TRUE 1
#define RV_CMP_UNDEFINED (-1) /* a NaN was involved */

/* Flags for call_prop. */
#define RV_CALL_LEX 0x1u  /* callproplex: pass null as the receiver */
#define RV_CALL_VOID 0x2u /* callpropvoid: `out` may be NULL */

/* Kinds for throw_error: the errors the interpreter raises itself. */
#define RV_ERR_TYPE 1u      /* TypeError */
#define RV_ERR_RANGE 2u     /* RangeError */
#define RV_ERR_REFERENCE 3u /* ReferenceError */
#define RV_ERR_ARGUMENT 4u  /* ArgumentError */
#define RV_ERR_VERIFY 5u    /* VerifyError */

/*
 * Every Ruffle Op maps onto inline C, one of these entries, or both (inline
 * fast path, call on the slow path). docs/PERFORMANCE_PLAN.md section 3.3
 * has the full op-by-op table.
 *
 * Conventions:
 *  - The first parameter is always the context.
 *  - Values go in by const pointer and come out through an `out` pointer
 *    that is never aliased with an input.
 *  - `argc`/`args` are a C array of values in call order, receiver excluded.
 *  - Functions that return void can't run ActionScript and can't fail.
 */
struct rv_api {
    uint32_t size;    /* sizeof(rv_api) as the runtime was built */
    uint32_t version; /* RV_API_VERSION of the runtime */

    /* --- Control ------------------------------------------------------- */

    /* Checks the script timeout and refills ctx->poll_budget. RV_OK or RV_FATAL. */
    rv_status (*poll)(rv_ctx *ctx);

    /* --- Exceptions ---------------------------------------------------- */

    /* Makes `v` the pending exception (op Throw). Always returns RV_THROW. */
    rv_status (*throw_value)(rv_ctx *ctx, const rv_value *v);
    /*
     * Raises the Error the interpreter would raise, e.g. (RV_ERR_TYPE, 1009)
     * for a null receiver, (RV_ERR_RANGE, 1506) for domain memory out of
     * range. `mname` names the property involved, or NULL. Returns RV_THROW.
     */
    rv_status (*throw_error)(rv_ctx *ctx, uint32_t kind, uint32_t code, rv_mname mname);
    /*
     * The null/undefined receiver error, exactly as Value::null_check
     * builds it (1009 for null, 1010 for undefined). Returns RV_THROW.
     */
    rv_status (*throw_null)(rv_ctx *ctx, const rv_value *receiver, rv_mname mname);
    /*
     * For catch blocks: 1 when the pending exception is an AS3 value that is
     * of type `cls` (NULL means `catch (e:*)`), 0 otherwise.
     */
    int (*exc_matches)(rv_ctx *ctx, rv_class cls);
    /* Moves the pending exception into *out and clears it. */
    void (*exc_take)(rv_ctx *ctx, rv_value *out);

    /* --- Conversions (Coerce*, Convert*) -------------------------------- */

    rv_status (*to_number)(rv_ctx *ctx, const rv_value *v, double *out);
    rv_status (*to_int32)(rv_ctx *ctx, const rv_value *v, int32_t *out);
    rv_status (*to_uint32)(rv_ctx *ctx, const rv_value *v, uint32_t *out);
    /* Only needed for strings; C handles the other tags inline (rv_truthy). */
    int (*to_boolean)(rv_ctx *ctx, const rv_value *v);
    /* ConvertS: ToString, null -> "null". */
    rv_status (*to_string)(rv_ctx *ctx, const rv_value *v, rv_value *out);
    /* CoerceS: like to_string, but null and undefined stay null. */
    rv_status (*coerce_s)(rv_ctx *ctx, const rv_value *v, rv_value *out);
    rv_status (*to_primitive)(rv_ctx *ctx, const rv_value *v, uint32_t hint, rv_value *out);
    /* Coerce { class } / CoerceSwapPop: Value::coerce_to_type. */
    rv_status (*coerce)(rv_ctx *ctx, const rv_value *v, rv_class cls, rv_value *out);
    /* ConvertO: throws 1009/1010 on null/undefined, else passes v through. */
    rv_status (*convert_o)(rv_ctx *ctx, const rv_value *v);

    /* --- Type tests ---------------------------------------------------- */

    int (*is_type)(rv_ctx *ctx, const rv_value *v, rv_class cls);
    void (*as_type)(rv_ctx *ctx, const rv_value *v, rv_class cls, rv_value *out);
    /* The *Late forms take the class as a value and can throw 1041. */
    rv_status (*is_type_late)(rv_ctx *ctx, const rv_value *v, const rv_value *cls, int *out);
    rv_status (*as_type_late)(rv_ctx *ctx, const rv_value *v, const rv_value *cls, rv_value *out);
    rv_status (*instance_of)(rv_ctx *ctx, const rv_value *v, const rv_value *type, int *out);
    void (*type_of)(rv_ctx *ctx, const rv_value *v, rv_value *out);

    /* --- Operators: the slow paths C doesn't inline ---------------------- */

    /* `+` for anything that isn't int+int or Number+Number (strings, XML, valueOf). */
    rv_status (*add)(rv_ctx *ctx, const rv_value *a, const rv_value *b, rv_value *out);
    /* a < b (abstract_lt): RV_CMP_TRUE/FALSE/UNDEFINED. > , <= , >= swap and invert. */
    rv_status (*lt)(rv_ctx *ctx, const rv_value *a, const rv_value *b, int *out);
    /* a == b (abstract_eq). */
    rv_status (*eq)(rv_ctx *ctx, const rv_value *a, const rv_value *b, int *out);
    /* a === b, for the cases rv_same can't decide (strings, NaN, int vs Number, XML). */
    int (*strict_eq)(rv_ctx *ctx, const rv_value *a, const rv_value *b);
    /* `name in obj` (op In). */
    rv_status (*in)(rv_ctx *ctx, const rv_value *name, const rv_value *obj, int *out);

    /* --- Slots (GetSlot, SetSlot, *GlobalSlot) ----------------------------- */

    /* obj must already be a non-null object (the translator null-checks inline). */
    void (*get_slot)(rv_ctx *ctx, const rv_value *obj, uint32_t slot, rv_value *out);
    /* Coerces to the slot's type, so it can throw. */
    rv_status (*set_slot)(rv_ctx *ctx, const rv_value *obj, uint32_t slot, const rv_value *v);
    /* SetSlotNoCoerce: the optimizer proved the type already matches. */
    void (*set_slot_nc)(rv_ctx *ctx, const rv_value *obj, uint32_t slot, const rv_value *v);
    void (*get_global_slot)(rv_ctx *ctx, uint32_t slot, rv_value *out);
    rv_status (*set_global_slot)(rv_ctx *ctx, uint32_t slot, const rv_value *v);

    /* --- Properties (GetProperty*, SetProperty*, InitProperty, ...) -------- */

    /* rt is NULL for a static multiname. All of these null-check obj. */
    rv_status (*get_prop)(rv_ctx *ctx, const rv_value *obj, rv_mname mn, const rv_rtname *rt,
                          rv_value *out);
    rv_status (*set_prop)(rv_ctx *ctx, const rv_value *obj, rv_mname mn, const rv_rtname *rt,
                          const rv_value *v);
    rv_status (*init_prop)(rv_ctx *ctx, const rv_value *obj, rv_mname mn, const rv_rtname *rt,
                           const rv_value *v);
    rv_status (*delete_prop)(rv_ctx *ctx, const rv_value *obj, rv_mname mn, const rv_rtname *rt,
                             int *out);
    /*
     * obj[index] where index is a number (the GetPropertyFast path: Array,
     * Vector, ByteArray, Dictionary). Falls back to the full lookup
     * internally, so it is always correct; `mn` is the MultinameL from the op.
     */
    rv_status (*get_index)(rv_ctx *ctx, const rv_value *obj, double index, rv_mname mn,
                           rv_value *out);
    rv_status (*set_index)(rv_ctx *ctx, const rv_value *obj, double index, rv_mname mn,
                           const rv_value *v);
    rv_status (*get_super)(rv_ctx *ctx, const rv_value *self, rv_mname mn, const rv_rtname *rt,
                           rv_value *out);
    rv_status (*set_super)(rv_ctx *ctx, const rv_value *self, rv_mname mn, const rv_rtname *rt,
                           const rv_value *v);
    rv_status (*get_descendants)(rv_ctx *ctx, const rv_value *obj, rv_mname mn,
                                 const rv_rtname *rt, rv_value *out);

    /* --- Scope (FindProp*, FindDef, GetLex, scope stack) ------------------- */

    /* FindProperty (strict = 0) and FindPropStrict (strict = 1). */
    rv_status (*find_prop)(rv_ctx *ctx, rv_mname mn, const rv_rtname *rt, int strict,
                           rv_value *out);
    rv_status (*find_def)(rv_ctx *ctx, rv_mname mn, rv_value *out);
    /* getlex: find_prop(strict) + get_prop in one call. */
    rv_status (*get_lex)(rv_ctx *ctx, rv_mname mn, rv_value *out);
    /* PushScope and PushWith null-check the value. */
    rv_status (*push_scope)(rv_ctx *ctx, const rv_value *v);
    rv_status (*push_with)(rv_ctx *ctx, const rv_value *v);
    void (*pop_scope)(rv_ctx *ctx);
    /* Depth of this call's scope stack, for restoring it in catch blocks. */
    uint32_t (*scope_depth)(rv_ctx *ctx);
    void (*scope_truncate)(rv_ctx *ctx, uint32_t depth);
    void (*get_scope_object)(rv_ctx *ctx, uint32_t index, rv_value *out);
    void (*get_outer_scope)(rv_ctx *ctx, uint32_t index, rv_value *out);
    /* GetScriptGlobals: can run the script's initializer, hence the status. */
    rv_status (*get_script_globals)(rv_ctx *ctx, rv_script script, rv_value *out);

    /* --- Calls ----------------------------------------------------------- */

    /* `out` may be NULL when the result is discarded. */

    /* Call: fn.call(receiver, args...). */
    rv_status (*call)(rv_ctx *ctx, const rv_value *fn, const rv_value *receiver, uint32_t argc,
                      const rv_value *args, rv_value *out);
    /* CallProperty / CallPropLex / CallPropVoid (flags RV_CALL_*). */
    rv_status (*call_prop)(rv_ctx *ctx, const rv_value *obj, rv_mname mn, const rv_rtname *rt,
                           uint32_t flags, uint32_t argc, const rv_value *args, rv_value *out);
    /* CallMethod: vtable dispatch by disp_id; obj must be non-null. */
    rv_status (*call_method)(rv_ctx *ctx, const rv_value *obj, uint32_t disp_id, uint32_t argc,
                             const rv_value *args, rv_value *out);
    /*
     * CallStatic, and CallNative (the optimizer's direct call of a builtin;
     * the runtime turns `method` back into the native function). When
     * `method` is itself AOT-compiled, the runtime calls it directly.
     */
    rv_status (*call_static)(rv_ctx *ctx, rv_method method, const rv_value *receiver,
                             uint32_t argc, const rv_value *args, rv_value *out);
    rv_status (*call_super)(rv_ctx *ctx, const rv_value *self, rv_mname mn, const rv_rtname *rt,
                            uint32_t argc, const rv_value *args, rv_value *out);

    /* --- Construction ---------------------------------------------------- */

    rv_status (*construct)(rv_ctx *ctx, const rv_value *ctor, uint32_t argc, const rv_value *args,
                           rv_value *out);
    rv_status (*construct_prop)(rv_ctx *ctx, const rv_value *obj, rv_mname mn,
                                const rv_rtname *rt, uint32_t argc, const rv_value *args,
                                rv_value *out);
    /* ConstructSuper: runs the superclass constructor on `self`. */
    rv_status (*construct_super)(rv_ctx *ctx, const rv_value *self, uint32_t argc,
                                 const rv_value *args);
    rv_status (*construct_slot)(rv_ctx *ctx, const rv_value *obj, uint32_t slot, uint32_t argc,
                                const rv_value *args, rv_value *out);
    /* ApplyType: Vector.<T>. */
    rv_status (*apply_type)(rv_ctx *ctx, const rv_value *base, uint32_t argc,
                            const rv_value *types, rv_value *out);
    /* NewObject: kv holds name0, value0, name1, value1, ... (2 * npairs values). */
    rv_status (*new_object)(rv_ctx *ctx, uint32_t npairs, const rv_value *kv, rv_value *out);
    rv_status (*new_array)(rv_ctx *ctx, uint32_t n, const rv_value *elems, rv_value *out);
    /* NewFunction: a closure over the current scope chain. */
    rv_status (*new_function)(rv_ctx *ctx, rv_method method, rv_value *out);
    /* NewClass: base is the superclass object, or null. Runs the class initializer. */
    rv_status (*new_class)(rv_ctx *ctx, rv_class cls, const rv_value *base, rv_value *out);
    rv_status (*new_activation)(rv_ctx *ctx, rv_class activation_class, rv_value *out);
    /* NewCatch: the catch scope object for the method's exception entry `index`. */
    rv_status (*new_catch)(rv_ctx *ctx, uint32_t index, rv_value *out);
    void (*push_namespace)(rv_ctx *ctx, rv_ns ns, rv_value *out);

    /* --- for-in / for-each ----------------------------------------------- */

    rv_status (*has_next)(rv_ctx *ctx, const rv_value *obj, int32_t index, int32_t *out);
    /*
     * HasNext2: updates the object and index registers in place. *out is 1
     * while there are more properties.
     */
    rv_status (*has_next2)(rv_ctx *ctx, rv_value *obj_reg, int32_t *index_reg, int *out);
    rv_status (*next_name)(rv_ctx *ctx, const rv_value *obj, int32_t index, rv_value *out);
    rv_status (*next_value)(rv_ctx *ctx, const rv_value *obj, int32_t index, rv_value *out);

    /* --- E4X and default XML namespace ------------------------------------ */

    rv_status (*esc_xattr)(rv_ctx *ctx, const rv_value *v, rv_value *out);
    rv_status (*esc_xelem)(rv_ctx *ctx, const rv_value *v, rv_value *out);
    rv_status (*check_filter)(rv_ctx *ctx, const rv_value *v);
    rv_status (*dxns)(rv_ctx *ctx, const rv_value *uri);
    rv_status (*dxns_late)(rv_ctx *ctx, const rv_value *uri);

    /* --- Domain memory ---------------------------------------------------- */

    /*
     * Li8..Lf64 and Si8..Sf64 are inline C on ctx->mem_base, bounds-checked
     * with RV_MEM_CHECK, which raises RangeError 1506 through throw_error.
     * No other entries are needed.
     */

    /* --- Diagnostics (optional; may be NULL) ------------------------------- */

    /* Called by generated code built with RV_TRACE_CALLS. */
    void (*trace_enter)(rv_ctx *ctx, uint32_t method_index);
    void (*trace_leave)(rv_ctx *ctx, uint32_t method_index, rv_status status);
};

/* ------------------------------------------------------------------------ */
/* Macros for generated code                                                */
/* ------------------------------------------------------------------------ */

/* Propagate anything that isn't RV_OK. */
#define RV_TRY(expr)                           \
    do {                                       \
        rv_status rv_s_ = (expr);              \
        if (RV_UNLIKELY(rv_s_ != RV_OK))       \
            return rv_s_;                      \
    } while (0)

/*
 * Inside a try block: jump to the handler label on an ActionScript
 * exception, propagate RV_FATAL.
 */
#define RV_TRY_CATCH(expr, label)              \
    do {                                       \
        rv_status rv_s_ = (expr);              \
        if (RV_UNLIKELY(rv_s_ != RV_OK)) {     \
            if (rv_s_ == RV_THROW)             \
                goto label;                    \
            return rv_s_;                      \
        }                                      \
    } while (0)

/* On every backward branch. */
#define RV_POLL(ctx, rt)                                   \
    do {                                                   \
        if (RV_UNLIKELY(--(ctx)->poll_budget == 0))        \
            RV_TRY((rt)->poll(ctx));                       \
    } while (0)

/* Throw 1009/1010 unless v is an object or a primitive with methods. */
#define RV_NULL_CHECK(ctx, rt, v, mn)                      \
    do {                                                   \
        if (RV_UNLIKELY(rv_is_nullish(v)))                 \
            return (rt)->throw_null((ctx), (v), (mn));     \
    } while (0)

/* Truthiness (IfTrue/IfFalse/Not/CoerceB), inline except for strings. */
RV_INLINE int rv_truthy(rv_ctx *ctx, const rv_api *rt, const rv_value *v) {
    switch (v->tag) {
    case RV_BOOL: return (int)v->u.b;
    case RV_INT: return v->u.i != 0;
    case RV_NUMBER: return v->u.num == v->u.num && v->u.num != 0.0;
    case RV_OBJECT: return 1;
    case RV_STRING: return rt->to_boolean(ctx, v);
    default: return 0; /* undefined, null */
    }
}

/* ToNumber with the numeric cases inline. */
RV_INLINE rv_status rv_to_f64(rv_ctx *ctx, const rv_api *rt, const rv_value *v, double *out) {
    if (RV_LIKELY(v->tag == RV_NUMBER)) {
        *out = v->u.num;
        return RV_OK;
    }
    if (v->tag == RV_INT) {
        *out = (double)v->u.i;
        return RV_OK;
    }
    return rt->to_number(ctx, v, out);
}

/* ToInt32 with the numeric cases inline. */
RV_INLINE rv_status rv_to_i32(rv_ctx *ctx, const rv_api *rt, const rv_value *v, int32_t *out) {
    if (RV_LIKELY(v->tag == RV_INT)) {
        *out = v->u.i;
        return RV_OK;
    }
    if (v->tag == RV_NUMBER) {
        *out = rv_f64_to_i32(v->u.num);
        return RV_OK;
    }
    return rt->to_int32(ctx, v, out);
}

/* The `+` operator with Ruffle's op_add fast paths inline. */
RV_INLINE rv_status rv_add(rv_ctx *ctx, const rv_api *rt, const rv_value *a, const rv_value *b,
                           rv_value *out) {
    if (a->tag == RV_INT && b->tag == RV_INT) {
        *out = rv_from_i32(a->u.i + b->u.i); /* both < 2^28, can't overflow */
        return RV_OK;
    }
    if (a->tag == RV_NUMBER && b->tag == RV_NUMBER) {
        *out = rv_from_f64(a->u.num + b->u.num);
        return RV_OK;
    }
    return rt->add(ctx, a, b, out);
}

/* a < b with the numeric cases inline: RV_CMP_TRUE/FALSE/UNDEFINED. */
RV_INLINE rv_status rv_lt(rv_ctx *ctx, const rv_api *rt, const rv_value *a, const rv_value *b,
                          int *out) {
    if (rv_is_numeric(a) && rv_is_numeric(b)) {
        double x = rv_num(a), y = rv_num(b);
        *out = (x != x || y != y) ? RV_CMP_UNDEFINED : (x < y ? RV_CMP_TRUE : RV_CMP_FALSE);
        return RV_OK;
    }
    return rt->lt(ctx, a, b, out);
}

/* Domain memory bounds check; `n` is the access width in bytes. */
#define RV_MEM_CHECK(ctx, rt, addr, n)                                          \
    do {                                                                        \
        if (RV_UNLIKELY((uint32_t)(addr) > (ctx)->mem_len ||                    \
                        (ctx)->mem_len - (uint32_t)(addr) < (uint32_t)(n)))     \
            return (rt)->throw_error((ctx), RV_ERR_RANGE, 1506u, NULL);         \
    } while (0)

/* ------------------------------------------------------------------------ */
/* Modules                                                                  */
/* ------------------------------------------------------------------------ */

/*
 * A link descriptor names an item in one of the SWF's ABC blocks: the
 * `abc`-th DoABC/DoABC2 tag in load order (index into rv_module.abc_hashes)
 * and an index into that block's constant pool or method/class table. The
 * runtime resolves each one to a handle when the module loads, exactly as
 * the interpreter would, and stores it in rv_env.
 */
typedef struct rv_link {
    uint32_t abc;
    uint32_t index;
} rv_link;

/* rv_method_entry.flags */
#define RV_MF_VARIADIC 0x1u      /* last arg is the rest Array / arguments object */
#define RV_MF_HAS_CATCH 0x2u     /* has try/catch (uses exc_* and scope_truncate) */
#define RV_MF_NEEDS_ACTIVATION 0x4u
#define RV_MF_SETS_DXNS 0x8u

typedef struct rv_method_entry {
    rv_link method; /* the ABC method this compiles */
    /*
     * 64-bit FNV-1a over: the method body's bytes, the Ruffle Op stream the
     * optimizer produced for it (including the classes, slots and dispatch
     * ids it resolved), and the layouts of the classes it touches. The
     * runtime computes the same hash the first time it would interpret the
     * method and uses `fn` only if they match.
     */
    uint64_t fingerprint;
    rv_method_fn fn;
    uint32_t flags; /* RV_MF_* */
    uint16_t num_params; /* declared parameters, excluding `this` and rest */
    uint16_t max_scope_depth;
} rv_method_entry;

/*
 * Everything a module exports. All pointers point into the module's own
 * read-only data. Entries in `methods` are sorted by (method.abc,
 * method.index) so the runtime can binary-search them.
 */
typedef struct rv_module {
    uint32_t magic;       /* RV_MODULE_MAGIC */
    uint32_t api_version; /* RV_API_VERSION the module was generated for */
    uint32_t size;        /* sizeof(rv_module) */
    uint32_t flags;       /* none yet, 0 */

    /* First 8 bytes of the Ruffle commit the capture ran on (e.g. 0x563854a8911eda67). */
    uint64_t ruffle_rev;
    /* 64-bit FNV-1a of the uncompressed SWF, header excluded. */
    uint64_t swf_hash;
    const char *swf_name; /* for logs */
    const char *generator; /* translator name and version, for logs */

    uint32_t num_abcs;
    const uint64_t *abc_hashes; /* FNV-1a of each ABC block, load order */

    /* Link tables, resolved into rv_env at load. */
    uint32_t num_strings;
    const rv_link *strings; /* index = ABC string pool index */
    uint32_t num_mnames;
    const rv_link *mnames; /* index = ABC multiname pool index */
    uint32_t num_classes;
    const rv_link *classes; /* index = ABC class index */
    uint32_t num_methods_linked;
    const rv_link *methods_linked; /* index = ABC method index (callees, closures) */
    uint32_t num_namespaces;
    const rv_link *namespaces; /* index = ABC namespace pool index */
    uint32_t num_scripts;
    const rv_link *scripts; /* index = ABC script index */

    uint32_t num_methods;
    const rv_method_entry *methods;

    /*
     * Optional, may be NULL. `init` runs once after linking, `fini` before
     * unloading. `init` may store a pointer in env->module_data.
     */
    rv_status (*init)(const rv_api *rt, rv_env *env);
    void (*fini)(const rv_env *env);
} rv_module;

/*
 * Loading
 * -------
 * Statically linked (the prototype): the build lists every generated
 * module in a table the runtime scans by swf_hash:
 *
 *     extern const rv_module rv_module_1f2e3d4c5b6a7980;
 *
 * As a .suprx next to the game (game.swf -> game.rv.suprx): RuffleVita
 * calls sceKernelLoadStartModule(path, sizeof(rv_handshake), &hs, ...).
 * The module's module_start receives the handshake as its argp and fills in
 * `module`. No NID imports or exports are needed in either direction: the
 * module reaches the runtime only through `api`.
 */
typedef struct rv_handshake {
    uint32_t magic;       /* RV_MODULE_MAGIC, set by the runtime */
    uint32_t api_version; /* the runtime's RV_API_VERSION */
    const rv_api *api;    /* the runtime's table, for modules that want it early */
    const rv_module *module; /* out: set by the module */
} rv_handshake;

/* module_start return values (SCE_KERNEL_START_SUCCESS / _FAILED). */
#define RV_SUPRX_START_SUCCESS 0
#define RV_SUPRX_START_FAILED 2

/*
 * Defines module_start/module_stop for a .suprx that exports `mod`. Link
 * with -nostartfiles and the usual vita-elf-create / vita-make-fself -s
 * steps.
 */
#define RV_DEFINE_SUPRX_ENTRY(mod)                                                     \
    int module_start(unsigned int argc, void *argp) {                                  \
        rv_handshake *hs = (rv_handshake *)argp;                                       \
        if (argc < sizeof(rv_handshake) || hs == NULL || hs->magic != RV_MODULE_MAGIC ||  \
            (hs->api_version >> 16) != RV_API_VERSION_MAJOR)                           \
            return RV_SUPRX_START_FAILED;                                              \
        hs->module = &(mod);                                                           \
        return RV_SUPRX_START_SUCCESS;                                                 \
    }                                                                                  \
    int module_stop(unsigned int argc, void *argp) {                                   \
        (void)argc;                                                                    \
        (void)argp;                                                                    \
        return RV_SUPRX_START_SUCCESS;                                                 \
    }

#ifdef __cplusplus
}
#endif

#endif /* RV_API_H */
