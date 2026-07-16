use std::ffi::c_void;

macro_rules! em_js_data {
    ($sym:ident, $str:expr) => {
        #[no_mangle]
        #[used]
        pub static $sym: [u8; concat!($str, "\0").len()] = {
            let mut a = [0u8; concat!($str, "\0").len()];
            let b = concat!($str, "\0").as_bytes();
            let mut i = 0;
            while i < b.len() {
                a[i] = b[i];
                i += 1;
            }
            a
        };
    };
}

em_js_data!(
    __em_js__jspi_log,
    "(x)<::>{ console.log('shim log:', x); }"
);

em_js_data!(
    __em_js____asyncjs__jspi_sleep,
    "(ms)<::>{ return new Promise(r => setTimeout(() => r(ms + 1), ms)); }"
);

em_js_data!(
    __em_js__jspi_spawn,
    "(fnptr, data, spGetter)<::>{ const table = wasmExports.__indirect_function_table; const entry = WebAssembly.promising(table.get(fnptr)); const getSp = table.get(spGetter); setTimeout(() => { entry(data, getSp()); }, 0); }"
);

#[link(wasm_import_module = "env")]
extern "C" {
    #[link_name = "jspi_log"]
    fn jspi_log_import(x: i32);
    #[link_name = "__asyncjs__jspi_sleep"]
    fn jspi_sleep_import(ms: i32) -> i32;
    #[link_name = "jspi_spawn"]
    fn jspi_spawn_import(
        entry: extern "C" fn(*mut c_void, u32),
        data: *mut c_void,
        sp_getter: extern "C" fn() -> u32,
    );
}

extern "C" {
    fn emscripten_stack_get_current() -> u32;
}

extern "C" fn sp_getter() -> u32 {
    unsafe { emscripten_stack_get_current() }
}

pub fn stack_current() -> u32 {
    unsafe { emscripten_stack_get_current() }
}

// wrappers reference the statics so the archive member holding them is always pulled in
#[inline(never)]
fn anchor() {
    std::hint::black_box((
        __em_js__jspi_log.as_ptr(),
        __em_js____asyncjs__jspi_sleep.as_ptr(),
        __em_js__jspi_spawn.as_ptr(),
    ));
}

pub fn log(x: i32) {
    anchor();
    unsafe { jspi_log_import(x) }
}

pub fn sleep(ms: i32) -> i32 {
    anchor();
    unsafe { jspi_sleep_import(ms) }
}

pub fn spawn(entry: extern "C" fn(*mut c_void, u32), data: *mut c_void) {
    anchor();
    unsafe { jspi_spawn_import(entry, data, sp_getter) }
}
