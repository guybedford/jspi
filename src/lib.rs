//! JSPI spill-stack primitives for `wasm32-unknown-emscripten`.
//! Safe blocking calls to async JavaScript from Rust, without stack corruption.
//!
//! JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) suspends the
//! engine-managed native wasm stack per activation. LLVM-compiled code also
//! uses a spill ("shadow") stack in linear memory through the
//! `__stack_pointer` global, which JSPI knows nothing about: concurrently
//! suspended activations share one spill stack and one stack pointer, and
//! silently corrupt each other unless every suspension saves its live slice
//! and every resumption restores it.
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
//! }
//! ```

#[cfg(all(target_os = "emscripten", target_feature = "atomics"))]
compile_error!(
    "jspi is incompatible with -pthread / target_feature=atomics: \
     JSPI activations share one real thread"
);

mod args;
pub use args::BlockingArgs;

#[cfg(target_os = "emscripten")]
mod emscripten;
#[cfg(target_os = "emscripten")]
pub use emscripten::{blocking_call, enter_promising, linked};

#[cfg(not(target_os = "emscripten"))]
mod unsupported;
#[cfg(not(target_os = "emscripten"))]
pub use unsupported::{blocking_call, enter_promising, linked};

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
