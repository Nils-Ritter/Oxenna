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
    let (mut mapper, mut frame_allocator) = unsafe { kmem::init() };
    unsafe {
        kmem::init_heap(&mut mapper, &mut frame_allocator).expect("failed to init heap");
    }
    console_println_color!(Color::GREEN, "[OK]");

    console_println!();
    console_println!("[INFO] Kernel is running.");
    console_println!("[INFO] Timer interrupts should now arrive.");
    console_println!("[INFO] Press keys to test keyboard IRQs.");
    console_println!();
}

fn kernel() {
    console::clear();
    console_println!("Welcome to Oxenna!");
    fb::present();
    console_print!("> ");
    fb::present();

}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    console_println_color!(Color::RED, "KERNEL PANIC: {}", info);
    serial_println!("KERNEL PANIC: {}", info);
    fb::present();
    loop {
        core::hint::spin_loop();
    }
}
