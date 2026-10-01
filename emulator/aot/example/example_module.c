/*
 * What the translator's output is meant to look like, written by hand for
 * two small methods. It compiles against rv_api.h but links against no
 * runtime yet; test_example.c drives it with a mock runtime.
 *
 * Source:
 *
 *   public class Vec2 {
 *       public var x:Number;             // slot 1
 *       public var y:Number;             // slot 2
 *       public function length():Number { return Math.sqrt(x * x + y * y); }
 *   }
 *
 *   public function sum(values:Array, n:int):Number {
 *       var total:Number = 0;
 *       for (var i:int = 0; i < n; i++) {
 *           try { total += values[i]; } catch (e:TypeError) { total = NaN; }
 *       }
 *       return total;
 *   }
 */

#define RV_API_IMPLEMENT_HELPERS
#include "../rv_api.h"

#include <math.h>

/* Module-wide link indices, as the translator numbers them. */
enum { MN_INDEX = 0, NUM_MNAMES };      /* the MultinameL used by values[i] */
enum { CLS_TYPE_ERROR = 0, NUM_CLASSES };

/*
 * Vec2.length(). The optimizer proved `this` is a Vec2 and x/y are Number
 * slots, so the slot reads are the only runtime calls and both results are
 * unboxed straight into doubles. Math.sqrt was already turned into a
 * CallNative by Ruffle's optimizer; the translator knows that builtin and
 * emits sqrt() instead.
 */
rv_status RV_METHOD_NAME(9c1d4a7e02b35f61)(rv_ctx *ctx, const rv_api *rt, const rv_env *env,
                                           const rv_value *args, uint32_t nargs, rv_value *ret) {
    rv_value tmp;
    double x, y;
    (void)env;
    (void)nargs;

    rt->get_slot(ctx, &args[0], 1, &tmp);
    RV_TRY(rv_to_f64(ctx, rt, &tmp, &x)); /* a Number slot is always numeric; cheap guard */
    rt->get_slot(ctx, &args[0], 2, &tmp);
    RV_TRY(rv_to_f64(ctx, rt, &tmp, &y));

    *ret = rv_from_f64(sqrt(x * x + y * y));
    return RV_OK;
}

/*
 * sum(values, n). `total` and `i` become C locals. values[i] goes through
 * get_index (Ruffle's GetPropertyFast), whose result needs a generic `+`.
 */
rv_status RV_METHOD_NAME(4f0e2b9d8a6c1375)(rv_ctx *ctx, const rv_api *rt, const rv_env *env,
                                           const rv_value *args, uint32_t nargs, rv_value *ret) {
    const rv_value *values = &args[1];
    int32_t n = args[2].u.i; /* declared int: already coerced by the runtime */
    double total = 0.0;
    int32_t i;
    uint32_t depth = rt->scope_depth(ctx);
    (void)nargs;

    for (i = 0; i < n; i++) {
        rv_value elem, acc, sum;

        /* try { */
        RV_TRY_CATCH(rt->get_index(ctx, values, (double)i, env->mnames[MN_INDEX], &elem), catch_0);
        acc = rv_from_f64(total);
        RV_TRY_CATCH(rv_add(ctx, rt, &acc, &elem, &sum), catch_0);
        RV_TRY_CATCH(rv_to_f64(ctx, rt, &sum, &total), catch_0);
        goto next;
        /* } catch (e:TypeError) { */
    catch_0:
        if (!rt->exc_matches(ctx, env->classes[CLS_TYPE_ERROR])) {
            return RV_THROW; /* not ours: leave it pending for the caller */
        }
        {
            rv_value e;
            rt->exc_take(ctx, &e);
            (void)e;
        }
        rt->scope_truncate(ctx, depth);
        total = NAN;
        /* } */
    next:
        RV_POLL(ctx, rt); /* backward branch */
    }

    *ret = rv_from_f64(total);
    return RV_OK;
}

static const rv_link link_mnames[NUM_MNAMES] = {{0, 17}};
static const rv_link link_classes[NUM_CLASSES] = {{0, 3}};

static const rv_method_entry methods[] = {
    /* sorted by (abc, index) */
    {{0, 12}, 0x4f0e2b9d8a6c1375ull, RV_METHOD_NAME(4f0e2b9d8a6c1375), RV_MF_HAS_CATCH, 2, 1},
    {{0, 40}, 0x9c1d4a7e02b35f61ull, RV_METHOD_NAME(9c1d4a7e02b35f61), 0, 0, 1},
};

static const uint64_t abc_hashes[] = {0x0123456789abcdefull};

const rv_module rv_module_example = {
    RV_MODULE_MAGIC,
    RV_API_VERSION,
    sizeof(rv_module),
    0,
    0x563854a8911eda67ull,
    0xfeedfacecafebeefull,
    "example.swf",
    "hand-written example",
    1, abc_hashes,
    0, NULL,
    NUM_MNAMES, link_mnames,
    NUM_CLASSES, link_classes,
    0, NULL,
    0, NULL,
    0, NULL,
    sizeof(methods) / sizeof(methods[0]), methods,
    NULL,
    NULL,
};

#ifdef RV_BUILD_SUPRX
RV_DEFINE_SUPRX_ENTRY(rv_module_example)
#endif
