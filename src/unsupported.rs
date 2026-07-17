use crate::BlockingArgs;

/// Always false: JSPI is only supported on `wasm32-unknown-emscripten`.
pub fn jspi_enabled() -> bool {
    false
}

/// Always false: JSPI is only supported on `wasm32-unknown-emscripten`.
pub fn in_promising() -> bool {
    false
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn enter_promising<R>(_f: impl FnOnce() -> R + Send + 'static) -> R {
    unsupported()
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn blocking_call<A: BlockingArgs>(_f: A::Fn, _args: A) {
    unsupported()
}

fn unsupported() -> ! {
    panic!("jspi: unsupported target (requires wasm32-unknown-emscripten)")
}
