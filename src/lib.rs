//! JSPI spill-stack primitives for `wasm32-unknown-emscripten`: safe
//! blocking calls to async JavaScript from Rust, without stack corruption.
//!
//! JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) suspends the
//! engine-managed native wasm stack per activation. LLVM-compiled code also
//! uses a spill ("shadow") stack in linear memory through the
//! `__stack_pointer` global, which JSPI knows nothing about: concurrently
//! suspended activations share one spill stack and one stack pointer, and
//! silently corrupt each other unless every suspension saves its live slice
//! and every resumption restores it.
//!
//! This crate is that eager save/restore convention reduced to primitives.
//! **The foreign call is the Suspending thing** — consumers declare their
//! own async imports (the `__asyncjs__` name prefix under `-sJSPI`) and call
//! them directly through [`blocking_call`]; the crate supplies only the
//! stack discipline that makes this sound.
//!
//! ```ignore
//! // first statement of a promising-entered function (glue entry, or
//! // main itself — -sJSPI auto-wraps main):
//! unsafe {
//!     jspi::enter_promising(|| {
//!         // ordinary Rust; blocking_call parks this activation until
//!         // the import's promise settles
//!         jspi::blocking_call(glue_fetch, (url_ptr as usize, url_len));
//!         let result = glue_take_result(); // plain import, post-restore
//!     })
//!     .unwrap()
//! }
//! ```
//!
//! **Healing invariant** (the correctness core): with eager save at every
//! suspension and eager restoration at every resumption, arbitrary
//! interleavings, non-LIFO wake orders, and overlapping activation ranges
//! are correct. Whatever a sibling scribbles into a parked slice, the
//! owner's own restore undoes before its code runs; only one activation
//! executes at a time (resumes fire only from an empty native stack).

#[cfg(all(target_os = "emscripten", target_feature = "atomics"))]
compile_error!(
    "jspi is incompatible with -pthread / target_feature=atomics: \
     JSPI activations share one real thread"
);

mod args;
pub use args::BlockingArgs;

/// Denial reasons for [`enter_promising`] / [`enter_promising_exclusive`]:
/// preconditions checked before any user code runs, returned as values so
/// hosts can observe and route them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterError {
    /// Attempted inside an already-open activation on this native stack.
    Nested,
    /// An exclusive activation owns promising ([`exclusive_promising`]).
    Exclusive,
    /// Exclusive entry denied: sibling activations are parked, so
    /// exclusivity cannot be granted.
    Parked,
}

impl std::fmt::Display for EnterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EnterError::Nested => "jspi: enter inside an open activation",
            EnterError::Exclusive => "jspi: an exclusive activation owns promising",
            EnterError::Parked => "jspi: exclusive entry with parked activations",
        })
    }
}

impl std::error::Error for EnterError {}

#[cfg(target_os = "emscripten")]
mod emscripten;
#[cfg(target_os = "emscripten")]
pub use emscripten::{
    blocking_call, enter_promising, enter_promising_exclusive, exclusive_promising, in_promising,
    jspi_enabled,
};
#[cfg(target_os = "emscripten")]
mod sleep;
#[cfg(target_os = "emscripten")]
pub use sleep::sleep;

#[cfg(not(target_os = "emscripten"))]
mod unsupported;
#[cfg(not(target_os = "emscripten"))]
pub use unsupported::{
    blocking_call, enter_promising, enter_promising_exclusive, exclusive_promising, in_promising,
    jspi_enabled, sleep,
};

#[doc(hidden)]
pub const fn __em_js_len(s: &str) -> usize {
    s.len() + 1
}

#[doc(hidden)]
pub const fn __em_js_bytes<const N: usize>(s: &str) -> [u8; N] {
    let mut a = [0u8; N];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        a[i] = b[i];
        i += 1;
    }
    a
}

/// Emit an emscripten EM_JS-convention JS function from Rust.
///
/// `$sym` must be `__em_js__<name>` (or `__em_js____asyncjs__<name>` for an
/// import that should receive `WebAssembly.Suspending` treatment under
/// `-sJSPI`), and `$body` the `"(args)<::>{ body }"` string. Must be used in
/// a `lib` crate (rustc internalizes `#[no_mangle]` statics when compiling a
/// `bin`, dropping the data export emscripten's metadata extraction keys
/// on), and the static must be referenced from linked code (an
/// `#[inline(never)]` anchor using `black_box`) so the archive member is
/// pulled in. Never add `#[link_section]`: rustc emits wasm custom sections,
/// breaking extraction.
#[doc(hidden)]
#[macro_export]
macro_rules! em_js_data {
    ($sym:ident, $body:expr) => {
        #[unsafe(no_mangle)]
        #[used]
        pub static $sym: [u8; $crate::__em_js_len($body)] =
            $crate::__em_js_bytes::<{ $crate::__em_js_len($body) }>($body);
    };
}
