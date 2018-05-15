#![cfg_attr(feature="clippy", feature(plugin))]
#![cfg_attr(feature="clippy", plugin(clippy))]
#![feature(nll)]

extern crate clap;
extern crate cretonne_codegen;
extern crate cretonne_wasm;
extern crate cretonne_native;
extern crate os_bootinfo;
extern crate region;
//extern crate wasm;
#[macro_use] extern crate slog;
extern crate slog_term;
extern crate slog_async;
extern crate slog_atomic;
extern crate wasmstandalone_execute;
extern crate wasmstandalone_runtime;
extern crate libloading;

use clap::{Arg, App};
use std::io::prelude::*;
use std::fs::File;
use std::mem::transmute;

use cretonne_codegen::settings;
use cretonne_wasm::{translate_module};
use cretonne_codegen::settings::Configurable;
//use cretonne::settings::{self, Configurable};
use std::fs::{OpenOptions};
use slog::{Logger, Drain, FnValue};
use slog_atomic::*;
use region::{Protection, protect};
use wasmstandalone_execute::{compile_module};
use wasmstandalone_runtime::{Instance, Module, ModuleEnvironment};
use wasmstandalone_runtime::module::Export;

// *requirement* LD_LIBRARY_PATH MUST contain ${RUST_SYSROOT}/lib
// RUST_BACKTRACE=1 ../../../target/debug/sip --wasm-file ../../../tmp/sip_javascript.wasm --proxy-lib ../../../target/debug/libproxylib.so --function call_test
// RUST_BACKTRACE=1 ./target/debug/sip --wasm-file ./tmp/sip_javascript.wasm --proxy-lib ./target/debug/libproxylib.so --function call_test
// To get generated x86 code size:
// $ ./target/debug/cton-util wasm -s --isa x86 ./wasmtests/arith.wat
// Function #0 code size: 25 bytes
// Function #0 bytecode size: 24 bytes
// Total module code size: 25 bytes
// Total module bytecode size: 24 bytes
fn main() {

    let matches = App::new("SIP (Software Isolated Processes) WASM Tester")
        .version("0.1")
        .about("Runs wasm test code")
        .arg(Arg::with_name("config")
             .short("c")
             .long("config")
             .value_name("FILE")
             .help("Sets a custom config file")
             .takes_value(true))
        .arg(Arg::with_name("wasm-file")
             .help("Sets the input wasm filename to use")
             .long("wasm-file")
             .required(true)
             .takes_value(true))
        .arg(Arg::with_name("proxy-lib")
             .help("Sets the proxy library (DSO) filename to use")
             .long("proxy-lib")
             .required(true)
             .takes_value(true))
        .arg(Arg::with_name("function")
             .help("The name of the function to call.")
             .long("function")
             .required(false)
             .takes_value(true))
        .get_matches();

    let wasm_file = matches.value_of("wasm-file").unwrap();
    let proxy_lib_file = matches.value_of("proxy-lib").unwrap();
    let function = matches.value_of("function").unwrap();
    println!("Compiling wasm file: {} and dynamically linking against proxy lib: {}", &wasm_file, &proxy_lib_file);
    let lib = libloading::Library::new(proxy_lib_file).unwrap();
    unsafe {
        let func: libloading::Symbol<unsafe extern fn(i32) -> i32> = lib.get(b"call_test").unwrap();
        func(1); // 
        println!("Successfully dynamically loaded and called call_test().");
    }

    match compile_and_execute(&wasm_file, &lib, &function) {
        Ok(_) => {
            println!("Success. Called function: {}", function);
        },
        Err(err) => println!("Wasm file '{}' failed to compile with error: {:?}", &wasm_file, err),
    }
}

