/*
 * Runs example_module.c against a tiny mock runtime, and checks the inline
 * ToInt32/ToUint32 helpers against the ECMAScript definition Ruffle uses.
 *
 *   cc -std=c99 -Wall -Wextra -O2 test_example.c example_module.c -lm && ./a.out
 */

#include "../rv_api.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

extern const rv_module rv_module_example;

/* --- mock runtime ------------------------------------------------------- */

typedef struct mock_ctx {
    rv_ctx pub;
    int pending;
    rv_value exc;
    uint32_t depth;
    uint32_t polls;
} mock_ctx;

/* A fake object: a Vec2 or an array of doubles. `u.ptr` points at one. */
typedef struct mock_obj {
    double slots[3];
    const rv_value *elems;
    uint32_t len;
} mock_obj;

static rv_value obj_value(mock_obj *o) {
    rv_value v;
    v.tag = RV_OBJECT;
    v.aux = 1;
    v.u.bits = 0;
    v.u.ptr = o;
    return v;
}

static rv_status m_poll(rv_ctx *ctx) {
    ((mock_ctx *)ctx)->polls++;
    ctx->poll_budget = 4;
    return RV_OK;
}

static rv_status m_throw_error(rv_ctx *ctx, uint32_t kind, uint32_t code, rv_mname mn) {
    mock_ctx *m = (mock_ctx *)ctx;
    (void)mn;
    m->pending = 1;
    m->exc = rv_from_i32((int32_t)(kind * 10000 + code));
    return RV_THROW;
}

static int m_exc_matches(rv_ctx *ctx, rv_class cls) {
    (void)cls;
    return ((mock_ctx *)ctx)->pending; /* every error is a TypeError here */
}

static void m_exc_take(rv_ctx *ctx, rv_value *out) {
    mock_ctx *m = (mock_ctx *)ctx;
    *out = m->exc;
    m->pending = 0;
}

static rv_status m_to_number(rv_ctx *ctx, const rv_value *v, double *out) {
    (void)ctx;
    *out = v->tag == RV_BOOL ? (double)v->u.b : NAN;
    return RV_OK;
}

static void m_get_slot(rv_ctx *ctx, const rv_value *obj, uint32_t slot, rv_value *out) {
    (void)ctx;
    *out = rv_from_f64(((mock_obj *)obj->u.ptr)->slots[slot]);
}

static rv_status m_get_index(rv_ctx *ctx, const rv_value *obj, double index, rv_mname mn,
                             rv_value *out) {
    mock_obj *o = (mock_obj *)obj->u.ptr;
    (void)mn;
    if (index >= o->len) {
        return m_throw_error(ctx, RV_ERR_TYPE, 1009, NULL); /* pretend: undefined.foo */
    }
    *out = o->elems[(uint32_t)index];
    return RV_OK;
}

static rv_status m_add(rv_ctx *ctx, const rv_value *a, const rv_value *b, rv_value *out) {
    double x, y;
    m_to_number(ctx, a, &x);
    m_to_number(ctx, b, &y);
    if (rv_is_numeric(a)) x = rv_num(a);
    if (rv_is_numeric(b)) y = rv_num(b);
    *out = rv_from_f64(x + y);
    return RV_OK;
}

static uint32_t m_scope_depth(rv_ctx *ctx) {
    return ((mock_ctx *)ctx)->depth;
}

static void m_scope_truncate(rv_ctx *ctx, uint32_t depth) {
    ((mock_ctx *)ctx)->depth = depth;
}

/* --- checks ------------------------------------------------------------- */

static int failures;

#define CHECK(cond)                                                    \
    do {                                                               \
        if (!(cond)) {                                                 \
            printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);     \
            failures++;                                                \
        }                                                              \
    } while (0)

/* ECMA-262 ToUint32, written the slow, obvious way. */
static uint32_t ref_to_u32(double d) {
    double m;
    if (!isfinite(d)) return 0;
    m = fmod(trunc(d), 4294967296.0);
    if (m < 0) m += 4294967296.0;
    return (uint32_t)m;
}

