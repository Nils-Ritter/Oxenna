use pic8259::ChainedPics;
use spin::Mutex;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = 40;

pub static PICS: Mutex<ChainedPics> =
    unsafe { Mutex::new(ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET)) };

pub fn init() {
    unsafe {
        let mut pics = PICS.lock();

        // Remap the PICs to vectors 32..47.
        pics.initialize();

        // Enable IRQ0 (PIT timer)
        // and IRQ1 (keyboard).
        //
        // PIC1 mask:
        //
        // bit 0 = IRQ0
        // bit 1 = IRQ1
        //
        // 0 = enabled
        // 1 = masked
        pics.write_masks(0b1111_1100, 0b1111_1111);
    }
}

pub fn end_of_interrupt(interrupt_id: u8) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(interrupt_id);
    }
}
