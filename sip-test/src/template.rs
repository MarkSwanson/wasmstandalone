
#[allow(unused_imports)]
use wasm_utils::{add, call_test};

static mut INIT: i32 = 0;
const X: i32 = 12;

#[no_mangle]
pub extern "C" fn start() {
    // init stuff...
    unsafe {INIT = 1};
}

// REMEMBER: if you change this file then run ./make
#[no_mangle]
pub extern "C" fn test(n: i32) -> i32 {
    #[allow(unused_mut)]
    let answer = add(n, n);
    let a = unsafe {call_test()};
    X - answer - a
    //X - answer
}


