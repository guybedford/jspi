//! Interleaving soundness: non-LIFO wake orders, same-tick double wakes,
//! sequential brackets, and memory growth while parked.

use std::cell::Cell;
use std::hint::black_box;

use jspi_test_glue::{TestPromise, await_pid, run_fiber, sleep};

thread_local! {
    static DONE: Cell<u32> = const { Cell::new(0) };
}

fn done() {
    DONE.set(DONE.get() + 1);
}

fn basic() {
    let before = DONE.get();
    run_fiber(|| {
        let buf = [0x5Au8; 128];
        black_box(&buf);
        sleep(10.0);
        assert_eq!(buf, [0x5Au8; 128], "basic: buffer corrupted across sleep");
        done();
    });
    sleep(30.0);
    assert_eq!(DONE.get(), before + 1, "basic: fiber did not complete");
}

fn recurse(depth: u8, park_pid: u32) {
    let mut buf = [0u8; 64];
    buf.fill(depth ^ 0xC3);
    black_box(&mut buf);
    if depth == 0 {
        await_pid(park_pid);
    } else {
        recurse(depth - 1, park_pid);
    }
    assert_eq!(
        buf,
        [depth ^ 0xC3; 64],
        "non-lifo: frame buffer corrupted at depth {depth}"
    );
}

// X parks deep, Y (below X) parks deep after it, then wakes and completes
// first; Z then re-occupies and scribbles the same region; X wakes last and
// asserts every frame on unwind. Wake order is unrelated to park order.
fn non_lifo() {
    let before = DONE.get();
    let px = TestPromise::new();
    let py = TestPromise::new();
    let pz = TestPromise::new();
    run_fiber(move || {
        recurse(8, px.0);
        done();
    });
    run_fiber(move || {
        recurse(8, py.0);
        done();
    });
    sleep(20.0); // X and Y parked at depth 0
    py.resolve();
    sleep(10.0); // Y unwound and completed
    run_fiber(move || {
        recurse(12, pz.0);
        done();
    });
    sleep(10.0); // Z parked deep in Y's (and X's) old region
    pz.resolve();
    sleep(10.0); // Z unwound and completed
    px.resolve();
    sleep(10.0);
    assert_eq!(DONE.get(), before + 3, "non-lifo: fibers did not complete");
}

// Two parked brackets waking from the same resolution in the same microtask
// drain: exercises restore ordering across one engine resume tick (the
// design point that killed JS-side restores).
fn same_tick_double_wake() {
    let before = DONE.get();
    let p = TestPromise::new();
    let shared_pid = p.share();
    run_fiber(move || {
        let buf = [0xA1u8; 200];
        black_box(&buf);
        p.wait();
        assert_eq!(buf, [0xA1u8; 200], "same-tick: fiber 1 buffer corrupted");
        done();
    });
    run_fiber(move || {
        let buf = [0xB2u8; 200];
        black_box(&buf);
        await_pid(shared_pid);
        assert_eq!(buf, [0xB2u8; 200], "same-tick: fiber 2 buffer corrupted");
        done();
    });
    sleep(20.0); // let both park
    p.resolve();
    sleep(20.0);
    assert_eq!(DONE.get(), before + 2, "same-tick: fibers did not complete");
}

// 50 save/call/restore cycles in one activation with per-iteration buffers.
fn sequential_brackets() {
    for i in 0..50u8 {
        let mut buf = [0u8; 96];
        buf.fill(i ^ 0x7B);
        black_box(&mut buf);
        sleep(1.0);
        assert_eq!(
            buf,
            [i ^ 0x7B; 96],
            "sequential: buffer corrupted at iteration {i}"
        );
    }
}

// Memory growth while a sibling is parked: the restore must use fresh heap
// views.
fn growth() {
    let before = DONE.get();
    let p = TestPromise::new();
    run_fiber(move || {
        let buf = [0x7Eu8; 256];
        black_box(&buf);
        p.wait();
        assert_eq!(buf, [0x7Eu8; 256], "growth: buffer corrupted across growth");
        done();
    });
    sleep(10.0);
    let big = vec![0x11u8; 128 * 1024 * 1024];
    black_box(&big[big.len() - 1]);
    drop(big);
    p.resolve();
    sleep(10.0);
    assert_eq!(DONE.get(), before + 1, "growth: fiber did not complete");
}

fn main() {
    // the safe full-capture root as the outermost activation root
    jspi::spawn(|| {
        assert!(
            jspi::linked(),
            "test requires -sJSPI and a JSPI-enabled host"
        );
        basic();
        non_lifo();
        same_tick_double_wake();
        sequential_brackets();
        growth();
        println!("suspend tests passed");
    })
}
