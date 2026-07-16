//! Port of Hood Chatham's reentrancy corruption reproduction: a victim's
//! suspended frames survive a sibling exiting above it (resetting the stack
//! pointer) and a plain activation scribbling down through its live range.
//! Must fail when built with --cfg jspi_disable_virtualization.

use std::cell::Cell;
use std::hint::black_box;

use jspi::{suspend_on, Deferred};
use jspi_test_glue::{call_plain_later, sleep};

thread_local! {
    static VICTIM_OK: Cell<bool> = const { Cell::new(false) };
}

fn scribble() {
    let mut junk = [0xEEu8; 16 * 1024];
    black_box(&mut junk);
}

fn main() {
    let _jspi_stack = jspi::wasm_enter();
    assert!(
        jspi::linked(),
        "test requires -sJSPI and a JSPI-enabled host"
    );

    let d_above = Deferred::new(None);
    let d_victim = Deferred::new(None);
    let (ca, cv) = (d_above.clone(), d_victim.clone());

    // occupies the region just below main's suspend point, then exits,
    // leaving the stack pointer above the victim
    let _ = jspi::spawn(move || {
        let pad = [0x44u8; 16];
        black_box(&pad);
        suspend_on(&ca).unwrap();
    });

    // parks below `above` with a recognizable buffer
    let _ = jspi::spawn(move || {
        let buf = *b"victim's important string / victim's important string!!";
        black_box(&buf);
        suspend_on(&cv).unwrap();
        assert_eq!(
            &buf[..],
            b"victim's important string / victim's important string!!",
            "victim stack corrupted"
        );
        VICTIM_OK.with(|v| v.set(true));
    });

    sleep(10.0); // both parked
    d_above.resolve();
    sleep(10.0); // `above` exited; stack pointer reset above victim
    call_plain_later(scribble);
    sleep(10.0); // scribbler ran down through the victim's live range
    d_victim.resolve();
    sleep(10.0);

    assert!(VICTIM_OK.with(|v| v.get()), "victim did not complete");
    println!("corruption test passed");
}
