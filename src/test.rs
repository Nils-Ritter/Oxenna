pub use oxenna_test_macro::test;

use crate::{
    serial_print,
    serial_println,
};

pub enum TestResult {
    Pass,
    Fail(&'static str),
}

impl TestResult {
    pub const fn passed(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

// ============================================================
// Test registration
// ============================================================

#[allow(unused)]
pub type TestFn = fn() -> TestResult;

#[allow(unused)]
#[repr(C)]
pub struct Test {
    pub name: &'static str,
    pub function: TestFn,
}

// ============================================================
// Linker-provided test section
// ============================================================
//
// Every #[test] function creates a Test entry in the
// .kernel_tests section.
//
// The linker script gives us these two symbols:
//
//     __kernel_tests_start
//     __kernel_tests_end
//
// We can then iterate over all registered tests.
//

#[allow(warnings)]
unsafe extern "C" {
    static __kernel_tests_start: Test;
    static __kernel_tests_end: Test;
}

// ============================================================
// Run all tests
// ============================================================

#[allow(unused)]
pub fn run() -> ! {
    serial_println!();
    serial_println!("========================================");
    serial_println!("       OXENNA KERNEL TEST SUITE");
    serial_println!("========================================");
    serial_println!();

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;

    let mut current: *const Test =
        &raw const __kernel_tests_start;

    let end: *const Test =
        &raw const __kernel_tests_end;

    while current < end {
        let test = unsafe {
            &*current
        };

        total += 1;

        serial_print!(
            "test {} ... ",
            test.name
        );

        let result =
            (test.function)();

        match result {
            TestResult::Pass => {
                passed += 1;

                serial_println!("PASS");
            }

            TestResult::Fail(reason) => {
                failed += 1;

                serial_println!("FAIL");
                serial_println!(
                    "    {}",
                    reason
                );
            }
        }

        current = unsafe {
            current.add(1)
        };
    }

    serial_println!();
    serial_println!("========================================");

    serial_println!(
        "Tests: {} total, {} passed, {} failed",
        total,
        passed,
        failed
    );

    serial_println!(
        "========================================"
    );

    if failed == 0 {
        serial_println!(
            "ALL TESTS PASSED"
        );

        exit_qemu(true);
    } else {
        serial_println!(
            "TESTS FAILED"
        );

        exit_qemu(false);
    }

    loop {
        core::hint::spin_loop();
    }
}

// ============================================================
// QEMU exit
// ============================================================

pub fn exit_qemu(
    success: bool,
) -> ! {
    //
    // QEMU's isa-debug-exit device listens on
    // port 0xf4.
    //

    let code: u32 =
        if success {
            0x10
        } else {
            0x11
        };

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4u16,
            in("eax") code,
            options(
                nomem,
                nostack,
                preserves_flags
            ),
        );
    }

    loop {
        core::hint::spin_loop();
    }
}
