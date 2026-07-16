//! Port of Hood Chatham's reentrancy corruption reproduction: fiber A
//! parks briefly above; fiber V writes a recognizable buffer and parks long;
//! A resumes and completes (its epilogues walk SP back above V); a plain
//! callback then scribbles a large frame down through V's live range; V
//! resumes and asserts its buffer. Must fail when built with
//! `--cfg jspi_disable_virtualization` (the proof lane).

use std::cell::Cell;
use std::hint::black_box;

use jspi_test_glue::{TestPromise, await_pid, call_plain_later, run_fiber, sleep};

thread_local! {
    static VICTIM_OK: Cell<bool> = const { Cell::new(false) };
}

const VICTIM_DATA: &[u8; 55] = b"victim's important string / victim's important string!!";

fn scribble() {
    let mut junk = [0xEEu8; 16 * 1024];
    black_box(&mut junk);
}

fn main() {
    jspi::spawn(|| {
        assert!(
            jspi::linked(),
            "test requires -sJSPI and a JSPI-enabled host"
        );

        let above = TestPromise::new();
        let victim = TestPromise::new();

        // occupies the region just below main's park point, then exits,
        // leaving the stack pointer reset above the victim
        run_fiber(move || {
            let pad = [0x44u8; 16];
            black_box(&pad);
            await_pid(above.0);
        });

        // parks below `above` with a recognizable buffer
        run_fiber(move || {
            let buf = *VICTIM_DATA;
            black_box(&buf);
            await_pid(victim.0);
            assert_eq!(&buf[..], &VICTIM_DATA[..], "victim stack corrupted");
            VICTIM_OK.set(true);
        });

        sleep(10.0); // both parked
        above.resolve();
        sleep(10.0); // `above` exited; SP reset above the victim
        call_plain_later(scribble);
        sleep(10.0); // scribbler ran down through the victim's live range
        victim.resolve();
        sleep(10.0);

        assert!(VICTIM_OK.get(), "victim did not complete");
        println!("corruption test passed");
    })
}
