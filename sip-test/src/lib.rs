#![feature(proc_macro)]
#![no_std]
#![feature(lang_items)]

pub mod template;
pub mod wasm_utils;

// panic_fmt() and eh_personality(): https://news.ycombinator.com/item?id=12208376

#[lang="panic_fmt"]
extern "C" fn panic_fmt(_: ::core::fmt::Arguments, _: &'static str, _: u32) -> ! {
    loop {}
}

#[lang = "eh_personality"] extern fn eh_personality() {}


