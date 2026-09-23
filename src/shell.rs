extern crate alloc;

use core::alloc::Layout;
use alloc::{alloc::alloc, alloc::dealloc, string::String};
use spin::Mutex;

#[cfg(feature = "userspace")]
use crate::user;

use crate::{
    acpi,
    console::{self, Console, with_console},
    console_print, console_println, console_println_color,
    fb::{self, Color},
    fs::{Entry, FS},
    kmem::{self, FRAME_ALLOCATOR},
};
use crate::test::TestResult;
use crate::test::test;

#[cfg(feature = "driver_qemu")]
use crate::drivers::qemu::qemu_shutdown;


/// Shell working directory, stored as a normalized absolute path.
///
/// The ext2 wrapper currently resolves paths from ROOT_INO, so the shell
/// resolves relative paths here before passing them to the filesystem.
static CURRENT_DIR: Mutex<String> = Mutex::new(String::new());


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
        "sven" => sven(), // NOTE: Do not add this to help
        "reboot" => reboot(),
        "shutdown" => shutdown(),
        "exit" => shutdown(),
        "setbg" => setbg(parts),
        "setfg" => setfg(parts),
        "mem-analyze" => mem_analyze_cmd(),
        "tg-serial" => toggle_serial(),
        "alloc!" => alloc_cmd(parts),
        "dealloc!" => dealloc_cmd(parts),

        // Filesystem commands.
        "ls" => ls(parts),
        "cd" => cd(parts),
        "pwd" => pwd(),
        "mkdir" => mkdir(parts),
        "touch" => touch(parts),
        "rm" => rm(parts),
        "rmdir" => rmdir(parts),
        "mv" => mv(parts),
        "cat" => cat(parts),
        "write" => write_file(parts),
        "stat" => stat(parts),
        #[cfg(feature = "userspace")]
        "run" => run(parts),

        _ => {
            #[cfg(feature = "userspace")]
            run_binapp(command, parts);

            #[cfg(not(feature = "userspace"))]
            console_println_color!(Color::RED, "No command or binapp found for: {}", command);
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
    console_println!("  tg-serial  - Toggles printing kTerm output to serial.");
    console_println!("  alloc!     - Allocate N bytes, prints a pointer");
    console_println!("  dealloc!   - Free a pointer previously returned by alloc");
    console_println!();
    console_println!("Filesystem:");
    console_println!("  ls [path]          - List directory contents");
    console_println!("  cd [path]          - Change directory");
    console_println!("  pwd                - Print working directory");
    console_println!("  mkdir <path>       - Create a directory");
    console_println!("  touch <path>       - Create an empty file");
    console_println!("  rm <path>          - Remove a file");
    console_println!("  rmdir <path>       - Remove an empty directory");
    console_println!("  mv <old> <new>     - Rename/move a file or directory");
    console_println!("  cat <path>         - Print a file");
    console_println!("  write <path> <txt> - Replace a file with text");
    console_println!("  stat <path>        - Show file metadata");
    #[cfg(feature = "userspace")]
    console_println!("  run <app.ox> [arg] - Execute an ELF .ox program");
}

fn fs_error(command: &str, path: &str, err: impl core::fmt::Debug) {
    console_println!("{}: '{}': {:?}", command, path, err);
}

#[cfg(feature = "userspace")]
fn run_binapp(command: &str, args: core::str::SplitWhitespace<'_>) {
    let exact = alloc::format!("/bin/{}", command);
    let with_ox = alloc::format!("/bin/{}.ox", command);

    let path = {
        let mut fs = FS.lock();

        match fs.stat(&exact) {
            Ok(stat) if matches!(stat.file_type, crate::fs::FileType::Regular) => exact,
            _ => match fs.stat(&with_ox) {
                Ok(stat) if matches!(stat.file_type, crate::fs::FileType::Regular) => with_ox,
                _ => {
                    console_println_color!(Color::RED, "No command or binapp found for: {}", command);
                    return;
                }
            },
        }
    };

    let argv: alloc::vec::Vec<&str> = args.collect();

    console_println_color!(Color::GREEN, "Starting {}...", path);

    match user::exec(&path, &argv) {
        Ok(result) => {
            console_println_color!(Color::GREEN, "{} exited with status {}", path, result.status);
        }
        Err(e) => {
            console_println!("run: '{}': {:?}", path, e);
        }
    }
}

#[cfg(feature = "userspace")]
fn run(mut args: core::str::SplitWhitespace<'_>) {
    let Some(program) = args.next() else {
        console_println!("Usage: run <program.ox> [args...]");
        return;
    };

    let path = absolute_path(program);
    if !path.ends_with(".ox") {
        console_println_color!(Color::RED, "run: '{}' is not an Oxenna .ox executable", program);
        return;
    }

    let argv: alloc::vec::Vec<&str> = args.collect();

    console_println!("Starting {}...", path);

    match user::exec(&path, &argv) {
        Ok(result) => {
            console_println_color!(Color::GREEN, "{} exited with status {}", path, result.status);
        }
        Err(e) => {
            console_println!("run: '{}': {:?}", path, e);
        }
    }
}

/// Remove the last component from an absolute path.
fn pop_path_component(path: &mut String) {
    if path == "/" {
        return;
    }

    if let Some(pos) = path.rfind('/') {
        if pos == 0 {
            path.truncate(1);
        } else {
            path.truncate(pos);
        }
    }
}

