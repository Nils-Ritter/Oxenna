#![no_std]
#![no_main]
#![allow(non_upper_case_globals)]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod fb;
pub mod syscall;
mod fs;
mod acpi;
mod font;
mod serial;
mod test;
mod kmem;
pub mod int;
pub mod gdt;
mod user;
mod pic;
mod console;
mod shell;
mod drivers;

#[cfg(feature = "test")]
#[path = "../tests/trivial_assert.rs"]
mod trivial_assert_test;

#[cfg(feature = "test")]
#[path = "../tests/userspace.rs"]
mod userspace_test;

#[cfg(feature = "test")]
#[path = "../tests/interrupts.rs"]
mod interrupts_test;

use crate::{fb::Color, kmem::heap::init_heap};
pub use crate::test::{TestResult, test};

extern crate oxenna_test_macro;

#[cfg(feature = "test")]
mod unit_tests;

use core::panic::PanicInfo;

pub const DEBUG_TOGGLE: bool = true;
pub static mut TESTING: bool = false;

use limine::{RequestsEndMarker, RequestsStartMarker, request::RsdpRequest};

#[used]
#[unsafe(link_section = ".limine_req_start")]
static REQUESTS_START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".limine_req_end")]
static REQUESTS_END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[used]
#[unsafe(link_section = ".limine_reqs")]
static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn kmain() -> ! {
    serial_println!("Oxenna starting.");
    kinit();

    serial_println!("[TEST] kinit returned");

    #[cfg(feature = "test")]
    {
        unsafe { TESTING = true; }
        serial_println!("[TEST] entering test::run()");
        test::run();
    }

    #[cfg(not(feature = "test"))]
    {
        unsafe { TESTING = false; }
        kernel();
    }
}

static mut KINIT_CALLED: bool = false;

fn kinit(){
    unsafe {
        if KINIT_CALLED { panic!("KINIT CANNOT BE CALLED MORE THAN ONCE"); }
        KINIT_CALLED = true;
    }

    serial_print!("[STARTUP] initializing framebuf...");
    fb::init();
    fb::clear(fb::Color::BLACK);
    serial_println!("[OK]");

    serial_print!("[STARTUP] initializing console...");
    console::init();
    serial_println!("[OK]");

    console_print!("[STARTUP] Initializing GDT...");
    gdt::init();
    console_println_color!(Color::GREEN, "[OK]");

    console_print!("[STARTUP] Initializing IDT...");
    int::init();
    console_println_color!(Color::GREEN, "[OK]");

    console_print!("[STARTUP] Initializing PIC...");
    pic::init();
    console_println_color!(Color::GREEN, "[OK]");

    console_print!("[STARTUP] Enabling interrupts...");
    x86_64::instructions::interrupts::enable();
    console_println_color!(Color::GREEN, "[OK]");

    console_print!("[STARTUP] initializing acpi...");
    acpi::init();
    console_println_color!(Color::GREEN, "[OK]");

    console_print!("[STARTUP] initializing kmem...");
    let (mapper, frame_allocator) = unsafe { kmem::init() };

    *kmem::MAPPER.lock() = Some(mapper);
    *kmem::FRAME_ALLOCATOR.lock() = Some(frame_allocator);

    unsafe {
        init_heap(
            kmem::MAPPER.lock().as_mut().unwrap(),
            kmem::FRAME_ALLOCATOR.lock().as_mut().unwrap(),
        ).expect("failed to init heap");
    }
    console_println_color!(Color::GREEN, "[OK]");

    console_println!();
    console_println!("[INFO] Kernel is running.");
    console_println!("[INFO] Timer interrupts should now arrive.");
    console_println!("[INFO] Press keys to test keyboard IRQs.");
    console_println!();
}

#[allow(unused)]
fn kernel() -> !{
    console::clear();
    console_println!("Welcome to...");
    console_println_color!(Color::BLUE, "________                                      ");
    console_println_color!(Color::BLUE, "\\_____  \\ ___  ___ ____   ____   ____ _____   ");
    console_println_color!(Color::BLUE, " /   |   \\\\  \\/  // __ \\ /    \\ /    \\\\__  \\  ");
    console_println_color!(Color::BLUE, "/    |    \\>    <\\  ___/|   |  \\   |  \\/ __ \\_");
    console_println_color!(Color::BLUE, "\\_______  /__/\\_ \\\\___  >___|  /___|  (____  /");
    console_println_color!(Color::BLUE, "        \\/      \\/    \\/     \\/     \\/     \\/");
    console_print!("\nType any command to get started: ");
    fb::present();
    user::run()
}

use core::arch::asm;

