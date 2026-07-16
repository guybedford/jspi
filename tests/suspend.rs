use std::cell::Cell;
use std::hint::black_box;

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

fn basic() {
    let h = jspi::spawn(|| {
        let buf = [0x5Au8; 128];
        black_box(&buf);
        sleep(10.0);
        assert_eq!(buf, [0x5Au8; 128], "basic: buffer corrupted across sleep");
        7u32
    });
    assert_eq!(h.join().unwrap(), 7, "basic: join result");
}

// Two suspensions waking from the same resolution in the same microtask
// drain: exercises restore ordering when continuations queue back-to-back.
fn same_tick_double_wake() {
    let before = done_count();
    let p = TestPromise::new();
    let shared_pid = p.share();
    let _ = jspi::spawn(move || {
        let buf = [0xA1u8; 200];
        black_box(&buf);
        p.wait().unwrap();
        assert_eq!(buf, [0xA1u8; 200], "same-tick: fiber 1 buffer corrupted");
        done();
    });
    let _ = jspi::spawn(move || {
        let buf = [0xB2u8; 200];
        black_box(&buf);
        jspi::suspend(shared_pid).unwrap();
        assert_eq!(buf, [0xB2u8; 200], "same-tick: fiber 2 buffer corrupted");
        done();
    });
    sleep(20.0); // let both park
    p.resolve();
    sleep(20.0);
    assert_eq!(
        done_count(),
        before + 2,
        "same-tick: fibers did not complete"
    );
}

fn recurse(depth: u8, park: &Deferred) {
    let mut buf = [0u8; 64];
    buf.fill(depth ^ 0xC3);
    black_box(&mut buf);
    if depth == 0 {
        suspend_on(park).unwrap();
    } else {
        recurse(depth - 1, park);
    }
    assert_eq!(
        buf,
        [depth ^ 0xC3; 64],
        "non-lifo: frame buffer corrupted at depth {depth}"
    );
}

// X parks deep, Y (below X) parks deep after it, then wakes and completes
// first; Z then occupies and scribbles the same region; X wakes last and
// asserts every frame on unwind. Wake order is unrelated to suspend order.
fn non_lifo() {
    let before = done_count();
    let dx = Deferred::new(None);
    let dy = Deferred::new(None);
    let dz = Deferred::new(None);
    let (cx, cy, cz) = (dx.clone(), dy.clone(), dz.clone());
    let _ = jspi::spawn(move || {
        recurse(8, &cx);
        done();
    });
    let _ = jspi::spawn(move || {
        recurse(8, &cy);
        done();
    });
    sleep(20.0); // X and Y parked at depth 0
    dy.resolve();
    sleep(10.0); // Y unwound and completed
    let _ = jspi::spawn(move || {
        recurse(12, &cz);
        done();
    });
    sleep(10.0); // Z parked deep in Y's (and X's) old region
    dz.resolve();
    sleep(10.0); // Z unwound and completed
    dx.resolve();
    sleep(10.0);
    assert_eq!(
        done_count(),
        before + 3,
        "non-lifo: fibers did not complete"
    );
}

// Synchronous promising dispatch from a JS frame above live wasm: the entry
// glue must save/restore the shared top, or this activation would suspend
// with the callee's top and under-save its own frames.
fn sandwich() {
    let before = done_count();
    let top_before = jspi::stack_top().expect("sandwich: no top registered");
    let d = Deferred::new(None);
    let dc = d.clone();
    let buf = [0x42u8; 128];
    black_box(&buf);
    jspi_test_glue::sandwich_dispatch(move || {
        let inner = [0x66u8; 128];
        black_box(&inner);
        suspend_on(&dc).unwrap();
        assert_eq!(inner, [0x66u8; 128], "sandwich: callee buffer corrupted");
        done();
    });
    assert_eq!(
        jspi::stack_top(),
        Some(top_before),
        "sandwich: shared top clobbered by synchronous promising dispatch"
    );
    sleep(10.0); // suspend with our own (restored) top
    d.resolve();
    sleep(10.0);
    assert_eq!(buf, [0x42u8; 128], "sandwich: caller buffer corrupted");
    assert_eq!(
        done_count(),
        before + 1,
        "sandwich: callee did not complete"
    );
}

// Memory growth while suspended: restore must use fresh heap views.
fn growth() {
    let before = done_count();
    let d = Deferred::new(None);
    let dc = d.clone();
    let _ = jspi::spawn(move || {
        let buf = [0x7Eu8; 256];
        black_box(&buf);
        suspend_on(&dc).unwrap();
        assert_eq!(buf, [0x7Eu8; 256], "growth: buffer corrupted across growth");
        done();
    });
    sleep(10.0);
    let big = vec![0x11u8; 128 * 1024 * 1024];
    black_box(&big[big.len() - 1]);
    drop(big);
    d.resolve();
    sleep(10.0);
    assert_eq!(done_count(), before + 1, "growth: fiber did not complete");
}

// Timeout resolution wakes a parked suspender; early resolve disarms the
// timer (indistinguishable by design).
fn deferred_timeout() {
    let d = Deferred::new(Some(15.0));
    let h = jspi::spawn(move || {
        suspend_on(&d).unwrap(); // woken by the deadline
        3u32
    });
    assert_eq!(h.join().unwrap(), 3, "timeout: fiber did not wake");

    let d = Deferred::new(Some(60_000.0));
    let dc = d.clone();
    let h = jspi::spawn(move || {
        suspend_on(&dc).unwrap();
        4u32
    });
    sleep(10.0);
    d.resolve(); // must clearTimeout: a pinned 60s timer would hang the run
    assert_eq!(h.join().unwrap(), 4, "timeout: early resolve did not wake");
}

fn main() {
    let _jspi_stack = jspi::wasm_enter();
    assert!(
        jspi::linked(),
        "test requires -sJSPI and a JSPI-enabled host"
    );
    basic();
    same_tick_double_wake();
    non_lifo();
    sandwich();
    growth();
    deferred_timeout();
    #[cfg(feature = "metrics")]
    {
        let m = jspi::metrics();
        assert!(
            m.saves > 0 && m.saves == m.restores,
            "metrics: unbalanced save/restore: {m:?}"
        );
        assert!(
            m.saved_bytes > 0 && m.saved_bytes == m.restored_bytes,
            "metrics: unbalanced bytes: {m:?}"
        );
        println!(
            "metrics: {} suspensions, {} bytes copied each way (avg {} bytes/slice)",
            m.saves,
            m.saved_bytes,
            m.saved_bytes / m.saves
        );
        jspi::reset_metrics();
        assert_eq!(
            jspi::metrics(),
            jspi::Metrics::default(),
            "metrics: reset failed"
        );
    }
    println!("suspend tests passed");
}