/// Resolve a shell path against the current working directory.
///
/// The filesystem wrapper currently exposes root-based path operations, so
/// this function turns every shell path into a normalized absolute path.
fn absolute_path(path: &str) -> String {
    if path.is_empty() {
        return CURRENT_DIR.lock().clone();
    }

    let mut result = if path.starts_with('/') {
        String::from("/")
    } else {
        CURRENT_DIR.lock().clone()
    };

    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => pop_path_component(&mut result),
            name => {
                if result != "/" {
                    result.push('/');
                }
                result.push_str(name);
            }
        }
    }

    if result.is_empty() {
        String::from("/")
    } else {
        result
    }
}

fn pwd() {
    let cwd = CURRENT_DIR.lock();
    if cwd.is_empty() {
        console_println!("/");
    } else {
        console_println!("{}", &*cwd);
    }
}

fn cd(mut args: core::str::SplitWhitespace<'_>) {
    let path = args.next().unwrap_or("/");

    if args.next().is_some() {
        console_println!("cd: too many arguments");
        return;
    }

    let target = absolute_path(path);

    // list_entries only succeeds when the target resolves to a directory.
    match FS.lock().list_entries(&target) {
        Ok(_) => {
            *CURRENT_DIR.lock() = target;
        }
        Err(e) => fs_error("cd", path, e),
    }
}

fn ls(mut args: core::str::SplitWhitespace<'_>) {
    let path = absolute_path(args.next().unwrap_or("."));

    match FS.lock().list_entries(&path) {
        Ok(mut entries) => {
            entries.sort_by(|a, b| name_of(a).cmp(name_of(b)));

            for entry in entries {
                match entry {
                    Entry::Dir(name) => {
                        console_println_color!(Color::BLUE, "{}/", name);
                    }
                    Entry::File(name) => {
                        console_println!("{}", name);
                    }
                }
            }
        }
        Err(e) => fs_error("ls", &path, e),
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

    let path = absolute_path(path);

    match FS.lock().mkdir(&path) {
        Ok(()) => {}
        Err(e) => fs_error("mkdir", &path, e),
    }
}

fn touch(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: touch <path>");
        return;
    };

    let path = absolute_path(path);

    match FS.lock().touch(&path) {
        Ok(()) => {}
        Err(e) => fs_error("touch", &path, e),
    }
}

fn rm(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: rm <path>");
        return;
    };

    let path = absolute_path(path);

    match FS.lock().remove(&path) {
        Ok(()) => {}
        Err(e) => fs_error("rm", &path, e),
    }
}

fn rmdir(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: rmdir <path>");
        return;
    };

    let path = absolute_path(path);

    match FS.lock().rmdir(&path) {
        Ok(()) => {}
        Err(e) => fs_error("rmdir", &path, e),
    }
}

fn mv(mut args: core::str::SplitWhitespace<'_>) {
    let Some(old) = args.next() else {
        console_println!("Usage: mv <old> <new>");
        return;
    };

    let Some(new) = args.next() else {
        console_println!("Usage: mv <old> <new>");
        return;
    };

    let old_abs = absolute_path(old);
    let new_abs = absolute_path(new);

    match FS.lock().rename(&old_abs, &new_abs) {
        Ok(()) => {}
        Err(e) => {
            console_println!("mv: '{}' -> '{}': {:?}", old, new, e);
        }
    }
}

fn cat(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: cat <path>");
        return;
    };

    let path_abs = absolute_path(path);

    match FS.lock().read_file(&path_abs) {
        Ok(data) => {
            match core::str::from_utf8(&data) {
                Ok(text) => console_print!("{}", text),
                Err(_) => {
                    console_println!("cat: '{}': binary file", path);
                }
            }
        }
        Err(e) => fs_error("cat", &path_abs, e),
    }
}

fn write_file(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: write <path> <text...>");
        return;
    };

    let path_abs = absolute_path(path);

    let mut data = alloc::vec::Vec::new();
    let mut first = true;

    for arg in args {
        if !first {
            data.push(b' ');
        }

        data.extend_from_slice(arg.as_bytes());
        first = false;
    }

    match FS.lock().write_file(&path_abs, &data) {
        Ok(()) => {}
        Err(e) => fs_error("write", &path_abs, e),
    }
}

fn stat(mut args: core::str::SplitWhitespace<'_>) {
    let Some(path) = args.next() else {
        console_println!("Usage: stat <path>");
        return;
    };

    let path_abs = absolute_path(path);

    match FS.lock().stat(&path_abs) {
        Ok(stat) => {
            console_println!("Path:   {}", path);
            console_println!("Type:   {:?}", stat.file_type);
            console_println!("Size:   {} bytes", stat.size);
            console_println!("Inode:  {}", stat.ino);
            console_println!("Links:  {}", stat.links);
            console_println!("Mode:   {:o}", stat.mode & 0o7777);
            console_println!("UID:    {}", stat.uid);
            console_println!("GID:    {}", stat.gid);
        }
        Err(e) => fs_error("stat", &path_abs, e),
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

pub fn shutdown(){
    console_println!("There currently is no support for acpi shutdown.");
    console_println!("However, qemu will close normally with the QEMU driver enabled.");
    #[cfg(feature = "driver_qemu")]
    qemu_shutdown(true);
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
    kmem::mem_analyze(FRAME_ALLOCATOR.lock().as_mut().unwrap());
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


