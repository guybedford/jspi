use std::cell::{Cell, RefCell};
use std::mem::MaybeUninit;
use std::ptr;

use crate::{BlockingArgs, EnterError};

/// Heap save-state for one parked bracket; heap and statics are never
/// inside any saved slice.
struct Meta {
    sp: usize,
    len: usize,
    buf: Box<[MaybeUninit<u8>]>,
}

thread_local! {
    /// Parity: parked activations hold scope + bracket = +2, invisible.
    /// Even permits `spawn`; odd permits `blocking_call`.
    static COUNTER: Cell<usize> = const { Cell::new(0) };
    /// The exclusive-owner bit: deliberately NOT part of COUNTER, so it
    /// stays raised while the owner is parked — that is the whole point.
    static EXCLUSIVE: Cell<bool> = const { Cell::new(false) };
    /// Parked save-state, keyed by frame probe address.
    static REGISTRY: RefCell<Vec<(usize, Meta)>> = const { RefCell::new(Vec::new()) };
    /// Meta transit across the restore instant: nothing may cross the
    /// restore in a frame slot.
    static IN_FLIGHT: Cell<Option<Meta>> = const { Cell::new(None) };
}

unsafe extern "C" {
    fn emscripten_stack_get_current() -> usize;
    fn emscripten_stack_get_base() -> usize;
    // compiler-rt stack_ops.S: frame-free at every opt level
    fn _emscripten_stack_restore(sp: usize);
}

crate::em_js_data!(
    __em_js__jspi_linked,
    "()<::>{ return (typeof Asyncify !== 'undefined' && !!Asyncify.makeAsyncFunction && typeof WebAssembly.promising === 'function') ? 1 : 0; }"
);

// The resume-boundary restore: zero wasm frames (wasm code would clobber or
// be clobbered by the region it rewrites). HEAPU8 is refreshed on growth.
crate::em_js_data!(
    __em_js__jspi_restore,
    "(buf, sp, len, setSp)<::>{ wasmExports.__indirect_function_table.get(setSp)(sp); HEAPU8.copyWithin(sp, buf, buf + len); }"
);

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn jspi_linked() -> i32;
    fn jspi_restore(buf: usize, sp: usize, len: usize, set_sp: usize);
}

#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js__jspi_linked.as_ptr(),
        __em_js__jspi_restore.as_ptr(),
    ));
}

/// Whether JSPI suspension is available: linked with `-sJSPI` on a
/// supporting host.
pub fn jspi_enabled() -> bool {
    anchor();
    unsafe { jspi_linked() != 0 }
}

/// Whether the currently executing code is inside an [`enter_promising`]
/// scope (with no bracket in flight): a suspendable activation is open on
/// this native stack and [`blocking_call`] is permitted. False from a
/// plain host callback, even while a sibling activation is parked.
pub fn in_promising() -> bool {
    COUNTER.get() & 1 == 1
}

/// Whether an exclusive activation ([`enter_promising_exclusive`]) owns
/// promising — including while it is parked. Distinct question from
/// [`in_promising`]: the owner holds the lock across its suspensions.
pub fn exclusive_promising() -> bool {
    EXCLUSIVE.get()
}

/// The suspendable-activation scope: call as the first statement of a
/// function entered from JS through `WebAssembly.promising` (glue entry,
/// or main itself under `-sJSPI`); runs `f` immediately — synchronous, not
/// a scheduler. Each [`blocking_call`] inside saves and restores the
/// entire live stack from its save point to the stack base, so nothing can
/// be under-saved: any call depth, no other placement requirement.
///
/// `Send + 'static` compiles away the lexical aliasing channel: no borrow
/// from outside the scope can be held across a suspension point inside it
/// (borrows created within the scope are inside the captured range and
/// heal with everything else). Denials are returned as [`EnterError`]
/// before any user code runs: [`EnterError::Nested`] inside an open
/// activation, [`EnterError::Exclusive`] while an
/// [`enter_promising_exclusive`] owner holds promising. A suspending call
/// in an activation that was not actually promising-entered traps at the
/// engine (loud, never corruption).
///
/// # Safety
///
/// The caller asserts what the types cannot see:
///
/// 1. This activation really was entered through `WebAssembly.promising`,
///    and this scope spans it.
/// 2. The multi-activation world: all activations share one `ThreadId` and
///    one set of `thread_local!` instances, and parked activations
///    interleave under that shared identity. The dependency tree must hold
///    no thread-identity-keyed reentrancy assumptions (`RefCell` in shared
///    state fails loud; identity-keyed locks do not). Hold nothing across
///    a bracket you would not hold across `epoll_wait`.
pub unsafe fn enter_promising<R>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, EnterError> {
    enter(f, false)
}