pub fn compile_and_execute(wasm_file: &str, dso: &libloading::Library, function_name: &str) -> Result<(), String> {
    let logger = create_terminal_logger();
    let mut file = File::open(wasm_file).unwrap_or_else(|e| panic!("Failed to open file: {} with error: {}", wasm_file, e));
    let mut wasm = Vec::new();
    file.read_to_end(&mut wasm).unwrap_or_else(|e| panic!("Failed to read file: {} with error: {}", wasm_file, e));

    let (mut flag_builder, isa_builder) = cretonne_native::builders()
        .expect("Host machine not supported.");


    // Enable verifier passes in debug mode.
    if cfg!(debug_assertions) {
        flag_builder.enable("enable_verifier").unwrap();
    }

    flag_builder.set("opt_level", "best").expect("failed to set opt_level to 'best'");
    let isa = isa_builder.finish(settings::Flags::new(flag_builder));

    let mut module = Module::new();
    let mut environ = ModuleEnvironment::new(isa.flags(), &mut module);

    translate_module(wasm.as_slice(), &mut environ)?;
    let translation = environ.finish_translation();

    // Format Vec<(String, String)>
    // The two strings are define as: module, FunctionName
    // Note: 'env' seems to be the required module name for imported functions.
    // TODO: Provide a link to somewhere in the wasm docs that explains 'env'.
    // example output: [
    //(
        //"env",
        //"call_test"
    //)
    //]
    debug!(logger, "imported_funcs: {:#?}", translation.module.imported_funcs);

    //let mut sip_allocator = SipAllocator::new(1 << 22, logger.clone()).unwrap();
    // only deals with wasm functions. not useful for imported functions.
    //let _compliation = translation.compile(&*isa)?;
    
    // : create imported_functions Vec
    // to create imported_functions we walk the imported_funcs table and push matching dlsym
    // addresses into the imported_functions Vec.
    //
    let mut imported_functions = Vec::new();
    for (_module, function_name) in &translation.module.imported_funcs {
        unsafe {
            match dso.get(function_name.as_bytes()) {
                Ok(sym) => imported_functions.push(*sym),
                Err(e) => return Err(format!("Failed to link imported function {} because of error: {}", function_name, e))
            };
            println!("Successfully linked imported function: {}", function_name);
        }
    }

    let _instance = match compile_module(&*isa, &translation, &imported_functions[..]) {
        Ok(compilation) => {
            let mut instance = Instance::new(compilation.module, &translation.lazy.data_initializers);
            execute(&compilation, &mut instance, function_name)?;
            instance
        }
        Err(s) => {
            return Err(s);
        }
    };

    //println!("compile() succeeded. About to emit()...");
    //compliation.emit()
    Ok(())
}

pub fn create_terminal_logger() -> Logger {

    let decorator = slog_term::PlainDecorator::new(std::io::stdout());
    let drain = slog_term::CompactFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = AtomicSwitch::new(drain);

    Logger::root(
        drain.fuse(),
        o!("version" => env!("CARGO_PKG_VERSION"))
    )
}

pub fn create_file_logger(log_path: &str) -> Result<Logger, String>  {
	let full_log_path = format!("tests/logs/{}", log_path);

    let file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .append(false)
        .open(full_log_path) {
            Ok(f) => f,
            Err(e) => panic!("Failed to open log file: {} because of error: {}", log_path, e),
        };

    let decorator = slog_term::PlainSyncDecorator::new(file);
    let drain = slog_term::FullFormat::new(decorator).build().fuse();

    Ok(Logger::root(drain, o!("src" => FnValue(move |info| {
        format!("{}:{}", info.file(), info.line())
    }))))
}


pub fn execute(
    compilation: &wasmstandalone_runtime::Compilation,
    instance: &mut wasmstandalone_runtime::Instance,
    fn_name: &str,
) -> Result<(), String> {
    //compilation.module.
    println!("compilation.module: {:#?}", &compilation.module);
    println!("compilation.functions: {:#?}", &compilation.functions);
    let fn_index: usize = match compilation.module.exports.get(fn_name) {
        None => return Err(format!("Wasm did not define function: {}", fn_name)),
        Some(export) => {
            match export {
                Export::Function(function_index) => {
                    *function_index
                },
                Export::Table(_table_index) => {
                    return Err(format!("Strangely, your function name: {} was actually a table.", fn_name));
                },
                Export::Memory(_memory_index) => {
                    return Err(format!("Strangely, your function name: {} was actually a memory.", fn_name));
                },
                Export::Global(_global_index) => {
                    return Err(format!("Strangely, your function name: {} was actually a global.", fn_name));
                },
            }
        },
    };
    //let start_index = compilation.module.start_func.ok_or_else(|| {
        //String::from("No start function defined, aborting execution")
    //})?;
    let code_buf = &compilation.functions[fn_index];
    match unsafe {
        protect(
            code_buf.as_ptr(),
            code_buf.len(),
            Protection::ReadWriteExecute,
        )
    } {
        Ok(()) => (),
        Err(err) => {
            return Err(format!(
                "failed to give executable permission to code: {}",
                err
            ))
        }
    }

    let vmctx = make_vmctx(instance);

    // Rather than writing inline assembly to jump to the code region, we use the fact that
    // the Rust ABI for calling a function with no arguments and no return matches the one of
    // the generated code.Thanks to this, we can transmute the code region into a first-class
    // Rust function and call it.
    unsafe {
        let start_func = transmute::<_, fn(*const *mut u8)>(code_buf.as_ptr());
        start_func(vmctx.as_ptr());
    }
    Ok(())
}

/// Create the VmCtx data structure for the JIT'd code to use. This must
/// match the VmCtx layout in the runtime.
fn make_vmctx(instance: &mut wasmstandalone_runtime::Instance) -> Vec<*mut u8> {
    let mut memories = Vec::new();
    let mut vmctx = Vec::new();
    vmctx.push(instance.globals.as_mut_ptr());
    for mem in &mut instance.memories {
        memories.push(mem.as_mut_ptr());
    }
    vmctx.push(memories.as_mut_ptr() as *mut u8);
    vmctx
}


