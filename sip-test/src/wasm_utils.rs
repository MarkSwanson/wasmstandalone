

#[no_mangle]
pub extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}

// This is how you define wasm import fns.
extern "C" {
    pub fn call_test(x: i32) -> i32;
}


