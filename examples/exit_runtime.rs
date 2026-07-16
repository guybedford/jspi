//! Built with -sEXIT_RUNTIME=1 by the chomp test:exit-runtime task: fibers
//! parked when main returns must still complete (runtime keepalive) before
//! the process exits 0.

use jspi_test_glue::sleep;

fn main() {
    let _jspi_stack = jspi::wasm_enter();
    assert!(jspi::linked());
    let _ = jspi::spawn(|| {
        sleep(30.0);
        println!("marker:fiber-done");
    });
    println!("marker:main-done");
}
