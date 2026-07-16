//! Shared JSPI spill-stack model primitives.
//!
//! JSPI (`WebAssembly.Suspending` / `WebAssembly.promising`) switches the
//! engine-managed native wasm stack per activation, but knows nothing about
//! the LLVM spill stack ("shadow stack") in linear memory. All activations
//! share one spill-stack region and one `__stack_pointer` global, so
//! concurrently suspended activations silently corrupt each other.
//!
//! This crate implements the eager slice-virtualization convention:
//! every suspension saves its own live spill slice `[sp, top)`, and every
//! resumption restores that slice and the stack pointer *at the true resume
//! boundary* (wasm-initiated, immediately after the suspending import
//! returns). Under this discipline arbitrary interleavings, non-LIFO wake
//! orders, and even overlapping slices are correct: whatever a sibling
//! scribbles or stale-restores into your range while you are suspended, your
//! own resume-restore heals before you run, and only one activation runs at
//! a time.
//!
//! Soundness invariants encoded here (do not weaken):
//!
//! 1. The save is synchronous with the suspend: it happens before the
//!    suspending import is called, so nothing can interleave.
//! 2. The restore is initiated from wasm after the suspending import
//!    returns. Restoring from the JS continuation is unsound: the engine
//!    resumes the wasm activation on a later tick, so another activation's
//!    restore can interleave and clobber the stack pointer or slice.
//! 3. The save id round-trips through the suspending import's *return
//!    channel*. Post-resume, pre-restore, the activation's own spill frame
//!    may hold sibling scribbles, so no value read from memory written
//!    before the suspension may be trusted. Engine-delivered values (wasm
//!    locals, return values) are safe.
//! 4. [`restore_stack`] is `#[inline(always)]` and must be called with
//!    nothing between the suspending import returning and the call. A
//!    non-inlined callee would allocate its frame at the stale stack
//!    pointer and its epilogue would rewrite the stack pointer after the
//!    restore fixed it.
//! 5. The suspending import must never throw into the resume boundary:
//!    unwinding before the restore would run landing pads against a
//!    scribbled, un-restored stack. Rejections are recorded JS-side and
//!    signalled through the id's high bit; [`suspend`] surfaces them as
//!    `Err` only after the restore completes.
//!
//! The activation *top* is tracked in a shared thread-local so that
//! independent JSPI integrations (wasm-bindgen, tokio, hand-written glue)
//! linking this crate agree on exact slice extents; without sharing, a
//! suspender that did not create the current activation would have to save
//! conservatively up to the stack base.

#[cfg(target_os = "emscripten")]
mod emscripten;

#[cfg(target_os = "emscripten")]
pub use emscripten::*;

#[cfg(not(target_os = "emscripten"))]
mod unsupported;

#[cfg(not(target_os = "emscripten"))]
pub use unsupported::*;

/// Padding for [`wasm_enter`], covering the promising entry's own frame
/// between its true entry stack pointer and the measurement point inside its
/// body. Overshoot above the true top is safe under the healing invariant
/// (over-saved bytes belong to a suspended ancestor, which re-heals them at
/// its own resume, or are dead); undershoot is not. Entry wrappers must be
/// thin enough that their frame fits in this padding, or use
/// [`wasm_enter_exact`] with a pre-entry measurement.
pub const STACK_TOP_PAD: usize = 4096;

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
/// a `lib` crate (rustc internalizes `#[no_mangle]` statics when compiling
/// a `bin`, dropping the wasm export emscripten's metadata extraction keys
/// on), and the static must be referenced from linked code so the archive
/// member is pulled in.
#[doc(hidden)]
#[macro_export]
macro_rules! em_js_data {
    ($sym:ident, $body:expr) => {
        #[no_mangle]
        #[used]
        pub static $sym: [u8; $crate::__em_js_len($body)] =
            $crate::__em_js_bytes::<{ $crate::__em_js_len($body) }>($body);
    };
}

/// Shared JS-side registry prelude. Every em_js body that touches shared
/// state starts with this. Hung off the per-instance `Module` object (in
/// scope for em_js bodies, per-instance under MODULARIZE and module-scoped
/// output): a `globalThis` registry would be shared across wasm instances
/// in one JS context, and the last-initialized instance's `setSp`/`getSp`/
/// `getTop`/`setTop` funcrefs would clobber the others' — cross-instance
/// stack-pointer writes, i.e. memory corruption.
#[doc(hidden)]
#[macro_export]
macro_rules! jspi_js_prelude {
    () => {
        "const J = Module.__jspi ??= { saves: new Map(), nsave: 1, promises: new Map(), npromise: 1 };"
    };
}
