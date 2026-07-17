use std::time::Duration;

use crate::{BlockingArgs, EnterError};

/// Always false: JSPI is only supported on `wasm32-unknown-emscripten`.
pub fn jspi_enabled() -> bool {
    false
}

/// Always false: JSPI is only supported on `wasm32-unknown-emscripten`.
pub fn in_promising() -> bool {
    false
}

/// Always false: JSPI is only supported on `wasm32-unknown-emscripten`.
pub fn exclusive_promising() -> bool {
    false
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn enter_promising<R>(_f: impl FnOnce() -> R + Send + 'static) -> Result<R, EnterError> {
    unsupported()
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn enter_promising_exclusive<R>(
    _f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, EnterError> {
    unsupported()
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn blocking_call<A: BlockingArgs>(_f: A::Fn, _args: A) {
    unsupported()
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn sleep(_dur: Duration) {
    unsupported()
}

fn unsupported() -> ! {
    panic!("jspi: unsupported target (requires wasm32-unknown-emscripten)")
}
