use spin::Once;

use core::sync::atomic::{
    AtomicU8,
    Ordering,
};

use x86_64::{
    registers::control::Cr2,
    structures::idt::{
        InterruptDescriptorTable,
        InterruptStackFrame,
        PageFaultErrorCode,
    },
};

use crate::{
    acpi, console, console_print, fb::Color, gdt, pic, shell::shutdown,
};

static IDT: Once<InterruptDescriptorTable> =
    Once::new();

// ==========================================================
// Keyboard configuration
// ==========================================================

/*
 * Number of terminal lines moved by one Ctrl+Arrow event.
 */
const SCROLL_LINES: usize = 3;

// ==========================================================
// Interrupt indexes
// ==========================================================

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum InterruptIndex {
    Timer = 32,
    Keyboard = 33,
}

impl InterruptIndex {
    pub const fn as_u8(
        self,
    ) -> u8 {
        self as u8
    }

    pub const fn as_usize(
        self,
    ) -> usize {
        self as usize
    }
}

// ==========================================================
// Ctrl state
// ==========================================================
//
// Bit 0 = left Ctrl
// Bit 1 = right Ctrl
//
// Atomic state avoids taking a spinlock from the keyboard IRQ.
//

const CTRL_LEFT: u8 = 1 << 0;
const CTRL_RIGHT: u8 = 1 << 1;

static CTRL_STATE: AtomicU8 =
    AtomicU8::new(0);

#[inline(always)]
fn ctrl_held() -> bool {
    CTRL_STATE.load(
        Ordering::Relaxed,
    ) != 0
}

#[inline(always)]
fn set_ctrl(
    mask: u8,
) {
    CTRL_STATE.fetch_or(
        mask,
        Ordering::Relaxed,
    );
}

#[inline(always)]
fn clear_ctrl(
    mask: u8,
) {
    CTRL_STATE.fetch_and(
        !mask,
        Ordering::Relaxed,
    );
}

// ==========================================================
// Initialization
// ==========================================================

pub fn init() {
    IDT.call_once(|| {
        let mut idt =
            InterruptDescriptorTable::new();

        // --------------------------------------------------
        // CPU exceptions
        // --------------------------------------------------

        idt.breakpoint
            .set_handler_fn(
                breakpoint_handler,
            );

        unsafe {
            idt.double_fault
                .set_handler_fn(
                    double_fault_handler,
                )
                .set_stack_index(
                    gdt::DOUBLE_FAULT_IST_INDEX,
                );
        }

        idt.general_protection_fault
            .set_handler_fn(
                general_protection_fault_handler,
            );

        idt.invalid_opcode
            .set_handler_fn(
                invalid_opcode_handler,
            );

        idt.page_fault
            .set_handler_fn(
                page_fault_handler,
            );

        idt.divide_error
            .set_handler_fn(
                divide_error_handler,
            );

        idt.invalid_tss
            .set_handler_fn(
                invalid_tss_handler,
            );

        idt.segment_not_present
            .set_handler_fn(
                segment_not_present_handler,
            );

        idt.stack_segment_fault
            .set_handler_fn(
                stack_segment_fault_handler,
            );

        idt.alignment_check
            .set_handler_fn(
                alignment_check_handler,
            );

        idt.x87_floating_point
            .set_handler_fn(
                x87_floating_point_handler,
            );

        idt.simd_floating_point
            .set_handler_fn(
                simd_floating_point_handler,
            );

        idt.virtualization
            .set_handler_fn(
                virtualization_handler,
            );

        idt.device_not_available
            .set_handler_fn(
                device_not_available_handler,
            );

        idt.debug
            .set_handler_fn(
                debug_handler,
            );

        idt.overflow
            .set_handler_fn(
                overflow_handler,
            );

        idt.bound_range_exceeded
            .set_handler_fn(
                bound_range_exceeded_handler,
            );

        idt.non_maskable_interrupt
            .set_handler_fn(
                non_maskable_interrupt_handler,
            );

        idt.machine_check
            .set_handler_fn(
                machine_check_handler,
            );

        // --------------------------------------------------
        // Hardware IRQs
        // --------------------------------------------------

        idt[
            InterruptIndex::Timer.as_u8()
        ]
        .set_handler_fn(
            timer_interrupt_handler,
        );

        idt[
            InterruptIndex::Keyboard.as_u8()
        ]
        .set_handler_fn(
            keyboard_interrupt_handler,
        );

        idt
    })
    .load();
}

// ==========================================================
// CPU EXCEPTIONS
// ==========================================================

