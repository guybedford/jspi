//! Built without -sJSPI by the chomp test:no-jspi task: linked() reports
//! false, suspension panics with the targeted message, and nothing else
//! breaks.

use std::panic::catch_unwind;

fn main() {
    let _jspi_stack = jspi::wasm_enter();
    assert!(!jspi::linked(), "expected linked() == false without -sJSPI");
    assert!(jspi::stack_top().is_some());
    let d = jspi::Deferred::new(None);
    let r = catch_unwind(|| {
        let _ = jspi::suspend_on(&d);
    });
    assert!(r.is_err(), "expected suspend_on to panic without -sJSPI");
    println!("marker:no-jspi-ok");
}
