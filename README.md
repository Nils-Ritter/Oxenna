<p align="center">
  <img src="logo_slim.webp" alt="Logo" width="2000">
</p>

# Oxenna

Oxenna is a small hobby operating system for x86_64, written in x86-Assembly and Rust.

The project is intended for learning and experimentation with operating-system development,
including kernel initialization, memory management, hardware access, interrupts,
and bare-metal Rust programming.

> **Status:** Working on userspace

## Features

- x86_64 kernel
- Written in Rust
- `no_std` bare-metal environment
- Bootable using the Limine bootloader
- Serial output for debugging
- Kernel tests running inside QEMU

## Requirements

- Linux, macOS, or Windows with WSL
- Rust nightly toolchain
- `rust-src` component
- `llvm-tools-preview` component
- QEMU
- `limine`

Install the Rust components and tools with:

```sh
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
```

On Debian or Ubuntu, install QEMU with:

```sh
sudo apt update
sudo apt install qemu-system-x86 qemu-utils
```

## Building

Clone the repository:

```sh
git clone https://github.org/Nils-Ritter/Oxenna.git
cd Oxenna
```

Build the kernel and create a bootable image:

```sh
make
```

The finished ISOs are placed in the projects root directory.

## Build configuration

Oxenna uses a small Kconfig-compatible configuration layer inspired by the Linux kernel.
The configuration is compile-time: selected components are passed to Cargo as features, so
unselected drivers/filesystems are left out of the kernel binary entirely.

Open the interactive configuration UI with:

```sh
make menuconfig
```

The UI supports keyboard navigation, Space to toggle options, and S to save. Dependencies
are enforced automatically; for example, the ext2 filesystem requires both the block-device
framework and the ATA driver. The resulting configuration is stored in `.config`, while
`out/config/config.mk` is generated for the Makefile.

Current configuration groups include:

- **General setup** — optimized release builds, userspace loader, and the interactive shell.
- **Drivers** — block-device framework and ATA PIO disk driver.
- **Filesystems** — ext2.

These are built-in kernel components rather than dynamically loadable modules. Adding a new
driver or subsystem means adding a Kconfig symbol, a Cargo feature, and the corresponding
`cfg(feature = "...")` gates in the Rust module tree.

After changing the configuration, build normally with:

```sh
make
```

To regenerate the configuration without opening the UI (useful after adding new Kconfig
symbols), run:

```sh
make olddefconfig
```

## Running

Run the operating system in QEMU:

```sh
make run
```

## Testing

Run the kernel tests inside QEMU:

```sh
make test
```

Tests are ran in a custom test harness, results are printed in the serial
console along with the normal kernel output.

## Project Structure

```text
.
├── .cargo/
│   └── config.toml       # Cargo target and runner configuration
├── src/
│   └── main.rs           # Kernel entry point
│   └── ...
├── limine/               # The limine bootloader files
│   └── ...
├── tests/                # The tests
│   └── ...
├── Cargo.toml            # Project manifest
├── Cargo.lock            # Locked dependency versions
├── test_macro/           # Cargo project for testing
│   └── src/
│       └── lib.rs
│   └── Cargo.toml        
├── README.md             # The file youre reading right now
├── limine.conf           # Limine bootloader config
├── linker.ld             # Linker config
├── rust-toolchain.toml   # Rust toolchain setting
├── Kconfig              # Kernel configuration symbols and dependencies
├── scripts/kconfig.py   # Lightweight menuconfig implementation
└── Makefile              # Build orchestration and menuconfig entry points
```

## Development

Format the source code:

```sh
cargo fmt
```

Check the project without running it:

```sh
cargo check
```

Because Oxenna is a bare-metal project,
some standard Rust tooling and libraries are not available inside the kernel.

## `.ox` userspace applications

Oxxena applications are ordinary **x86-64 ELF64** executables. The `.ox`
extension is only a naming convention; the loader checks the ELF header and
program headers rather than the filename contents.

The current loader supports static `ET_EXEC` and `ET_DYN` binaries with
`PT_LOAD` segments. ELF interpreters (`PT_INTERP`) and dynamic loading are not
implemented yet.

A small direct-syscall example is included:

```text
apps/hello.S
```

Build it and put it on the ext2 disk image:

```bash
make apps
make disk-apps
make run
```

It will be installed as:

```text
/bin/hello.ox
```

Then from the Oxenna shell:

```text
run /bin/hello.ox
```

You can also install any externally-built ELF file:

```bash
make disk-install APP=hello.ox DEST=/bin/hello.ox
```

The syscall ABI uses Linux x86-64 syscall numbers for the implemented calls,
including `read`, `write`, `open`, `close`, `stat`, `fstat`, `lseek`, `mmap`,
`munmap`, `brk`, `getpid`, `uname`, `getdents64`, `clock_gettime`,
`arch_prctl`, `futex`, `openat`, `newfstatat`, `exit`, and `exit_group`.

The process model is intentionally small: one userspace process runs at a
time, `exit` returns control to the shell, and the process's user mappings are
reclaimed afterwards.

## Roadmap

- [x] Implement a disk driver
- [ ] Improve automated testing
- [ ] Support additional hardware
- [ ] Write a basic scheduler

## License

This project is licensed under the terms of the license included in this repository.
