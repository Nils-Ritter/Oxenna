pub fn qemu_shutdown(
    success: bool,
) -> ! {
    //
    // QEMU's isa-debug-exit device listens on
    // port 0xf4.
    //

    let code: u32 =
        if success {
            0x10
        } else {
            0x11
        };

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4u16,
            in("eax") code,
            options(
                nomem,
                nostack,
                preserves_flags
            ),
        );
    }

    loop {
        core::hint::spin_loop();
    }
}
