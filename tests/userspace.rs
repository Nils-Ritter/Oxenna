use crate::test::{test, TestResult};

use x86_64::{
    VirtAddr,
    instructions::segmentation::{CS, Segment},
    registers::control::{Cr0, Cr3, Cr4},
};

// ============================================================
// Helpers
// ============================================================

fn pass() -> TestResult {
    TestResult::Pass
}

fn fail(reason: &'static str) -> TestResult {
    TestResult::Fail(reason)
}

// ============================================================
// Address helpers
// ============================================================
//
// x86-64 canonical addresses have all bits 63..48 equal to
// bit 47.
//
// This is the address format used by the 48-bit virtual address
// configuration we currently expect from QEMU/x86-64.
//
// We implement this ourselves because x86_64 0.15.5 does not
// expose VirtAddr::is_canonical().
// ============================================================

fn is_canonical(address: u64) -> bool {
    let bit_47 = (address >> 47) & 1;
    let upper = address >> 48;

    if bit_47 == 0 {
        upper == 0
    } else {
        upper == 0xffff
    }
}

// ============================================================
// CPU privilege state
// ============================================================

#[test]
fn kernel_is_running_in_ring0() -> TestResult {
    let cs = CS::get_reg();

    if cs.rpl() == x86_64::PrivilegeLevel::Ring0 {
        pass()
    } else {
        fail("kernel CS is not Ring 0")
    }
}

#[test]
fn kernel_cs_is_nonzero() -> TestResult {
    let cs = CS::get_reg();

    if cs.index() != 0 {
        pass()
    } else {
        fail("kernel CS selector is null")
    }
}

#[test]
fn kernel_cs_has_ring0_rpl() -> TestResult {
    let cs = CS::get_reg();

    if cs.rpl() == x86_64::PrivilegeLevel::Ring0 {
        pass()
    } else {
        fail("current CS does not have Ring 0 privilege")
    }
}

// ============================================================
// Control registers
// ============================================================

#[test]
fn cr3_is_nonzero() -> TestResult {
    let (frame, _) = Cr3::read();

    if frame.start_address().as_u64() != 0 {
        pass()
    } else {
        fail("CR3 contains a null page-table address")
    }
}

#[test]
fn cr3_is_page_aligned() -> TestResult {
    let (frame, _) = Cr3::read();

    let address = frame.start_address().as_u64();

    if address & 0xfff == 0 {
        pass()
    } else {
        fail("CR3 is not page aligned")
    }
}

// ============================================================
// Long-mode prerequisites
// ============================================================

#[test]
fn protected_mode_is_enabled() -> TestResult {
    let cr0 = Cr0::read();

    if cr0.contains(
        x86_64::registers::control::Cr0Flags::PROTECTED_MODE_ENABLE
    ) {
        pass()
    } else {
        fail("protected mode is disabled")
    }
}

#[test]
fn pae_is_enabled() -> TestResult {
    let cr4 = Cr4::read();

    if cr4.contains(
        x86_64::registers::control::Cr4Flags::PHYSICAL_ADDRESS_EXTENSION
    ) {
        pass()
    } else {
        fail("physical address extension (PAE) is disabled")
    }
}

// ============================================================
// Virtual address layout
// ============================================================

#[test]
fn kernel_base_is_canonical() -> TestResult {
    let address = 0xffffffff80000000u64;

    if is_canonical(address) {
        pass()
    } else {
        fail("kernel base address is not canonical")
    }
}

#[test]
fn kernel_base_is_page_aligned() -> TestResult {
    let address = 0xffffffff80000000u64;

    if address & 0xfff == 0 {
        pass()
    } else {
        fail("kernel base address is not page aligned")
    }
}

#[test]
fn expected_user_address_is_canonical() -> TestResult {
    let address = 0x0000000000400000u64;

    if is_canonical(address) {
        pass()
    } else {
        fail("expected user address is not canonical")
    }
}

#[test]
fn canonical_address_rejects_invalid_high_bits() -> TestResult {
    // Bit 47 is zero, so bits 63..48 must also be zero.

    let invalid = 0x0001000000000000u64;

    if !is_canonical(invalid) {
        pass()
    } else {
        fail("canonical-address validation accepted an invalid address")
    }
}

#[test]
fn canonical_address_accepts_high_half() -> TestResult {
    let address = 0xffff800000000000u64;

    if is_canonical(address) {
        pass()
    } else {
        fail("canonical-address validation rejected a valid high address")
    }
}

// ============================================================
// Linker / userspace regression
// ============================================================
//
// This doesn't enter ring 3. It simply ensures this test module
// itself made it into the kernel test registry.
//
// This is particularly useful when debugging:
//
//     KEEP(*(.kernel_tests))
//
// in linker.ld.
// ============================================================

#[test]
fn userspace_test_suite_is_loaded() -> TestResult {
    pass()
}
