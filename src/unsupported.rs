use std::any::Any;
use std::marker::PhantomData;

/// Always false: JSPI is only supported on wasm32-unknown-emscripten.
pub fn linked() -> bool {
    false
}

#[doc(hidden)]
pub fn stack_current() -> usize {
    unsupported()
}

#[doc(hidden)]
pub fn stack_base() -> usize {
    unsupported()
}

#[must_use = "the activation top is cleared when this guard drops"]
pub struct ActivationGuard {
    _not_send: PhantomData<*mut ()>,
}

pub fn wasm_enter() -> ActivationGuard {
    unsupported()
}

pub fn wasm_enter_exact(_top: usize) -> ActivationGuard {
    unsupported()
}

pub fn stack_top() -> Option<usize> {
    None
}

#[doc(hidden)]
pub fn save_stack() -> u32 {
    unsupported()
}

#[doc(hidden)]
pub fn restore_stack(_id: u32) {
    unsupported()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejected {
    pub pid: u32,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "jspi: awaited promise {} rejected", self.pid)
    }
}

impl std::error::Error for Rejected {}

#[doc(hidden)]
pub fn suspend(_pid: u32) -> Result<(), Rejected> {
    unsupported()
}

#[derive(Clone)]
pub struct Deferred {
    _not_send: PhantomData<*mut ()>,
}

impl Deferred {
    pub fn new(_timeout_ms: Option<f64>) -> Deferred {
        unsupported()
    }

    pub fn resolve(&self) {
        unsupported()
    }

    pub fn ptr_eq(_a: &Deferred, _b: &Deferred) -> bool {
        unsupported()
    }
}

pub fn suspend_on(_d: &Deferred) -> Result<(), Rejected> {
    unsupported()
}

#[must_use = "dropping a JoinHandle detaches the fiber"]
pub struct JoinHandle<T> {
    _marker: PhantomData<*mut T>,
}

impl<T> JoinHandle<T> {
    pub fn join(self) -> Result<T, Box<dyn Any + Send>> {
        unsupported()
    }
}

pub fn spawn<T: 'static>(_f: impl FnOnce() -> T + 'static) -> JoinHandle<T> {
    unsupported()
}

#[cfg(feature = "metrics")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Metrics {
    pub saves: u64,
    pub restores: u64,
    pub saved_bytes: u64,
    pub restored_bytes: u64,
}

#[cfg(feature = "metrics")]
pub fn metrics() -> Metrics {
    Metrics::default()
}

#[cfg(feature = "metrics")]
pub fn reset_metrics() {}

fn unsupported() -> ! {
    panic!("jspi: unsupported target (requires wasm32-unknown-emscripten)")
}
