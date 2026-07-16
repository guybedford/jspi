//! Built with -sMODULARIZE by the chomp test:two-instance task and
//! instantiated twice in one node process by scripts/two_instance_driver.cjs:
//! per-instance registry state (Module.__jspi) must keep interleaved
//! suspensions across instances from corrupting each other (a shared
//! globalThis registry would cross-wire the setSp/getSp funcrefs).

use std::hint::black_box;

use jspi_test_glue::sleep;

fn main() {
    let _jspi_stack = jspi::wasm_enter();
    assert!(jspi::linked());
    let h = jspi::spawn(|| {
        let buf = [0xAAu8; 300];
        black_box(&buf);
        sleep(20.0); // parked while the sibling instance suspends and resumes
        assert_eq!(buf, [0xAAu8; 300], "fiber buffer corrupted");
        41u32
    });
    let buf = [0xBBu8; 200];
    black_box(&buf);
    sleep(10.0);
    assert_eq!(buf, [0xBBu8; 200], "main buffer corrupted");
    assert_eq!(h.join().unwrap(), 41);
    println!("marker:instance-ok");
}
