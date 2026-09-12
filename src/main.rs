#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;
use alloc::{boxed::Box, vec::Vec, string::String, collections::BTreeMap};

mod fb;
mod fs;
mod acpi;
mod font;
mod serial;
mod test;
mod kmem;
pub mod int;
pub mod gdt;
mod pic;
mod console;
mod shell;

#[cfg(feature = "test")]
#[path = "../tests/trivial_assert.rs"]
mod trivial_assert_test;

#[cfg(feature = "test")]
#[path = "../tests/interrupts.rs"]
mod interrupts_test;

use crate::{console::with_console, fb::Color};
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

    #[cfg(feature = "test")]
    {
        unsafe { TESTING = true; }
        test::run();
    }

    #[cfg(not(feature = "test"))]
    {
        unsafe { TESTING = false; }
        kernel();
    }

    loop {
        x86_64::instructions::hlt();
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
        kmem::init_heap(
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

fn kernel() {
    console_println!("Welcome to Oxenna!");
    console_print!("> ");
    fb::present();

}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Stop the world. If a timer or keyboard interrupt fires while we're
    // mid-panic, it could re-enter code that touches whatever just broke.
    x86_64::instructions::interrupts::disable();

    let location = info
        .location()
        .map(|loc| (loc.file(), loc.line(), loc.column()));

    let (cr3_frame, _) = x86_64::registers::control::Cr3::read();
    let cr3 = cr3_frame.start_address().as_u64();

    let cr2 = x86_64::registers::control::Cr2::read()
        .map(|addr| addr.as_u64())
        .unwrap_or(0);

    // ---- serial: plain text, no color codes, easy to grep in logs ----
    serial_println!();
    serial_println!("================================================================");
    serial_println!("KERNEL PANIC");
    serial_println!("================================================================");

    match location {
        Some((file, line, col)) => {
            serial_println!("  Location: {}:{}:{}", file, line, col);
        }
        None => {
            serial_println!("  Location: unknown");
        }
    }

    serial_println!("  Message:  {}", info.message());
    serial_println!("  CR2:      {:#018x}  (last page-fault address)", cr2);
    serial_println!("  CR3:      {:#018x}  (active page table)", cr3);
    serial_println!("================================================================");
    serial_println!();

    // ---- framebuffer console: same info, boxed for visibility ----
    console_println_color!(Color::RED, "");
    console_println_color!(Color::RED, "########################################");
    console_println_color!(Color::RED, "#            KERNEL PANIC              #");
    console_println_color!(Color::RED, "########################################");
    console_println_color!(Color::RED, "");

    match location {
        Some((file, line, col)) => {
            console_println_color!(Color::RED, "  at {}:{}:{}", file, line, col);
        }
        None => {
            console_println_color!(Color::RED, "  at unknown location");
        }
    }

    console_println_color!(Color::RED, "");
    console_println_color!(Color::RED, "  {}", info.message());
    console_println_color!(Color::RED, "");
    console_println_color!(Color::RED, "  CR2: {:#018x}", cr2);
    console_println_color!(Color::RED, "  CR3: {:#018x}", cr3);
    console_println_color!(Color::RED, "");
    console_println_color!(Color::RED, "  System halted.");
    console_println_color!(Color::RED, "");

    fb::present();

    loop {
        x86_64::instructions::hlt();
    }
}
