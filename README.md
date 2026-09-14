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
git clone https://github.com/Nils-Ritter/Oxenna.git
cd Oxenna
```

Build the kernel and create a bootable image:

```sh
make
```

The finished ISOs are placed in the projects root directory.

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
├── rust-toolchain        # Rust toolchain setting
└── Makefile              # The makefile
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

## Roadmap

- [ ] Implement a disk driver
- [ ] Improve automated testing
- [ ] Support additional hardware
- [ ] Write a basic scheduler

## License

This project is licensed under the terms of the license included in this repository.