/// [`enter_promising`] claiming exclusive ownership of promising for the
/// activation's whole extent: the exclusive bit stays raised across the
/// owner's suspensions (unlike the parity counter, which parks with it),
/// so every other enter — including from host callbacks while the owner
/// is parked — is denied with [`EnterError::Exclusive`] until the owner
/// completes. Granted only from quiescence: an open activation denies
/// with [`EnterError::Nested`], parked siblings with
/// [`EnterError::Parked`] (already-parked activations would still
/// interleave, so exclusivity could not be honored).
///
/// # Safety
///
/// Identical contract to [`enter_promising`].
pub unsafe fn enter_promising_exclusive<R>(
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, EnterError> {
    enter(f, true)
}

fn enter<R>(f: impl FnOnce() -> R + Send + 'static, exclusive: bool) -> Result<R, EnterError> {
    anchor();
    if EXCLUSIVE.get() {
        return Err(EnterError::Exclusive);
    }
    let c = COUNTER.get();
    if c & 1 == 1 {
        return Err(EnterError::Nested);
    }
    if exclusive && c != 0 {
        return Err(EnterError::Parked);
    }
    COUNTER.set(c + 1);
    EXCLUSIVE.set(exclusive);
    // rebalance on both exit paths so an unwinding activation cannot
    // poison later ones
    struct Exit {
        exclusive: bool,
    }
    impl Drop for Exit {
        fn drop(&mut self) {
            COUNTER.set(COUNTER.get() - 1);
            if self.exclusive {
                EXCLUSIVE.set(false);
            }
        }
    }
    let _exit = Exit { exclusive };
    let r = f();
    assert!(COUNTER.get() & 1 == 1, "jspi: activation parity corrupted");
    Ok(r)
}

/// Bracket one foreign blocking call: save the live slice, call `f(args)`
/// (the consumer's `__asyncjs__` Suspending import, parking until its
/// promise settles), restore slice and stack pointer at the wasm resume
/// boundary. Misplacement — outside an activation scope, from a plain host
/// callback, reentrant — is denied by parity as a catchable panic before
/// anything runs.
///
/// # Safety
///
/// The caller vouches for `f`: a genuine suspending import (or any
/// non-suspending call, for which the bracket degrades to an
/// identical-bytes no-op), unit return (type-enforced), never throwing
/// into the resume site, and panicking only before its suspension point
/// (those unwind cleanly through the bracket; after it is a contract
/// violation, denied best-effort by abort).
pub unsafe fn blocking_call<A: BlockingArgs>(f: A::Fn, args: A) {
    let c = COUNTER.get();
    assert!(
        c & 1 == 1,
        "jspi: blocking_call requires an open activation scope with no \
         bracket in flight"
    );
    let (sp, top) = unsafe { (emscripten_stack_get_current(), emscripten_stack_get_base()) };
    let len = top - sp;
    let mut buf = Box::new_uninit_slice(len);
    // save is atomic with the suspension: nothing interleaves before f
    unsafe { ptr::copy_nonoverlapping(sp as *const MaybeUninit<u8>, buf.as_mut_ptr(), len) };
    // key = fp + const: recomputable post-resume from the engine-preserved
    // frame-pointer wasm local, no memory or SP-global read
    let probe: u8 = 0;
    let key = &raw const probe as usize;
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        assert!(
            !r.iter().any(|(k, _)| *k == key),
            "jspi: bracket key collision with a parked activation"
        );
        r.push((key, Meta { sp, len, buf }));
    });
    COUNTER.set(c + 1);

    // Closes the bracket if a pre-suspension panic unwinds out of f
    // (C-unwind). No restore: live memory is still the truth, and an SP
    // write mid-unwind would land callee frames on the unwinder's. A
    // post-resume panic is a contract violation: the key field is
    // untrusted memory, validated against the registry — abort on miss.
    struct UnwindClose {
        key: usize,
    }
    impl Drop for UnwindClose {
        fn drop(&mut self) {
            let Some(meta) = registry_remove(self.key) else {
                eprintln!("jspi: bracket unwound with unrecognized save-state");
                std::process::abort();
            };
            drop(meta);
            COUNTER.set(COUNTER.get() - 1);
        }
    }
    let unwind_close = UnwindClose { key };

    A::call(f, args); // parks; on resume SP and all of [sp, top) are untrusted

    std::mem::forget(unwind_close);
    let key = &raw const probe as usize;
    let Some(meta) = registry_remove(key) else {
        // unwinding over a still-scribbled frame is not an option
        eprintln!("jspi: bracket save-state missing at resume boundary");
        std::process::abort();
    };
    let (buf_ptr, sp, len) = (meta.buf.as_ptr() as usize, meta.sp, meta.len);
    IN_FLIGHT.set(Some(meta));
    // The restore rewrites this frame's own slots and sets SP := sp (this
    // frame's base), so it must run here, not in a callee: a callee frame
    // would sit below sp and be popped mid-execution by the SP write.
    #[cfg(not(jspi_disable_virtualization))]
    unsafe {
        let set_sp: unsafe extern "C" fn(usize) = _emscripten_stack_restore;
        jspi_restore(buf_ptr, sp, len, set_sp as usize);
    }
    #[cfg(jspi_disable_virtualization)]
    let _ = (buf_ptr, sp, len);
    // post-restore: static and heap reads only, frame slots fresh-only
    drop(IN_FLIGHT.take());
    COUNTER.set(COUNTER.get() - 1);
}

fn registry_remove(key: usize) -> Option<Meta> {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.iter()
            .position(|(k, _)| *k == key)
            .map(|i| r.swap_remove(i).1)
    })
}
