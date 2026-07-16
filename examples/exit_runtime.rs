//! Built with -sEXIT_RUNTIME=1 by the chomp test:exit-runtime task: fibers
//! parked when main returns must still complete (runtime keepalive) before
//! the process exits 0.

use jspi_test_glue::{run_fiber, sleep};

fn main() {
    unsafe {
        jspi::stack_root(|| {
            assert!(jspi::linked());
            run_fiber(|| {
                sleep(30.0);
                println!("marker:fiber-done");
            });
            println!("marker:main-done");
        })
    }
}
