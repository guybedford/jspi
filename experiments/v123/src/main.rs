use std::ffi::c_void;

extern "C" fn fiber_entry(data: *mut c_void, top: u32) {
    shim::log(1000 + data as i32);
    // JS-measured top vs Rust-side FFI SP: delta shows entry frame size
    shim::log(top as i32 - shim::stack_current() as i32);
    let r = shim::sleep(50); // suspends
    shim::log(2000 + r);
}

fn main() {
    shim::log(1);
    shim::spawn(fiber_entry, 41 as *mut c_void);
    shim::log(2);
}