static void test_conversions(void) {
    static const double cases[] = {
        0.0, -0.0, 0.5, -0.5, 1.9, -1.9, 255.0, 2147483647.0, 2147483647.9, 2147483648.0,
        -2147483648.0, -2147483648.9, -2147483649.0, 4294967295.0, 4294967296.0, 4294967297.5,
        -4294967296.0, 1e20, -1e20, 9007199254740993.0, INFINITY, -INFINITY, NAN, 123456789.75,
    };
    size_t i;
    for (i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
        double d = cases[i];
        uint32_t want = ref_to_u32(d);
        CHECK(rv_f64_to_u32(d) == want);
        CHECK(rv_f64_to_i32(d) == (int32_t)want);
    }
    CHECK(rv_from_i32(1 << 28).tag == RV_NUMBER);
    CHECK(rv_from_i32((1 << 28) - 1).tag == RV_INT);
    CHECK(rv_from_i32(-(1 << 28)).tag == RV_INT);
    CHECK(rv_from_i32(-(1 << 28) - 1).tag == RV_NUMBER);
    CHECK(rv_from_u32(0x80000000u).tag == RV_NUMBER && rv_from_u32(0x80000000u).u.num == 2147483648.0);
}

int main(void) {
    rv_api api;
    rv_env env;
    mock_ctx ctx;
    rv_value args[3], ret;
    mock_obj vec = {{0.0, 3.0, 4.0}, NULL, 0};
    rv_value elems[5];
    mock_obj arr;
    static const rv_class classes[1] = {NULL};
    static const rv_mname mnames[1] = {NULL};
    const rv_module *mod = &rv_module_example;

    test_conversions();

    memset(&api, 0, sizeof api);
    api.size = sizeof api;
    api.version = RV_API_VERSION;
    api.poll = m_poll;
    api.throw_error = m_throw_error;
    api.exc_matches = m_exc_matches;
    api.exc_take = m_exc_take;
    api.to_number = m_to_number;
    api.get_slot = m_get_slot;
    api.get_index = m_get_index;
    api.add = m_add;
    api.scope_depth = m_scope_depth;
    api.scope_truncate = m_scope_truncate;
    CHECK(RV_API_HAS(&api, trace_leave));

    memset(&env, 0, sizeof env);
    env.classes = classes;
    env.mnames = mnames;

    memset(&ctx, 0, sizeof ctx);
    ctx.pub.poll_budget = 4;

    CHECK(mod->magic == RV_MODULE_MAGIC && mod->num_methods == 2);

    /* Vec2(3, 4).length() == 5 */
    args[0] = obj_value(&vec);
    CHECK(mod->methods[1].fn(&ctx.pub, &api, &env, args, 1, &ret) == RV_OK);
    CHECK(ret.tag == RV_NUMBER && ret.u.num == 5.0);

    /* sum([1, 2.5, true, 4, 5], 5) == 13.5, with the Bool going through rt->add */
    elems[0] = rv_from_i32(1);
    elems[1] = rv_from_f64(2.5);
    elems[2] = rv_bool(1);
    elems[3] = rv_from_i32(4);
    elems[4] = rv_from_f64(5.0);
    arr.elems = elems;
    arr.len = 5;
    args[0] = rv_undefined();
    args[1] = obj_value(&arr);
    args[2] = rv_from_i32(5);
    CHECK(mod->methods[0].fn(&ctx.pub, &api, &env, args, 3, &ret) == RV_OK);
    CHECK(ret.tag == RV_NUMBER && ret.u.num == 13.5);
    CHECK(ctx.polls == 1); /* five backward branches, budget of four */

    /* Reading past the end throws inside the try; the catch sets NaN. */
    args[2] = rv_from_i32(6);
    CHECK(mod->methods[0].fn(&ctx.pub, &api, &env, args, 3, &ret) == RV_OK);
    CHECK(ret.tag == RV_NUMBER && ret.u.num != ret.u.num);
    CHECK(ctx.pending == 0);

    if (failures == 0) {
        printf("all checks passed\n");
    }
    return failures != 0;
}
