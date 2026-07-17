//! Denial and unwinding soundness: the parity counter denies misplaced
//! calls as catchable panics, panics unwind over healed frames, drops that
//! park mid-unwind work, and settled values/rejections are fetched by plain
//! imports post-restore.

use std::cell::Cell;
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};

use jspi_test_glue::{TestPromise, call_plain_later, glue_noop, glue_sleep, run_fiber, sleep};

thread_local! {
    static DONE: Cell<u32> = const { Cell::new(0) };
}

fn done() {
    DONE.set(DONE.get() + 1);
}

// Parity denial: a scope nested inside a scope fails as a catchable panic;
// the system stays fully operational afterwards.
fn parity_denial() {
    let r = catch_unwind(|| unsafe { jspi::enter_promising(|| {}) });
    assert!(
        r.is_err(),
        "parity: expected nested activation scope to panic"
    );
    // a bracket inside the scope (even under catch_unwind) is legal
    let r = catch_unwind(|| unsafe { jspi::blocking_call(glue_sleep, (1.0,)) });
    assert!(r.is_ok(), "parity: bracket inside scope should work");
    sleep(1.0);
}

// A consumer shim reaching back into blocking_call with a bracket already
// in flight: the parity counter denies it pre-suspension, and the panic
// unwinds through the outer bracket (`C-unwind`), whose RAII close restores
// identical bytes and rebalances parity — catchable, frames intact, system
// fully operational.
extern "C-unwind" fn reentrant_shim() {
    unsafe { jspi::blocking_call(glue_noop, ()) };
}

fn reentrant_denial() {
    let buf = [0x24u8; 128];
    black_box(&buf);
    let r = catch_unwind(|| {
        unsafe { jspi::blocking_call(reentrant_shim as extern "C-unwind" fn(), ()) };
    });
    assert!(r.is_err(), "reentrant: expected nested bracket to panic");
    assert_eq!(buf, [0x24u8; 128], "reentrant: buffer corrupted by unwind");
    // bracket fully closed during the unwind: parking works again
    sleep(1.0);
    assert_eq!(
        buf, [0x24u8; 128],
        "reentrant: buffer corrupted after re-park"
    );
}

// Plain host callbacks may resolve promises but never block: a
// blocking_call attempt from one fails the parity assert as a catchable
// panic (the counter denies before the engine could trap).
fn plain_callback_discipline() {
    let before = DONE.get();
    let p = TestPromise::new();
    run_fiber(move || {
        let buf = [0x3Au8; 128];
        black_box(&buf);
        p.wait();
        assert_eq!(buf, [0x3Au8; 128], "plain-callback: fiber buffer corrupted");
        done();
    });
    call_plain_later(move || {
        let r = catch_unwind(|| unsafe { jspi::blocking_call(glue_sleep, (1.0,)) });
        assert!(
            r.is_err(),
            "plain-callback: expected blocking_call to panic from a plain activation"
        );
        p.resolve();
        done();
    });
    sleep(20.0);
    assert_eq!(
        DONE.get(),
        before + 2,
        "plain-callback: fiber or callback did not complete"
    );
}

struct DropCheck {
    buf: [u8; 96],
}

impl Drop for DropCheck {
    fn drop(&mut self) {
        assert_eq!(
            self.buf, [0xD7u8; 96],
            "panic-unwind: drop observed corrupted frame during unwind"
        );
        done();
    }
}

// Panic after a bracket returns unwinds through park frames: drops run
// against restored slices while a sibling remains parked and is unharmed.
fn panic_after_bracket() {
    let before = DONE.get();
    let victim = TestPromise::new();
    let panicker = TestPromise::new();
    run_fiber(move || {
        let buf = [0x99u8; 160];
        black_box(&buf);
        victim.wait();
        assert_eq!(buf, [0x99u8; 160], "panic-unwind: victim buffer corrupted");
        done();
    });
    run_fiber(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let dc = DropCheck { buf: [0xD7u8; 96] };
            black_box(&dc);
            panicker.wait();
            panic!("intentional test panic");
        }));
        assert!(r.is_err(), "panic-unwind: expected caught unwind");
        sleep(5.0); // activation survives its own caught panic and parks again
        done();
    });
    sleep(10.0); // both parked
    panicker.resolve();
    sleep(10.0); // panicker resumed, unwound (running DropCheck), completed
    victim.resolve();
    sleep(10.0);
    assert_eq!(
        DONE.get(),
        before + 3,
        "panic-unwind: fibers did not complete"
    );
}

struct ParkOnDrop;

impl Drop for ParkOnDrop {
    fn drop(&mut self) {
        sleep(5.0);
        done();
    }
}

// A drop that parks while a panic is in flight: wasm-EH-in-flight + JSPI
// interaction.
fn park_during_unwind() {
    let before = DONE.get();
    run_fiber(|| {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let sod = ParkOnDrop;
            black_box(&sod);
            let buf = [0xD7u8; 96];
            black_box(&buf);
            panic!("intentional test panic");
        }));
        assert!(r.is_err(), "park-during-unwind: expected caught unwind");
        done();
    });
    sleep(40.0);
    assert_eq!(
        DONE.get(),
        before + 2,
        "park-during-unwind: drop park or fiber did not complete"
    );
}

// Values resolve through the foreign call and are fetched by plain imports
// post-restore; rejections are recorded, returned normally after the
// restore, and fetched as errors.
fn value_fetch() {
    let before = DONE.get();
    let p = TestPromise::new();
    let rejected = TestPromise::new();
    run_fiber(move || {
        let buf = [0x51u8; 128];
        black_box(&buf);
        p.wait();
        assert_eq!(p.take_result(), 42.0, "value-fetch: wrong resolved value");
        assert!(!p.take_error(), "value-fetch: unexpected error");
        rejected.wait();
        assert!(rejected.take_error(), "value-fetch: rejection not recorded");
        assert_eq!(buf, [0x51u8; 128], "value-fetch: buffer corrupted");
        sleep(5.0); // parking still works after a rejection
        done();
    });
    sleep(10.0);
    p.resolve_with(42.0);
    sleep(10.0);
    rejected.reject();
    sleep(20.0);
    assert_eq!(
        DONE.get(),
        before + 1,
        "value-fetch: fiber did not complete"
    );
}

fn main() {
    // outside any scope: parity even, denied as a catchable panic
    let r = catch_unwind(|| unsafe { jspi::blocking_call(glue_sleep, (1.0,)) });
    assert!(
        r.is_err(),
        "expected blocking_call outside a scope to panic"
    );
    unsafe {
        jspi::enter_promising(|| {
            assert!(
                jspi::jspi_enabled(),
                "test requires -sJSPI and a JSPI-enabled host"
            );
            // non-suspending call: the bracket degrades to an
            // identical-bytes no-op
            jspi::blocking_call(glue_noop, ());
            parity_denial();
            reentrant_denial();
            plain_callback_discipline();
            panic_after_bracket();
            park_during_unwind();
            value_fetch();
            println!("panic tests passed");
        })
    }
}
