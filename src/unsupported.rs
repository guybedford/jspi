use crate::BlockingArgs;

/// Always false: JSPI is only supported on `wasm32-unknown-emscripten`.
pub fn linked() -> bool {
    false
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub fn spawn<R>(_f: impl FnOnce() -> R + Send + 'static) -> R {
    unsupported()
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub unsafe fn stack_root<R>(_f: impl FnOnce() -> R) -> R {
    unsupported()
}

/// Unsupported target: panics. See the emscripten implementation for the
/// real contract.
pub fn blocking_call<A: BlockingArgs>(_f: A::Fn, _args: A) {
    unsupported()
}

fn unsupported() -> ! {
    panic!("jspi: unsupported target (requires wasm32-unknown-emscripten)")
}
