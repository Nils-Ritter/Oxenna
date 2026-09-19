use core::ptr;

use x86_64::instructions::port::Port;

use crate::RSDP_REQUEST;

// ============================================================
// ACPI structures
// ============================================================

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RsdpV1 {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RsdpV2 {
    v1: RsdpV1,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[allow(unused)]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct GenericAddress {
    address_space: u8,
    bit_width: u8,
    bit_offset: u8,
    access_size: u8,
    address: u64,
}

// ============================================================
// ACPI state
// ============================================================

#[derive(Clone, Copy)]
struct ResetInfo {
    address_space: u8,
    address: u64,
    value: u8,
}

// We don't need a heap for this.
//
// `Option<ResetInfo>` is just a few bytes stored statically.
static mut RESET_INFO: Option<ResetInfo> = None;

// ============================================================
// Initialization
// ============================================================

pub fn init() {
    crate::serial_println!("ACPI: initializing...");

    let Some(response) = RSDP_REQUEST.response() else {
        crate::serial_println!(
            "ACPI: Limine did not provide an RSDP"
        );
        return;
    };

    let rsdp_address = response.address as usize;

    crate::serial_println!(
        "ACPI: RSDP at {:#x}",
        rsdp_address
    );

    unsafe {
        let rsdp_v1 =
            &*(rsdp_address as *const RsdpV1);

        // Check the RSDP signature.
        if rsdp_v1.signature != *b"RSD PTR " {
            crate::serial_println!(
                "ACPI: invalid RSDP signature"
            );
            return;
        }

        // Validate the first 20 bytes.
        if !checksum_valid(
            rsdp_address as *const u8,
            20,
        ) {
            crate::serial_println!(
                "ACPI: RSDP checksum invalid"
            );
            return;
        }

        let revision = rsdp_v1.revision;

        crate::serial_println!(
            "ACPI: revision {}",
            revision
        );

        if revision >= 2 {
            let rsdp =
                &*(rsdp_address as *const RsdpV2);

            let length =
                rsdp.length as usize;

            // Validate the complete extended RSDP.
            if !checksum_valid(
                rsdp_address as *const u8,
                length,
            ) {
                crate::serial_println!(
                    "ACPI: extended RSDP checksum invalid"
                );
                return;
            }

            let xsdt_address =
                rsdp.xsdt_address;

            crate::serial_println!(
                "ACPI: XSDT at {:#x}",
                xsdt_address
            );

            find_fadt(xsdt_address);
        } else {
            crate::serial_println!(
                "ACPI: ACPI 1.0 detected"
            );

            crate::serial_println!(
                "ACPI: RSDT support not implemented yet"
            );
        }
    }
}

// ============================================================
// Checksum
// ============================================================

unsafe fn checksum_valid(
    address: *const u8,
    length: usize,
) -> bool { unsafe {
    let mut sum: u8 = 0;

    for i in 0..length {
        sum = sum.wrapping_add(
            ptr::read(address.add(i))
        );
    }

    sum == 0
}}

// ============================================================
// Find FADT in XSDT
// ============================================================

unsafe fn find_fadt(xsdt_address: u64) { unsafe {
    let xsdt =
        xsdt_address as *const SdtHeader;

    let header =
        &*xsdt;

    // Validate XSDT signature.
    if header.signature != *b"XSDT" {
        crate::serial_println!(
            "ACPI: invalid XSDT signature"
        );
        return;
    }

    let length =
        header.length as usize;

    if length < core::mem::size_of::<SdtHeader>() {
        crate::serial_println!(
            "ACPI: invalid XSDT length"
        );
        return;
    }

    // XSDT entries are 64-bit addresses.
    let entry_count =
        (length - core::mem::size_of::<SdtHeader>())
            / 8;

    crate::serial_println!(
        "ACPI: XSDT contains {} entries",
        entry_count
    );

    let entries =
        (xsdt_address as *const u8)
            .add(core::mem::size_of::<SdtHeader>())
            as *const u64;

    for i in 0..entry_count {
        let table_address =
            ptr::read_unaligned(
                entries.add(i)
            );

        if table_address == 0 {
            continue;
        }

        let table_header =
            &*(table_address as *const SdtHeader);

        let signature =
            table_header.signature;

        crate::serial_println!(
            "ACPI: table {} = {}",
            i,
            signature_to_str(signature)
        );

        if signature == *b"FACP" {
            crate::serial_println!(
                "ACPI: found FADT at {:#x}",
                table_address
            );

            parse_fadt(table_address);

            return;
        }
    }

    crate::serial_println!(
        "ACPI: FADT not found"
    );
}}

// ============================================================
// Parse FADT
// ============================================================
//
// We deliberately don't define the entire FADT structure.
//
// Instead, we read the fields we need by their ACPI-defined
// offsets. This avoids accidentally taking references to
// packed/unaligned fields.
//
// FADT offsets:
//
//   0x00  SDT header
//   ...
//   0x70  reset register (Generic Address Structure)
//   0x7C  reset value
//
// The extended FADT fields are later in the structure.
//
// ============================================================

unsafe fn parse_fadt(
    address: u64,
) { unsafe {
    let base =
        address as *const u8;

    let header =
        &*(base as *const SdtHeader);

    let length =
        header.length as usize;

    crate::serial_println!(
        "ACPI: FADT length = {}",
        length
    );

    // --------------------------------------------------------
    // Validate checksum
    // --------------------------------------------------------

    if !checksum_valid(base, length) {
        crate::serial_println!(
            "ACPI: FADT checksum invalid"
        );
        return;
    }

    crate::serial_println!(
        "ACPI: FADT checksum valid"
    );

    // --------------------------------------------------------
    // RESET_REG
    // --------------------------------------------------------
    //
    // Generic Address Structure:
    //
    // +0  address space
    // +1  bit width
    // +2  bit offset
    // +3  access size
    // +4  address (u64)
    //
    // RESET_REG starts at FADT offset 0x70.
    // --------------------------------------------------------

    const RESET_REG_OFFSET: usize = 0x70;
    const RESET_VALUE_OFFSET: usize = 0x7C;

    // Make sure the FADT is long enough.
    if length < RESET_VALUE_OFFSET + 1 {
        crate::serial_println!(
            "ACPI: FADT is too short for RESET_REG"
        );
        return;
    }

    let reset_reg =
        base.add(RESET_REG_OFFSET);

    let address_space =
        ptr::read(reset_reg);

    let bit_width =
        ptr::read(reset_reg.add(1));

    let bit_offset =
        ptr::read(reset_reg.add(2));

    let access_size =
        ptr::read(reset_reg.add(3));

    let reset_address =
        ptr::read_unaligned(
            reset_reg.add(4)
                as *const u64
        );

    let reset_value =
        ptr::read(
            base.add(RESET_VALUE_OFFSET)
        );

    crate::serial_println!(
        "ACPI: RESET_REG"
    );

    crate::serial_println!(
        "  address space: {:#x}",
        address_space
    );

    crate::serial_println!(
        "  bit width: {}",
        bit_width
    );

    crate::serial_println!(
        "  bit offset: {}",
        bit_offset
    );

    crate::serial_println!(
        "  access size: {}",
        access_size
    );

    crate::serial_println!(
        "  address: {:#x}",
        reset_address
    );

    crate::serial_println!(
        "  value: {:#x}",
        reset_value
    );

    // --------------------------------------------------------
    // Save reset information
    // --------------------------------------------------------

    if reset_address == 0 {
        crate::serial_println!(
            "ACPI: RESET_REG is unavailable"
        );

        return;
    }

    RESET_INFO = Some(ResetInfo {
        address_space,
        address: reset_address,
        value: reset_value,
    });

    crate::serial_println!(
        "ACPI: reset mechanism available"
    );
}}

// ============================================================
// Reboot
// ============================================================

/// Reboot the machine using the ACPI reset register.
///
/// This function should never return.
#[allow(unused)]
pub fn reboot() -> ! {
    crate::serial_println!(
        "ACPI: rebooting..."
    );

    // Hardware interrupts are irrelevant during reset
    // and could interfere with the tiny reset sequence.
    x86_64::instructions::interrupts::disable();

    let reset = unsafe {
        RESET_INFO
    };

    let Some(reset) = reset else {
        crate::serial_println!(
            "ACPI: no reset mechanism available"
        );

        fallback_reboot();
    };

    // ACPI Generic Address Structure address spaces:
    //
    // 0 = System Memory
    // 1 = System I/O
    //
    // For the initial x86 implementation, we support
    // System I/O here.
    if reset.address_space == 1 {
        // System I/O ports are 16-bit on x86.
        if reset.address > u16::MAX as u64 {
            crate::serial_println!(
                "ACPI: RESET_REG I/O address is invalid"
            );

            fallback_reboot();
        }

        unsafe {
            let mut port =
                Port::<u8>::new(
                    reset.address as u16
                );

            port.write(reset.value);
        }

        // If the platform hasn't reset after the write,
        // the reset mechanism didn't work.
        crate::serial_println!(
            "ACPI: reset command returned"
        );
    } else {
        crate::serial_println!(
            "ACPI: RESET_REG address space {:#x} is not supported yet",
            reset.address_space
        );
    }

    fallback_reboot();
}

// ============================================================
// Fallback reboot
// ============================================================

#[allow(unused)]
fn fallback_reboot() -> ! {
    crate::serial_println!(
        "ACPI: attempting fallback reboot..."
    );

    // --------------------------------------------------------
    // 8042 keyboard controller reset
    // --------------------------------------------------------
    //
    // This is a fallback only. Modern systems don't have to
    // implement this mechanism.
    // --------------------------------------------------------

    unsafe {
        let mut status =
            Port::<u8>::new(0x64);

        let mut data =
            Port::<u8>::new(0x60);

        // Wait until the controller input buffer is empty.
        for _ in 0..100_000 {
            if status.read() & 0x02 == 0 {
                break;
            }

            core::hint::spin_loop();
        }

        // Tell the keyboard controller to pulse RESET.
        status.write(0xFE);

        // Keep the compiler from considering `data`
        // completely unused in the I/O sequence.
        let _ = &mut data;
    }

    // --------------------------------------------------------
    // Triple fault fallback
    // --------------------------------------------------------
    //
    // If the previous methods failed, deliberately load an
    // invalid IDT and trigger an exception.
    //
    // This causes the CPU to enter a shutdown/reset path on
    // typical x86/QEMU environments.
    // --------------------------------------------------------

    unsafe {
        core::arch::asm!(
            "cli",
            "lidt [{0}]",
            "int3",
            in(reg) &INVALID_IDT,
            options(noreturn)
        );
    }
}

#[allow(unused)]
#[repr(C, packed)]
struct InvalidIdt {
    limit: u16,
    base: u64,
}

#[allow(unused)]
static INVALID_IDT: InvalidIdt = InvalidIdt {
    limit: 0,
    base: 0,
};

// ============================================================
// Utility
// ============================================================

fn signature_to_str(signature: [u8; 4]) -> &'static str {
    match &signature {
        b"FACP" => "FACP",
        b"APIC" => "APIC",
        b"HPET" => "HPET",
        b"MCFG" => "MCFG",
        b"DSDT" => "DSDT",
        b"SSDT" => "SSDT",
        _ => "????",
    }
}
