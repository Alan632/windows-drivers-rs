// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Default Panic Handlers for programs built with the WDK (Windows Drivers Kit)

#![no_std]

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
unsafe extern "system" {
    fn KeBugCheckEx(
        BugCheckCode: u32,
        BugCheckParameter1: usize,
        BugCheckParameter2: usize,
        BugCheckParameter3: usize,
        BugCheckParameter4: usize,
    ) -> !;
}

#[cfg(not(test))]
const BUGCHECK_RUST_CODE: u32 = 0x5255_5354; // RUST

#[cfg(all(
    debug_assertions,
    // Disable inclusion of panic handlers when compiling tests for wdk crate
    not(test)
))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // SAFETY: `brk #0xF000` (aarch64) and `int 3` (x86/x86_64) are software
    // breakpoint instructions that trap into an attached debugger. If no debugger
    // is attached, the unhandled exception causes a bugcheck.
    // Implementations derived from details outlined in [MSVC `__debugbreak` intrinsic documentation](https://learn.microsoft.com/en-us/cpp/intrinsics/debugbreak?view=msvc-170#remarks)
    unsafe {
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("brk #0xF000");

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        core::arch::asm!("int 3");
    }
    rust_ke_bugcheck(_info)
}

#[cfg(all(
    not(debug_assertions),
    // Disable inclusion of panic handlers when compiling tests for wdk crate
    not(test)
))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    rust_ke_bugcheck(_info)
}

#[cfg(not(test))]
fn rust_ke_bugcheck(_info: &PanicInfo) -> ! {
    let (panic_filename_ptr, panic_filename_len, panic_line, panic_column) = _info
        .location()
        .map(|loc| {
            (
                loc.file().as_ptr() as usize,
                loc.file().len(),
                loc.line() as usize,
                loc.column() as usize,
            )
        })
        .unwrap_or((0, 0, 0, 0));

    // SAFETY: `KeBugCheckEx` is a Windows kernel API exported by `ntoskrnl.exe`
    // that is callable at any IRQL and never returns (it halts the system with a
    // bugcheck). The parameters are scalar `usize` values recorded in the crash
    // dump; `KeBugCheckEx` does not dereference `panic_filename_ptr` — it is stored
    // as an opaque value for post-mortem analysis. The FFI signature matches the
    // WDK declaration in `wdm.h`. This call is sound because:
    // 1. The function is always available in kernel mode (linked via ntoskrnl.lib).
    // 2. The calling convention is correct (`extern "system"` maps to the appropriate
    //    Windows calling convention for the target architecture).
    // 3. The `-> !` return type is upheld — `KeBugCheckEx` never returns.
    unsafe {
        KeBugCheckEx(
            BUGCHECK_RUST_CODE,
            panic_filename_ptr,
            panic_filename_len,
            panic_line,
            panic_column,
        )
    }
}
