use ruffle_core::backend::log::LogBackend;

/// Routes ActionScript `trace()` output into our log.
#[derive(Clone, Default)]
pub struct TracingLogBackend;

impl LogBackend for TracingLogBackend {
    fn avm_trace(&self, message: &str) {
        tracing::info!(target: "avm_trace", "{message}");
    }

    fn avm_warning(&self, message: &str) {
        tracing::info!(target: "avm_trace", "Warning: {message}");
    }
}
