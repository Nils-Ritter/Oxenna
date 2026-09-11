extern crate alloc;
use core::alloc::Layout;
use alloc::alloc::{alloc, dealloc};
use x86_64::{VirtAddr, structures::paging::{PageTable, Translate}};

use crate::{acpi, console::{self, Console, clear, with_console}, console_print, console_println, console_println_color, fb::{self, Color}, fs::{Entry, FS}, kmem, test::exit_qemu};
use crate::test::TestResult;
use crate::kmem::mem_analyze;

pub fn execute(line: &str) {
    let mut parts = line.split_whitespace();

    let command = match parts.next() {
        Some(command) => command,
        None => return,
    };

    match command {
        "help" => help(),
        "clear" => clearterm(),
        "echo" => echo(parts),
        "info" => info(),
        "panic" => panic(),
        "bp" => bp(),
        "sven" => sven(), //NOTE: Do not add this to help
        "reboot" => reboot(),
        "shutdown" => shutdown(),
        "exit" => shutdown(),
        "setbg" => setbg(parts),
        "setfg" => setfg(parts),
        "mem-analyze" => mem_analyze_cmd(),
        "tg-serial" => toggle_serial(),
        "alloc!" => alloc_cmd(parts),
        "dealloc!" => dealloc_cmd(parts),
        "ls" => ls(parts),
        "mkdir" => mkdir(parts),
        "touch" => touch(parts),
        _ => {
            console_println!("Unknown command: {}", command);
            console_println!("Type 'help' for a list of commands.");
        }
    }
}

fn help() {
    console_println!("Available commands:");
    console_println!("  help       - Show this help");
    console_println!("  clear      - Clear the screen");
    console_println!("  echo       - Print text");
    console_println!("  info       - Show system information");
    console_println!("  panic      - Intentionally throws a kernel panic");
    console_println!("  bp         - Sets and steps over a breakpoint");
    console_println!("  reboot     - Reboots the machine.");
    console_println!("  shutdown   - Shuts the computer down.");
    console_println!("  exit       - Shuts the computer down.");
    console_println!("  setbg      - Sets the background color.");
    console_println!("  setfg      - Sets the foreground color.");
    console_println!("  tg-serial  - Toggles priting kTerm output to serial.");
    console_println!("  alloc!     - Allocate N bytes, prints a pointer");
    console_println!("  dealloc!   - Free a pointer previously returned by alloc");
    console_println!("  ls         - List directory contents");
    console_println!("  mkdir      - Create a directory");
    console_println!("  touch      - Create an empty file");
}

fn ls(mut args: core::str::SplitWhitespace<'_>) {
    let path = args.next().unwrap_or("/");

    let entries = FS.lock().list_entries(path);

    match entries {
        Ok(mut entries) => {
            if entries.is_empty() {
                return;
            }

            entries.sort_by(|a, b| name_of(a).cmp(name_of(b)));

            for entry in entries {
                match entry {
                    Entry::Dir(name) => console_println_color!(Color::BLUE, "{}/", name),
                    Entry::File(name) => console_println!("{}", name),
                }
            }
        }
        Err(_) => console_println!("ls: cannot access '{}': No such directory", path),
    }
}

fn name_of(entry: &Entry) -> &str {
    match entry {
        Entry::Dir(name) | Entry::File(name) => name,
    }
}

fn mkdir(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: mkdir <path>");
        return;
    };

    match FS.lock().mkdir(path) {
        Ok(()) => {}
        Err(_) => console_println!("mkdir: cannot create directory '{}': already exists or parent missing", path),
    }
}

fn touch(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: touch <path>");
        return;
    };

    match FS.lock().touch(path) {
        Ok(()) => {}
        Err(_) => console_println!("touch: cannot create '{}': is a directory or parent missing", path),
    }
}

fn clearterm() {
    console::clear();
}

fn echo(args: core::str::SplitWhitespace<'_>) {
    let mut first = true;

    for arg in args {
        if !first {
            console_print!(" ");
        }

        console_print!("{}", arg);
        first = false;
    }

    console_println!();
}

fn toggle_serial(){
    let state = console::serial_mirror_enabled();
    console::set_serial_mirror(!state);
    console_println_color!(Color::GREEN, "Toggled serial mirroring");
}

fn info() {
    console_println!("Oxenna");
    console_println!("Architecture: x86_64");
    console_println!("Bootloader: Limine");
    console_println!("Framebuffer: {}x{}", fb::width(), fb::height());

}

fn panic(){
    panic!("Intentional debug panic");
}

fn bp(){
    x86_64::instructions::interrupts::int3();
}

fn sven(){
    console_println!("This command is dedicated to my friend bunny, sven!");
    console_println!("Say bye bye to your pc :)");
    acpi::reboot();
}

fn reboot(){
    acpi::reboot();
}

fn shutdown(){
    console_println!("There currently is no support for acpi shutdown.");
    console_println!("However, qemu will close normally.");
    exit_qemu(true);
}

fn setbg(mut args: core::str::SplitWhitespace<'_>) {
    let Some(color_name) = args.next() else {
        console_println!("Incorrect usage!");
        console_println!("Usage: setbg <color>");
        return;
    };

    let Some(color) = Color::from_name(color_name) else {
        console_println!("Unknown color: {}", color_name);
        return;
    };

    with_console(|console| {
        Console::set_background(console, color);
    });

    with_console(|console| {
        Console::clear(console);
    });
}

