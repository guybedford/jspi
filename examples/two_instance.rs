//! Built with -sMODULARIZE by the chomp test:two-instance task and
//! instantiated twice in one node process by scripts/two_instance_driver.cjs:
//! per-instance registry state (Module.__jspiTest) must keep interleaved
//! suspensions across instances from corrupting each other (a shared
//! globalThis registry would cross-wire state; empirically verified crash).

use std::hint::black_box;

use jspi_test_glue::{run_fiber, sleep};

fn main() {
    unsafe {
        jspi::enter_promising(|| {
            assert!(jspi::jspi_enabled());
            run_fiber(|| {
                let buf = [0xAAu8; 300];
                black_box(&buf);
                sleep(20.0); // parked while the sibling instance suspends and resumes
                assert_eq!(buf, [0xAAu8; 300], "fiber buffer corrupted");
                println!("marker:instance-ok");
            });
            let buf = [0xBBu8; 200];
            black_box(&buf);
            sleep(10.0);
            assert_eq!(buf, [0xBBu8; 200], "main buffer corrupted");
            sleep(30.0); // fiber completes before main returns
        })
    }
}
