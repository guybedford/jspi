//! Unwinding soundness: rejections surface only after the stack is healed,
//! panics unwind through suspension frames over restored slices, drops that
//! suspend mid-unwind still observe intact frames, and fiber panics
//! propagate through join without crossing the promising boundary.

use std::cell::Cell;
use std::hint::black_box;
use std::panic::{catch_unwind, AssertUnwindSafe};

use jspi::{suspend_on, Deferred};
use jspi_test_glue::{sleep, TestPromise};

thread_local! {
    static DONE: Cell<u32> = const { Cell::new(0) };
}

fn done() {
    DONE.with(|d| d.set(d.get() + 1));
}

fn done_count() -> u32 {
    DONE.with(|d| d.get())
}

// Rejected promise: Err returned post-restore, frames intact, activation
// continues and can suspend again.
fn reject() {
    let before = done_count();
    let p = TestPromise::new();
    let _ = jspi::spawn(move || {
        let buf = [0x3Cu8; 128];
        black_box(&buf);
        assert!(
            p.wait().is_err(),
            "reject: expected Err from rejected promise"
        );
        assert_eq!(
            buf, [0x3Cu8; 128],
            "reject: buffer corrupted across rejection"
        );
        sleep(5.0); // suspension still works after a rejection
        done();
    });
    sleep(10.0);
    p.reject();
    sleep(20.0);
    assert_eq!(done_count(), before + 1, "reject: fiber did not complete");
}

struct DropCheck {
    buf: [u8; 96],
    hit: &'static str,
}

impl Drop for DropCheck {
    fn drop(&mut self) {
        assert_eq!(
            self.buf, [0xD7u8; 96],
            "{}: drop observed corrupted frame during unwind",
            self.hit
        );
        done();
    }
}

// Panic after resume unwinds through suspension frames: drops run against
// the restored slices while a sibling remains suspended and is unharmed.
fn panic_through_suspension() {
    let before = done_count();
    let d_victim = Deferred::new(None);
    let d_panic = Deferred::new(None);
    let (cv, cp) = (d_victim.clone(), d_panic.clone());
    let _ = jspi::spawn(move || {
        let buf = [0x99u8; 160];
        black_box(&buf);
        suspend_on(&cv).unwrap();
        assert_eq!(buf, [0x99u8; 160], "panic: victim buffer corrupted");
        done();
    });
    let _ = jspi::spawn(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _dc = DropCheck {
                buf: [0xD7u8; 96],
                hit: "panic_through_suspension",
            };
            black_box(&_dc);
            suspend_on(&cp).unwrap();
            panic!("intentional test panic");
        }));
        assert!(r.is_err(), "panic: expected caught unwind");
        sleep(5.0); // activation survives its own caught panic and suspends again
        done();
    });
    sleep(10.0); // both parked
    d_panic.resolve();
    sleep(10.0); // panicker resumed, unwound (running DropCheck), completed
    d_victim.resolve();
    sleep(10.0);
    assert_eq!(done_count(), before + 3, "panic: fibers did not complete");
}

struct SuspendOnDrop;

impl Drop for SuspendOnDrop {
    fn drop(&mut self) {
        sleep(5.0);
        done();
    }
}

// A drop that suspends while a panic is in flight: exploratory coverage for
// wasm-EH-in-flight + JSPI interaction.
fn suspend_during_unwind() {
    let before = done_count();
    let _ = jspi::spawn(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _sod = SuspendOnDrop;
            let buf = [0xD7u8; 96];
            black_box(&buf);
            panic!("intentional test panic");
        }));
        assert!(r.is_err(), "suspend-during-unwind: expected caught unwind");
        done();
    });
    sleep(40.0);
    assert_eq!(
        done_count(),
        before + 2,
        "suspend-during-unwind: drop suspension or fiber did not complete"
    );
}

// Unknown and already-consumed pids fail loudly (no suspension occurs, so
// the panic is safe pre-restore) and leave the system fully operational.
fn invalid_pid() {
    let r = catch_unwind(|| {
        let _ = jspi::suspend(0x0FFF_FFFF);
    });
    assert!(r.is_err(), "invalid-pid: expected panic on unknown pid");

    let p = TestPromise::new();
    let h = jspi::spawn(move || {
        p.resolve();
        p.wait().unwrap(); // consumes the registration
    });
    h.join().unwrap();
    let r = catch_unwind(|| {
        let _ = p.wait(); // already consumed
    });
    assert!(r.is_err(), "invalid-pid: expected panic on consumed pid");
    sleep(5.0); // suspension still works after both panics
}

// After an ActivationGuard drops, the registration is cleared: suspension
// fails loudly instead of silently reusing a dead activation's top.
fn guard_cleared() {
    let h = jspi::spawn(|| {
        let g = jspi::wasm_enter();
        drop(g);
        let r = catch_unwind(|| sleep(1.0));
        assert!(
            r.is_err(),
            "guard-cleared: expected suspend to panic after guard drop"
        );
    });
    h.join().unwrap();
}

// Fiber panics are caught at the trampoline (never crossing the promising
// boundary) and delivered through join.
fn join_propagates_panic() {
    let h = jspi::spawn(|| -> u32 {
        sleep(5.0); // panic on the resumed leg
        panic!("boom");
    });
    let err = h.join().unwrap_err();
    assert_eq!(
        err.downcast_ref::<&str>(),
        Some(&"boom"),
        "join: wrong panic payload"
    );
    // system fully operational afterward
    let h = jspi::spawn(|| 5u32);
    assert_eq!(h.join().unwrap(), 5, "join: fiber after panic failed");
}

fn main() {
    let _jspi_stack = jspi::wasm_enter();
    assert!(
        jspi::linked(),
        "test requires -sJSPI and a JSPI-enabled host"
    );
    reject();
    panic_through_suspension();
    suspend_during_unwind();
    invalid_pid();
    guard_cleared();
    join_propagates_panic();
    println!("panic tests passed");
}
