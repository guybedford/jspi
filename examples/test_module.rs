//! Thin bin for the JS-driven test module: all exports and em_js glue live
//! in the `jspi-test-module` lib (rustc internalizes `#[no_mangle]` statics
//! when compiling a bin). Built with `-sMODULARIZE` and driven by
//! `tests/driver.cjs`.

fn main() {
    jspi_test_module::init();
}
