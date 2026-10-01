//! RuffleVita: counts executed ops and adjacent op pairs, to decide which
//! fast paths and superinstructions are worth building. Only compiled with
//! the `rv_opstats` feature; `run_actions` calls `record` for every op.

use crate::avm2::op::Op;
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::Discriminant;

type Kind = Discriminant<Op<'static>>;

#[derive(Default)]
struct Stats {
    ops: HashMap<Kind, (u64, String)>,
    pairs: HashMap<(Kind, Kind), u64>,
    last: Option<Kind>,
    total: u64,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
}

fn kind(op: &Op<'_>) -> Kind {
    // SAFETY: the discriminant doesn't depend on the lifetime.
    unsafe { std::mem::transmute::<Discriminant<Op<'_>>, Kind>(std::mem::discriminant(op)) }
}

fn name(op: &Op<'_>) -> String {
    let s = format!("{op:?}");
    let end = s.find([' ', '{', '(']).unwrap_or(s.len());
    s[..end].to_owned()
}

#[inline(never)]
pub fn record(op: &Op<'_>) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        let k = kind(op);
        s.total += 1;
        s.ops.entry(k).or_insert_with(|| (0, name(op))).0 += 1;
        if let Some(last) = s.last {
            *s.pairs.entry((last, k)).or_default() += 1;
        }
        s.last = Some(k);
    })
}

/// A new method starts: don't pair its first op with the caller's last.
pub fn break_sequence() {
    STATS.with(|s| s.borrow_mut().last = None)
}

/// The top ops and op pairs since the last report, then resets.
pub fn report(top: usize) -> String {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        let total = s.total.max(1) as f64;
        let mut ops: Vec<_> = s.ops.values().cloned().collect();
        ops.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out = format!("AVM2 ops: {} executed\n", s.total);
        for (n, name) in ops.iter().take(top) {
            out += &format!("  {:5.2}%  {name}\n", *n as f64 * 100.0 / total);
        }
        let mut pairs: Vec<_> = s.pairs.iter().map(|(k, n)| (*n, *k)).collect();
        pairs.sort_by(|a, b| b.0.cmp(&a.0));
        out += "AVM2 op pairs:\n";
        for (n, (a, b)) in pairs.iter().take(top) {
            let na = &s.ops[a].1;
            let nb = &s.ops[b].1;
            out += &format!("  {:5.2}%  {na} > {nb}\n", *n as f64 * 100.0 / total);
        }
        *s = Stats::default();
        out
    })
}
