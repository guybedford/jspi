//! Built without -sJSPI by the chomp test:no-jspi task: linked() reports
//! false, spawn itself works (it suspends nothing), and a non-suspending
//! blocking_call degrades to an identical-bytes no-op.

use std::hint::black_box;

use jspi_test_glue::glue_noop;

fn main() {
    jspi_test_glue::init();
    jspi::spawn(|| {
        assert!(!jspi::linked(), "expected linked() == false without -sJSPI");
        let buf = [0x42u8; 64];
        black_box(&buf);
        jspi::blocking_call(glue_noop, ());
        assert_eq!(
            buf, [0x42u8; 64],
            "no-jspi: buffer corrupted by no-op bracket"
        );
        println!("marker:no-jspi-ok");
    })
}