use x86_64::{
    instructions::segmentation::{CS, SS, Segment},
    registers::{
        control::{Cr0, Cr2, Cr3, Cr4},
        model_specific::Efer,
        rflags::RFlags,
    },
};

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // ------------------------------------------------------------
    // Stop the world.
    //
    // If a timer/keyboard interrupt fires while we're printing the
    // panic information, it could re-enter code that is already in
    // an inconsistent state.
    // ------------------------------------------------------------
    x86_64::instructions::interrupts::disable();

    // ------------------------------------------------------------
    // Panic location
    // ------------------------------------------------------------

    let location = info
        .location()
        .map(|loc| {
            (
                loc.file(),
                loc.line(),
                loc.column(),
            )
        });

    // ------------------------------------------------------------
    // Control registers
    // ------------------------------------------------------------

    let (cr3_frame, _) = Cr3::read();

    let cr3 =
        cr3_frame.start_address().as_u64();

    let cr2 = Cr2::read()
        .map(|addr| addr.as_u64())
        .unwrap_or(0);

    let cr0 =
        Cr0::read().bits();

    let cr4 =
        Cr4::read().bits();

    // ------------------------------------------------------------
    // Extended Feature Enable Register
    //
    // Particularly useful now that we're using SYSCALL/SYSRET.
    // EFER.SCE should be set when syscalls are enabled.
    // ------------------------------------------------------------

    let efer =
        Efer::read().bits();

    // ------------------------------------------------------------
    // Segment registers
    //
    // CS is particularly useful because Oxenna now supports ring 3.
    //
    // RPL is encoded in the bottom two bits:
    //
    //     CS & 3 == 0 -> ring 0
    //     CS & 3 == 3 -> ring 3
    // ------------------------------------------------------------

    let cs =
        CS::get_reg().0;

    let ss =
        SS::get_reg().0;

    let privilege_level =
        cs & 0x3;

    // ------------------------------------------------------------
    // Current stack pointer
    // ------------------------------------------------------------

    let rsp: u64;

    unsafe {
        asm!(
            "mov {}, rsp",
            out(reg) rsp,
            options(nomem, nostack, preserves_flags)
        );
    }

    // ------------------------------------------------------------
    // Current RFLAGS
    // ------------------------------------------------------------

    let rflags: u64;

    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
    }


    // ------------------------------------------------------------
    // Decode some useful flags
    // ------------------------------------------------------------

    let interrupts_enabled =
        (rflags & (1 << 9)) != 0;

    let direction_flag =
        (rflags & (1 << 10)) != 0;

    let carry_flag =
        (rflags & (1 << 0)) != 0;

    let zero_flag =
        (rflags & (1 << 6)) != 0;

    let sign_flag =
        (rflags & (1 << 7)) != 0;

    let overflow_flag =
        (rflags & (1 << 11)) != 0;

    // ------------------------------------------------------------
    // ---- SERIAL OUTPUT -----------------------------------------
    //
    // Plain text, no ANSI colors, easy to grep from QEMU/serial
    // logs.
    // ------------------------------------------------------------

    serial_println!();
    serial_println!(
        "================================================================"
    );
    serial_println!(
        "                         KERNEL PANIC"
    );
    serial_println!(
        "================================================================"
    );

    // ------------------------------------------------------------
    // Panic information
    // ------------------------------------------------------------

    match location {
        Some((file, line, col)) => {
            serial_println!(
                "  Location : {}:{}:{}",
                file,
                line,
                col
            );
        }

        None => {
            serial_println!(
                "  Location : unknown"
            );
        }
    }

    serial_println!(
        "  Message  : {}",
        info.message()
    );

    // ------------------------------------------------------------
    // Execution context
    // ------------------------------------------------------------

    serial_println!();
    serial_println!(
        "-------------------- EXECUTION CONTEXT ------------------------"
    );

    serial_println!(
        "  CPL      : ring {}",
        privilege_level
    );

    serial_println!(
        "  CS       : {:#06x}",
        cs
    );

    serial_println!(
        "  SS       : {:#06x}",
        ss
    );

    serial_println!(
        "  RSP      : {:#018x}",
        rsp
    );

    serial_println!(
        "  RFLAGS   : {:#018x}",
        rflags
    );

    serial_println!(
        "  IF       : {}",
        if interrupts_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    serial_println!(
        "  CF       : {}",
        carry_flag
    );

    serial_println!(
        "  ZF       : {}",
        zero_flag
    );

    serial_println!(
        "  SF       : {}",
        sign_flag
    );

    serial_println!(
        "  OF       : {}",
        overflow_flag
    );

    serial_println!(
        "  DF       : {}",
        direction_flag
    );

    // ------------------------------------------------------------
    // Memory-management state
    // ------------------------------------------------------------

    serial_println!();
    serial_println!(
        "-------------------- MEMORY STATE -----------------------------"
    );

    serial_println!(
        "  CR0      : {:#018x}",
        cr0
    );

    serial_println!(
        "  CR2      : {:#018x}",
        cr2
    );

    serial_println!(
        "  CR3      : {:#018x}",
        cr3
    );

    serial_println!(
        "  CR4      : {:#018x}",
        cr4
    );

    serial_println!(
        "  EFER     : {:#018x}",
        efer
    );

    serial_println!(
        "  CR0.PG   : {}",
        (cr0 & (1 << 31)) != 0
    );

    serial_println!(
        "  CR0.WP   : {}",
        (cr0 & (1 << 16)) != 0
    );

    serial_println!(
        "  CR4.PAE  : {}",
        (cr4 & (1 << 5)) != 0
    );

    serial_println!(
        "  CR4.PGE  : {}",
        (cr4 & (1 << 7)) != 0
    );

    serial_println!(
        "  CR4.SMEP : {}",
        (cr4 & (1 << 20)) != 0
    );

    serial_println!(
        "  CR4.SMAP : {}",
        (cr4 & (1 << 21)) != 0
    );

    serial_println!(
        "  EFER.SCE : {}",
        (efer & (1 << 0)) != 0
    );

    serial_println!(
        "  EFER.NXE : {}",
        (efer & (1 << 11)) != 0
    );

    serial_println!(
        "  EFER.LME : {}",
        (efer & (1 << 8)) != 0
    );

    // ------------------------------------------------------------
    // CR2 deserves a specific warning.
    //
    // CR2 is only architecturally meaningful as the faulting
    // linear address after a page fault. It can contain an old
    // value after unrelated exceptions/panics.
    // ------------------------------------------------------------

    serial_println!();
    serial_println!(
        "-------------------- PAGE FAULT STATE ------------------------"
    );

    serial_println!(
        "  CR2      : {:#018x}",
        cr2
    );

    serial_println!(
        "  NOTE     : CR2 is only meaningful for a page-fault context"
    );

    serial_println!();

    // ------------------------------------------------------------
    // Stack information
    // ------------------------------------------------------------

    serial_println!(
        "-------------------- STACK STATE ------------------------------"
    );

    serial_println!(
        "  RSP      : {:#018x}",
        rsp
    );

    serial_println!(
        "  RSP % 16 : {}",
        rsp & 0xf
    );

    serial_println!(
        "  RSP % 8  : {}",
        rsp & 0x7
    );

    serial_println!(
        "================================================================"
    );

    serial_println!(
        "                         SYSTEM HALTED"
    );

    serial_println!(
        "================================================================"
    );

    serial_println!();

    // ------------------------------------------------------------
    // ---- FRAMEBUFFER OUTPUT ------------------------------------
    // ------------------------------------------------------------

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "############################################################"
    );

    console_println_color!(
        Color::RED,
        "#                    KERNEL PANIC                          #"
    );

    console_println_color!(
        Color::RED,
        "############################################################"
    );

    console_println_color!(
        Color::RED,
        ""
    );

    // ------------------------------------------------------------
    // Panic information
    // ------------------------------------------------------------

    match location {
        Some((file, line, col)) => {
            console_println_color!(
                Color::RED,
                "  Location : {}:{}:{}",
                file,
                line,
                col
            );
        }

        None => {
            console_println_color!(
                Color::RED,
                "  Location : unknown"
            );
        }
    }

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "  {}",
        info.message()
    );

    // ------------------------------------------------------------
    // Execution context
    // ------------------------------------------------------------

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "  CPU CONTEXT"
    );

    console_println_color!(
        Color::RED,
        "    CPL      : ring {}",
        privilege_level
    );

    console_println_color!(
        Color::RED,
        "    CS       : {:#06x}",
        cs
    );

    console_println_color!(
        Color::RED,
        "    SS       : {:#06x}",
        ss
    );

    console_println_color!(
        Color::RED,
        "    RSP      : {:#018x}",
        rsp
    );

    console_println_color!(
        Color::RED,
        "    RFLAGS   : {:#018x}",
        rflags
    );

    console_println_color!(
        Color::RED,
        "    IF       : {}",
        if interrupts_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    // ------------------------------------------------------------
    // Memory state
    // ------------------------------------------------------------

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "  MEMORY STATE"
    );

    console_println_color!(
        Color::RED,
        "    CR0      : {:#018x}",
        cr0
    );

    console_println_color!(
        Color::RED,
        "    CR2      : {:#018x}",
        cr2
    );

    console_println_color!(
        Color::RED,
        "    CR3      : {:#018x}",
        cr3
    );

    console_println_color!(
        Color::RED,
        "    CR4      : {:#018x}",
        cr4
    );

    console_println_color!(
        Color::RED,
        "    EFER     : {:#018x}",
        efer
    );

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "  FEATURES"
    );

    console_println_color!(
        Color::RED,
        "    Paging   : {}",
        if (cr0 & (1 << 31)) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    console_println_color!(
        Color::RED,
        "    PAE      : {}",
        if (cr4 & (1 << 5)) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    console_println_color!(
        Color::RED,
        "    SMEP     : {}",
        if (cr4 & (1 << 20)) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    console_println_color!(
        Color::RED,
        "    SMAP     : {}",
        if (cr4 & (1 << 21)) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    console_println_color!(
        Color::RED,
        "    NX       : {}",
        if (efer & (1 << 11)) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    console_println_color!(
        Color::RED,
        "    SYSCALL  : {}",
        if (efer & (1 << 0)) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "  CR2 is only meaningful after a page fault."
    );

    console_println_color!(
        Color::RED,
        "  System halted."
    );

    console_println_color!(
        Color::RED,
        ""
    );

    console_println_color!(
        Color::RED,
        "############################################################"
    );

    fb::present();

    // ------------------------------------------------------------
    // Halt forever.
    // ------------------------------------------------------------

    loop {
        x86_64::instructions::hlt();
    }
}
