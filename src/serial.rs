use alloc::fmt;

use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::{Config, Uart16550Tty, backend::PioBackend};

lazy_static! {
    pub static ref SERIAL1: Mutex<Uart16550Tty<PioBackend>> = Mutex::new(unsafe {
        Uart16550Tty::new_port(0x3F8, Config::default()).expect("failed to initialize UART")
    });
}

/// Low-level formatted serial output.
///
/// This is the common implementation used by both
/// `serial_print!` and `serial_println!`.
#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments<'_>) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        SERIAL1
            .lock()
            .write_fmt(args)
            .expect("Printing to serial failed");
    });
}

/// Write formatted data to the serial port.
pub fn write_fmt(args: fmt::Arguments<'_>) {
    _print(args);
}

/// Prints to the host through the serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::write_fmt(
            core::format_args!($($arg)*)
        )
    };
}

/// Prints to the host through the serial interface,
/// appending a newline.
#[macro_export]
macro_rules! serial_println {
    () => {
        $crate::serial::write_fmt(
            core::format_args!("\n")
        )
    };

    ($($arg:tt)*) => {
        $crate::serial::write_fmt(
            core::format_args!(
                "{}\n",
                core::format_args!($($arg)*)
            )
        )
    };
}
