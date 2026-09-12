use spin::Once;

use x86_64::{
    VirtAddr,
    instructions::{
        segmentation::{CS, SS, Segment},
        tables::load_tss,
    },
    structures::{
        gdt::{
            Descriptor,
            GlobalDescriptorTable,
            SegmentSelector,
        },
        tss::TaskStateSegment,
    },
};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const DOUBLE_FAULT_STACK_SIZE: usize = 4096 * 5;
const USER_KERNEL_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct Stack<const N: usize>([u8; N]);

static mut DOUBLE_FAULT_STACK:
    Stack<DOUBLE_FAULT_STACK_SIZE> =
    Stack([0; DOUBLE_FAULT_STACK_SIZE]);

/*
 * Kernel stack used when the CPU enters ring 0
 * from ring 3.
 *
 * The CPU obtains this address from TSS.RSP0.
 */
static mut USER_KERNEL_STACK:
    Stack<USER_KERNEL_STACK_SIZE> =
    Stack([0; USER_KERNEL_STACK_SIZE]);

static TSS: Once<TaskStateSegment> =
    Once::new();

static GDT: Once<(GlobalDescriptorTable, Selectors)> =
    Once::new();

/*
 * spin::Once in the version used by Oxenna does not
 * expose the stored value directly, so keep a second
 * Once containing just the selectors.
 */
static SELECTORS: Once<Selectors> =
    Once::new();

#[derive(Clone, Copy)]
pub struct Selectors {
    pub kernel_code_selector: SegmentSelector,
    pub kernel_data_selector: SegmentSelector,

    pub user_code_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector,

    pub tss_selector: SegmentSelector,
}

pub fn selectors() -> Selectors {
    /*
     * This should only be called after init().
     *
     * SELECTORS was initialized during init(), so this
     * closure should never execute.
     */
    *SELECTORS.call_once(|| {
        panic!("gdt::selectors() called before gdt::init()")
    })
}

pub fn init() {
    // ========================================================
    // TSS
    // ========================================================

    let tss = TSS.call_once(|| {
        let mut tss =
            TaskStateSegment::new();

        // ----------------------------------------------------
        // Double-fault IST stack
        // ----------------------------------------------------

        let double_fault_stack_start =
            VirtAddr::from_ptr(
                unsafe {
                    core::ptr::addr_of!(
                        DOUBLE_FAULT_STACK.0
                    )
                },
            );

        let double_fault_stack_end =
            double_fault_stack_start
                + DOUBLE_FAULT_STACK_SIZE as u64;

        tss.interrupt_stack_table[
            DOUBLE_FAULT_IST_INDEX as usize
        ] = double_fault_stack_end;

        // ----------------------------------------------------
        // Ring 3 -> Ring 0 kernel stack
        // ----------------------------------------------------
        //
        // When an interrupt/exception occurs while CPL=3,
        // the CPU switches to TSS.RSP0 before entering the
        // kernel handler.

        let kernel_stack_start =
            VirtAddr::from_ptr(
                unsafe {
                    core::ptr::addr_of!(
                        USER_KERNEL_STACK.0
                    )
                },
            );

        let kernel_stack_end =
            kernel_stack_start
                + USER_KERNEL_STACK_SIZE as u64;

        tss.privilege_stack_table[0] =
            kernel_stack_end;

        tss
    });

    // ========================================================
    // GDT
    // ========================================================

    let (gdt, selectors) =
        GDT.call_once(|| {
            let mut gdt =
                GlobalDescriptorTable::new();

            // ------------------------------------------------
            // Kernel code
            // DPL = 0
            // ------------------------------------------------

            let kernel_code_selector =
                gdt.append(
                    Descriptor::kernel_code_segment()
                );

            // ------------------------------------------------
            // Kernel data
            // DPL = 0
            // ------------------------------------------------

            let kernel_data_selector =
                gdt.append(
                    Descriptor::kernel_data_segment()
                );

            // ------------------------------------------------
            // User code
            // DPL = 3
            // ------------------------------------------------

            let user_code_selector =
                gdt.append(
                    Descriptor::user_code_segment()
                );

            // ------------------------------------------------
            // User data
            // DPL = 3
            // ------------------------------------------------

            let user_data_selector =
                gdt.append(
                    Descriptor::user_data_segment()
                );

            // ------------------------------------------------
            // TSS
            // ------------------------------------------------

            let tss_selector =
                gdt.append(
                    Descriptor::tss_segment(tss)
                );

            (
                gdt,
                Selectors {
                    kernel_code_selector,
                    kernel_data_selector,
                    user_code_selector,
                    user_data_selector,
                    tss_selector,
                },
            )
        });

    /*
     * Publish the selectors while we still have access
     * to the value returned by GDT.call_once().
     */
    SELECTORS.call_once(|| *selectors);

    // ========================================================
    // Load GDT
    // ========================================================

    gdt.load();

    // ========================================================
    // Reload kernel segment registers
    // ========================================================

    unsafe {
        CS::set_reg(
            selectors.kernel_code_selector
        );

        SS::set_reg(
            selectors.kernel_data_selector
        );

        load_tss(
            selectors.tss_selector
        );
    }
}
