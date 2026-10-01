//! RuffleVita: dumps the optimized op stream of chosen methods, as the AOT
//! translator will see it (desktop development only).
//!
//! `RUFFLEVITA_AOT_DUMP=<file>` turns it on; `RUFFLEVITA_AOT_METHODS` lists
//! the methods to dump, separated by `|`, as `rv_prof` names them (e.g.
//! `Box2D.Common.Math::b2Mat22/Set()`). Methods are dumped when first
//! verified, i.e. right after Ruffle's optimizer has run on them.

use crate::avm2::function::display_function;
use crate::avm2::method::Method;
use crate::avm2::op::Op;
use crate::string::WString;
use std::io::Write;
use std::sync::OnceLock;

struct Config {
    path: String,
    methods: Vec<String>,
}

fn config() -> Option<&'static Config> {
    static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let path = std::env::var("RUFFLEVITA_AOT_DUMP").ok()?;
            let methods = std::env::var("RUFFLEVITA_AOT_METHODS")
                .unwrap_or_default()
                .split('|')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            Some(Config { path, methods })
        })
        .as_ref()
}

pub fn after_verify<'gc>(method: Method<'gc>, ops: &[Op<'gc>]) {
    let Some(config) = config() else { return };
    let mut name = WString::new();
    display_function(&mut name, method);
    let name = name.to_utf8_lossy().into_owned();
    if !config.methods.is_empty() && !config.methods.iter().any(|m| *m == name) {
        return;
    }
    let mut out = format!("== {name} ({} ops)\n", ops.len());
    for (i, op) in ops.iter().enumerate() {
        let mut text = format!("{op:?}");
        if text.len() > 160 {
            text.truncate(160);
            text.push('…');
        }
        out += &format!("{i:4}  {text}\n");
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&config.path) {
        let _ = f.write_all(out.as_bytes());
    }
}