extern "x86-interrupt" fn breakpoint_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: BREAKPOINT"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    crate::console_println_color!(
        Color::RED,
        "EXCEPTION: BREAKPOINT"
    );

    crate::console_println_color!(
        Color::RED,
        "{:#?}",
        stack_frame
    );
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    crate::serial_println!();
    crate::serial_println!(
        "EXCEPTION: DOUBLE FAULT"
    );

    crate::serial_println!(
        "error code: {:#x}",
        error_code
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: GENERAL PROTECTION FAULT"
    );

    crate::serial_println!(
        "error code: {:#x}",
        error_code
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn invalid_opcode_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: INVALID OPCODE"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let address =
        Cr2::read();

    let from_user =
        stack_frame
            .code_segment
            .rpl()
            == x86_64::PrivilegeLevel::Ring3;

    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: PAGE FAULT"
    );

    crate::serial_println!(
        "accessed address: {:?}",
        address
    );

    crate::serial_println!(
        "error code: {:?}",
        error_code
    );

    crate::serial_println!(
        "CPL: {:?}",
        stack_frame.code_segment.rpl()
    );

    crate::serial_println!(
        "from user: {}",
        from_user
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn divide_error_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: DIVIDE ERROR"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn invalid_tss_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: INVALID TSS"
    );

    crate::serial_println!(
        "error code: {:#x}",
        error_code
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: SEGMENT NOT PRESENT"
    );

    crate::serial_println!(
        "error code: {:#x}",
        error_code
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: STACK SEGMENT FAULT"
    );

    crate::serial_println!(
        "error code: {:#x}",
        error_code
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::serial_println!();

    crate::serial_println!(
        "EXCEPTION: ALIGNMENT CHECK"
    );

    crate::serial_println!(
        "error code: {:#x}",
        error_code
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn x87_floating_point_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: x87 FLOATING POINT"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn simd_floating_point_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: SIMD FLOATING POINT"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn virtualization_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: VIRTUALIZATION"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn device_not_available_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: DEVICE NOT AVAILABLE"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn debug_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: DEBUG"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );
}

extern "x86-interrupt" fn overflow_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: OVERFLOW"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn bound_range_exceeded_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: BOUND RANGE EXCEEDED"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn non_maskable_interrupt_handler(
    stack_frame: InterruptStackFrame,
) {
    crate::serial_println!(
        "EXCEPTION: NON-MASKABLE INTERRUPT"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

extern "x86-interrupt" fn machine_check_handler(
    stack_frame: InterruptStackFrame,
) -> ! {
    crate::serial_println!(
        "EXCEPTION: MACHINE CHECK"
    );

    crate::serial_println!(
        "{:#?}",
        stack_frame
    );

    panic_loop();
}

// ==========================================================
// HARDWARE INTERRUPTS
// ==========================================================

extern "x86-interrupt" fn timer_interrupt_handler(
    _stack_frame: InterruptStackFrame,
) {
    /*
     * Keep the timer IRQ extremely cheap.
     */
    pic::end_of_interrupt(
        InterruptIndex::Timer.as_u8(),
    );
}

// ==========================================================
// Keyboard interrupt
// ==========================================================

extern "x86-interrupt" fn keyboard_interrupt_handler(
    _stack_frame: InterruptStackFrame,
) {
    use pc_keyboard::{
        layouts,
        DecodedKey,
        HandleControl,
        KeyCode,
        KeyState,
        Keyboard,
        ScancodeSet1,
    };

    use x86_64::instructions::port::Port;

    use crate::pic::PICS;

    /*
     * The decoder persists across interrupts.
     */
    static KEYBOARD: spin::Mutex<
        Keyboard<
            layouts::Us104Key,
            ScancodeSet1,
        >
    > = spin::Mutex::new(
        Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore,
        ),
    );

    let mut keyboard =
        KEYBOARD.lock();

    let mut port =
        Port::new(0x60);

    /*
     * Read the PS/2 scancode immediately.
     */
    let scancode: u8 =
        unsafe {
            port.read()
        };

    if let Ok(Some(key_event)) =
        keyboard.add_byte(scancode)
    {
        // ==================================================
        // Ctrl state
        // ==================================================

        match key_event.code {
            KeyCode::LControl => {
                match key_event.state {
                    KeyState::Down
                    | KeyState::SingleShot => {
                        set_ctrl(CTRL_LEFT);
                    }

                    KeyState::Up => {
                        clear_ctrl(CTRL_LEFT);
                    }
                }
            }

            KeyCode::RControl => {
                match key_event.state {
                    KeyState::Down
                    | KeyState::SingleShot => {
                        set_ctrl(CTRL_RIGHT);
                    }

                    KeyState::Up => {
                        clear_ctrl(CTRL_RIGHT);
                    }
                }
            }

            _ => {}
        }

        // ==================================================
        // Decode key
        // ==================================================

        if let Some(key) =
            keyboard.process_keyevent(
                key_event,
            )
        {
            match key {
                // ==========================================
                // Unicode input
                // ==========================================

                DecodedKey::Unicode(character) => {
                    /*
                     * Ctrl+Arrow is represented as a RawKey,
                     * so normal Unicode input can go directly
                     * to the console.
                     */
                    let ctrl =
                        ctrl_held();

                    console::receive_key(
                        character,
                    );

                    match character {
                        'l' if ctrl => {
                            console::clear();
                            console_print!("$ ");
                        }
                        _ => {}
                    }
                }

                // ==========================================
                // Raw keys
                // ==========================================

                DecodedKey::RawKey(key_code) => {
                    /*
                     * Atomic load instead of taking the old
                     * CtrlState spinlock.
                     */
                    let ctrl =
                        ctrl_held();

                    match key_code {
                        // ----------------------------------
                        // Ctrl + Up
                        // ----------------------------------

                        KeyCode::ArrowUp
                            if ctrl =>
                        {
                            console::scroll_up(
                                SCROLL_LINES,
                            );
                        }

                        // ----------------------------------
                        // Ctrl + Down
                        // ----------------------------------

                        KeyCode::ArrowDown
                            if ctrl =>
                        {
                            console::scroll_down(
                                SCROLL_LINES,
                            );
                        }

                        // ----------------------------------
                        // Ctrl itself
                        // ----------------------------------

                        KeyCode::LControl
                        | KeyCode::RControl => {}

                        // ----------------------------------
                        // Other raw keys
                        // ----------------------------------

                        _ => {}
                    }
                }
            }
        }
    }

    // ======================================================
    // End of interrupt
    // ======================================================

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(
                InterruptIndex::Keyboard.as_u8(),
            );
    }
}

// ==========================================================
// Panic
// ==========================================================

fn panic_loop() -> ! {
    x86_64::instructions::interrupts::disable();

    loop {
        core::hint::spin_loop();
    }
}
