//! JIT-style runtime for WebAssembly using Cretonne.

#![deny(missing_docs)]

extern crate cretonne;
extern crate cton_wasm;
extern crate region;
extern crate wasmstandalone_runtime;

use cretonne::isa::TargetIsa;
//use std::collections::Vec;
use std::mem::transmute;
use region::Protection;
use region::protect;
use std::ptr::write_unaligned;
use wasmstandalone_runtime::Compilation;

/// Executes a module that has been translated with the `standalone::Runtime` runtime implementation.
pub fn compile_module<'data, 'module>(
    isa: &TargetIsa,
    translation: &wasmstandalone_runtime::ModuleTranslation<'data, 'module>,
    imported_functions: &[u64],
) -> Result<wasmstandalone_runtime::Compilation<'module>, String> {
    debug_assert!(
        translation.module.start_func.is_none() ||
            translation.module.start_func.unwrap() >= translation.module.imported_funcs.len(),
        "imported start functions not supported yet"
    );

    let (mut compilation, relocations) = translation.compile(isa)?;

    // Apply relocations, now that we have virtual addresses for everything.
    relocate(&mut compilation, &relocations, &imported_functions);

    Ok(compilation)
}

/// Performs the relocations inside the function bytecode, provided the necessary metadata
// Imported Functions:
// * target_func_address should be a pointer to the dlsym() result.
// * compile_module will receive 'imported functions', which will be available here.
// * the wasm function index space is defined to start with the imported functions, followed by the wasm-defined functions.
// * because the function_relocs points to 2 tables, we must be sure to use the r.func_index
//   property - because it spans imported functions and wasm functions.
#[allow(ptr_arg)]
fn relocate(compilation: &mut Compilation, relocations: &wasmstandalone_runtime::Relocations,
            imported_functions: &[u64]) {
    // The relocations are relative to the relocation's address plus four bytes
    // TODO: Support architectures other than x64, and other reloc kinds.
    for (i, function_relocs) in relocations.iter().enumerate() {
        for r in function_relocs {
            let (target_func_address, body) = {
                if r.func_index < imported_functions.len() {
                    println!("i: {}, imported_functions index: {}", i, r.func_index);
                    (imported_functions[r.func_index] as isize, &mut compilation.functions[i])
                }
                else {
                    println!("i: {}, compilation.functions index: {}", i, imported_functions.len() - r.func_index);
                    (compilation.functions[imported_functions.len() - r.func_index].as_ptr() as isize, &mut compilation.functions[i])
                }
            };
            unsafe {
                let reloc_address = body.as_mut_ptr().offset(r.offset as isize + 4) as isize;
                let reloc_addend = r.addend as isize;
                let reloc_delta_i32 = (target_func_address - reloc_address + reloc_addend) as i32;
                write_unaligned(reloc_address as *mut i32, reloc_delta_i32);
            }
        }
    }
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

/// Jumps to the code region of memory and execute the start function of the module.
pub fn execute(
    compilation: &wasmstandalone_runtime::Compilation,
    instance: &mut wasmstandalone_runtime::Instance,
) -> Result<(), String> {
    let start_index = compilation.module.start_func.ok_or_else(|| {
        String::from("No start function defined, aborting execution")
    })?;
    let code_buf = &compilation.functions[start_index];
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