fn setfg(mut args: core::str::SplitWhitespace<'_>) {
    let Some(color_name) = args.next() else {
        console_println!("Incorrect usage!");
        console_println!("Usage: setfg <color>");
        return;
    };

    let Some(color) = Color::from_name(color_name) else {
        console_println!("Unknown color: {}", color_name);
        return;
    };

    with_console(|console| {
        Console::set_foreground(console, color);
    });

    with_console(|console| {
        Console::clear(console);
    });
}

fn mem_analyze_cmd(){
    kmem::mem_analyze();
}

fn alloc_cmd(mut args: core::str::SplitWhitespace<'_>) {
    let Some(size_str) = args.next() else {
        console_println!("Incorrect usage!");
        console_println!("Usage: alloc <size> [align]");
        return;
    };

    let Ok(size) = size_str.parse::<usize>() else {
        console_println!("Invalid size: {}", size_str);
        return;
    };

    if size == 0 {
        console_println!("Size must be greater than 0");
        return;
    }

    let align = match args.next() {
        Some(align_str) => match align_str.parse::<usize>() {
            Ok(align) if align.is_power_of_two() => align,
            _ => {
                console_println!("Invalid alignment: {} (must be a power of two)", align_str);
                return;
            }
        },
        None => core::mem::align_of::<usize>(),
    };

    let Ok(layout) = Layout::from_size_align(size, align) else {
        console_println!("Invalid layout: size={} align={}", size, align);
        return;
    };

    let ptr = unsafe { alloc(layout) };

    if ptr.is_null() {
        console_println_color!(Color::RED, "Allocation failed: out of memory");
        return;
    }

    console_println_color!(
        Color::GREEN,
        "Allocated {} bytes (align {}) at {:#x}",
        size,
        align,
        ptr as usize
    );
    console_println!(
        "To free: dealloc {:#x} {} {}",
        ptr as usize,
        size,
        align
    );
}

fn dealloc_cmd(mut args: core::str::SplitWhitespace<'_>) {
    let Some(ptr_str) = args.next() else {
        console_println!("Incorrect usage!");
        console_println!("Usage: dealloc <ptr> <size> [align]");
        return;
    };

    let Some(size_str) = args.next() else {
        console_println!("Incorrect usage!");
        console_println!("Usage: dealloc <ptr> <size> [align]");
        return;
    };

    let trimmed = ptr_str
        .trim_start_matches("0x")
        .trim_start_matches("0X");

    let Ok(addr) = usize::from_str_radix(trimmed, 16) else {
        console_println!("Invalid pointer: {}", ptr_str);
        return;
    };

    let Ok(size) = size_str.parse::<usize>() else {
        console_println!("Invalid size: {}", size_str);
        return;
    };

    let align = match args.next() {
        Some(align_str) => match align_str.parse::<usize>() {
            Ok(align) if align.is_power_of_two() => align,
            _ => {
                console_println!("Invalid alignment: {} (must be a power of two)", align_str);
                return;
            }
        },
        None => core::mem::align_of::<usize>(),
    };

    let Ok(layout) = Layout::from_size_align(size, align) else {
        console_println!("Invalid layout: size={} align={}", size, align);
        return;
    };

    let ptr = addr as *mut u8;

    if ptr.is_null() {
        console_println!("Cannot deallocate a null pointer");
        return;
    }

    unsafe {
        dealloc(ptr, layout);
    }

    console_println_color!(Color::GREEN, "Deallocated {:#x}", addr);
}

//TESTS

fn test_set_color(
    color_name: &str,
    expected: Color,
    set_color: fn(core::str::SplitWhitespace<'_>),
    get_color: fn(&mut Console) -> Color,
    error_message: &'static str,
) -> TestResult {
    set_color(color_name.split_whitespace());

    with_console(|console| {
        if get_color(console) == expected {
            TestResult::Pass
        } else {
            TestResult::Fail(error_message)
        }
    })
}

///DO NOT USE GLOBALLY.
///Only to be used if you know EXACTLY what this does.
///Youve been warned.
macro_rules! color_test {
    ($test_name:ident, $setter:ident, $getter:path, $name:expr, $color:expr, $error:expr) => {
        #[test]
        fn $test_name() -> TestResult {
            test_set_color(
                $name,
                $color,
                $setter,
                $getter,
                $error,
            )
        }
    };
}

color_test!(
    test_setbg_red,
    setbg,
    Console::get_background,
    "red",
    Color::RED,
    "set red failed"
);

color_test!(
    test_setbg_black,
    setbg,
    Console::get_background,
    "black",
    Color::BLACK,
    "set black failed"
);

color_test!(
    test_setbg_green,
    setbg,
    Console::get_background,
    "green",
    Color::GREEN,
    "set green failed"
);

color_test!(
    test_setbg_blue,
    setbg,
    Console::get_background,
    "blue",
    Color::BLUE,
    "set blue failed"
);

color_test!(
    test_setbg_white,
    setbg,
    Console::get_background,
    "white",
    Color::WHITE,
    "set white failed"
);

color_test!(
    test_setfg_red,
    setfg,
    Console::get_foreground,
    "red",
    Color::RED,
    "set red failed"
);

color_test!(
    test_setfg_black,
    setfg,
    Console::get_foreground,
    "black",
    Color::BLACK,
    "set black failed"
);

color_test!(
    test_setfg_green,
    setfg,
    Console::get_foreground,
    "green",
    Color::GREEN,
    "set green failed"
);

color_test!(
    test_setfg_blue,
    setfg,
    Console::get_foreground,
    "blue",
    Color::BLUE,
    "set blue failed"
);

color_test!(
    test_setfg_white,
    setfg,
    Console::get_foreground,
    "white",
    Color::WHITE,
    "set white failed"
);
