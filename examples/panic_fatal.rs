//! Run by the chomp test:panic-fatal task: a panic escaping a fiber crosses
//! the promising boundary as a rejection and must be fatal and loud — the
//! process exits non-zero after main completed (marker present).

use jspi_test_glue::{run_fiber, sleep};

fn main() {
    unsafe {
        jspi::enter_promising(|| {
            assert!(jspi::jspi_enabled());
            run_fiber(|| {
                sleep(5.0);
                panic!("intentional escaped fiber panic");
            });
            println!("marker:main-done");
        })
    }
}
