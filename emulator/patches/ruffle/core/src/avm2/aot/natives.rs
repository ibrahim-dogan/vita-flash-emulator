//! RuffleVita: hand-written replacements for well-known library methods that
//! are far too slow to interpret on the Vita. `aot::resolve` picks them by
//! name, before the compiled table.

use crate::avm2::activation::Activation;
use crate::avm2::method::Method;
use crate::avm2::op::Op;
use crate::avm2::value::Value;
use crate::avm2::Error;

/// Whether `method` is Apparat's `apparat.lzma::LZMADecoder/code()`.
pub fn is_apparat_lzma_code(method: Method<'_>) -> bool {
    let Some(class) = method.bound_class() else { return false };
    &*class.name().local_name() == b"LZMADecoder"
        && super::method_name(method) == "apparat.lzma::LZMADecoder/code()"
}

/// `LZMADecoder.code(input, output, outSize)`, which Apparat's "Matryoshka"
/// packer runs at startup to unpack the real game (Super Mario Crossover 3).
/// Interpreted, it trips the 15 s script timeout even on a desktop.
/// Matryoshka has just read the 13-byte `.lzma` header (5 property bytes,
/// then the 8-byte size) from `input`, so the stream starts 13 bytes back;
/// any other use is interpreted as before.
pub fn apparat_lzma_code<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _ops: &[Op<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let input = activation.local_register(1);
    let output = activation.local_register(2);
    if decode(input, output).is_some() {
        return Ok(true.into());
    }
    let method = activation.context.avm2.call_stack().borrow().top();
    match method {
        Some(method) => activation.run_actions(method),
        None => Ok(false.into()),
    }
}

fn decode(input: Value<'_>, output: Value<'_>) -> Option<()> {
    let (input, output) = (input.as_object()?, output.as_object()?);
    let unpacked = {
        let bytes = input.as_bytearray()?;
        let start = bytes.position().checked_sub(13)?;
        let mut unpacked = Vec::new();
        lzma_rs::lzma_decompress(&mut &bytes.bytes()[start..], &mut unpacked).ok()?;
        bytes.set_position(bytes.len());
        unpacked
    };
    tracing::info!("Apparat LZMA: unpacked {} bytes natively", unpacked.len());
    let written = output.as_bytearray_mut()?.write_bytes(&unpacked);
    written.ok()
}
