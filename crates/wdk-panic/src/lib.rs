// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Default Panic Handler for programs built with the WDK (Windows Driver Kit)
//!
//! **WDM and KMDF** drivers trigger a bugcheck (`0x52555354` / `RUST`) via
//! `KeBugCheckEx`. The panic source location is recorded in the bugcheck
//! parameters for post-mortem analysis.
//!
//! **UMDF** drivers print panic information to the system debugger.
//! Call `install_panic_hook` early in `DriverEntry`.

#![cfg_attr(
    any(driver_model__driver_type = "WDM", driver_model__driver_type = "KMDF"),
    no_std
)]

#[cfg(all(
    not(test),
    any(driver_model__driver_type = "WDM", driver_model__driver_type = "KMDF")
))]
mod kernel_panic_handler {
    use core::panic::PanicInfo;

    #[cfg(debug_assertions)]
    use wdk::dbg_break;

    #[allow(non_snake_case)]
    unsafe extern "system" {
        fn KeBugCheckEx(
            BugCheckCode: u32,
            BugCheckParameter1: usize,
            BugCheckParameter2: usize,
            BugCheckParameter3: usize,
            BugCheckParameter4: usize,
        ) -> !;
    }

    const BUGCHECK_RUST_CODE: u32 = 0x5255_5354; // RUST

    #[cold]
    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        #[cfg(debug_assertions)]
        dbg_break();
        rust_ke_bugcheck(info)
    }

    #[cold]
    fn rust_ke_bugcheck(info: &PanicInfo) -> ! {
        let (panic_filename_ptr, panic_filename_len, panic_line, panic_column) =
            info.location().map_or((0, 0, 0, 0), |loc| {
                (
                    loc.file().as_ptr() as usize,
                    loc.file().len(),
                    loc.line() as usize,
                    loc.column() as usize,
                )
            });

        // SAFETY: `KeBugCheckEx` is a Windows kernel API exported by `ntoskrnl.exe`
        // that is callable at any IRQL and never returns (it halts the system with a
        // bugcheck). The parameters are scalar `usize` values recorded in the crash
        // dump; `KeBugCheckEx` does not dereference `panic_filename_ptr` — it is stored
        // as an opaque value for post-mortem analysis. The FFI signature matches the
        // WDK declaration in `wdm.h`. This call is sound because:
        // 1. The function is always available in kernel mode (linked via ntoskrnl.lib).
        // 2. The calling convention is correct (`extern "system"` maps to the
        //    appropriate Windows calling convention for the target architecture).
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
}

/// Registers a panic hook for UMDF drivers that prints panic information via
/// [`wdk::println!`] and then aborts the host process.
///
/// Output is routed through [`OutputDebugStringA`], which is received by any
/// user-mode debugger attached to `WUDFHost.exe`, or by the kernel debugger
/// when no user-mode debugger is attached.
///
/// After printing, the process is aborted via [`std::process::abort`]. This
/// avoids unwinding through WDF/FFI callback frames, which would otherwise
/// trigger a secondary `panic in a function that cannot unwind` abort with
/// a misleading diagnostic.
///
/// Note that aborting `WUDFHost.exe` causes the UMDF reflector to restart the
/// host per the driver's host-failure policy, so `DriverEntry` may run again
/// until the PnP retry limit is reached.
///
/// Calling this replaces the default `std` panic hook.
///
/// # Usage
///
/// Call this early on in `DriverEntry`:
///
/// ```ignore
/// wdk_panic::install_panic_hook();
/// ```
#[cfg(all(not(test), driver_model__driver_type = "UMDF"))]
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        wdk::println!("[PANIC] {info}");
        std::process::abort();
    }));
}
