use spin::Once;

use x86_64::{
    VirtAddr,
    instructions::{
        segmentation::{CS, SS, Segment},
        tables::load_tss,
    },
    registers::{
        model_specific::{
            Efer,
            EferFlags,
            LStar,
            SFMask,
            Star,
        },
        rflags::RFlags,
    },
    structures::{
        gdt::{
            Descriptor,
            DescriptorFlags,
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
 * from ring 3 through an interrupt or exception.
 *
 * The CPU obtains this address from TSS.RSP0.
 *
 * SYSCALL does NOT use TSS.RSP0 automatically.
 * The syscall entry stub uses the same stack explicitly.
 */
static mut USER_KERNEL_STACK:
    Stack<USER_KERNEL_STACK_SIZE> =
    Stack([0; USER_KERNEL_STACK_SIZE]);

/*
 * Kernel stack used by the SYSCALL entry path.
 *
 * This is separate from the TSS conceptually, although it
 * currently points at the same stack as RSP0.
 *
 * SYSCALL does not perform the privilege-stack switch that
 * IRET/interrupt entry does, so the assembly entry point has
 * to load RSP itself.
 *
 * This is global because Oxenna is currently single-CPU.
 * Once SMP is implemented this must become per-CPU state.
 */
#[unsafe(no_mangle)]
pub static mut SYSCALL_KERNEL_STACK_TOP: u64 = 0;

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

    /*
     * Required by the SYSRET selector layout.
     *
     * The relevant GDT entries are:
     *
     *   user compatibility code
     *   user data
     *   user 64-bit code
     *
     * in exactly that order.
     */
    pub user_compat_code_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector,
    pub user_code_selector: SegmentSelector,

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

        /*
         * SYSCALL doesn't consult TSS.RSP0, so publish the
         * same stack explicitly for the syscall entry stub.
         */
        unsafe {
            SYSCALL_KERNEL_STACK_TOP =
                kernel_stack_end.as_u64();
        }

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
            // GDT index 1
            // DPL = 0
            // ------------------------------------------------

            let kernel_code_selector =
                gdt.append(
                    Descriptor::kernel_code_segment()
                );

            // ------------------------------------------------
            // Kernel data
            // GDT index 2
            // DPL = 0
            // ------------------------------------------------

            let kernel_data_selector =
                gdt.append(
                    Descriptor::kernel_data_segment()
                );

            /*
             * ------------------------------------------------
             * IMPORTANT SYSRET GDT LAYOUT
             * ------------------------------------------------
             *
             * SYSRETQ derives the user selectors from STAR:
             *
             *     user CS = STAR[63:48] + 16
             *     user SS = STAR[63:48] + 8
             *
             * Therefore we need:
             *
             *     GDT[n + 0] = user compatibility code
             *     GDT[n + 1] = user data
             *     GDT[n + 2] = user 64-bit code
             *
             * With the entries below:
             *
             *     index 3 = user 32-bit code
             *     index 4 = user data
             *     index 5 = user 64-bit code
             *
             * The corresponding selectors are:
             *
             *     0x1b = user 32-bit code
             *     0x23 = user data
             *     0x2b = user 64-bit code
             *
             * STAR is configured using 0x2b as the SYSRET CS,
             * and x86_64::registers::model_specific::Star::write()
             * verifies that the corresponding selectors are laid
             * out correctly.
             */

            // ------------------------------------------------
            // User compatibility-mode code
            // GDT index 3
            // DPL = 3
            //
            // This descriptor is required for SYSRET's
            // selector layout, even though Oxenna currently
            // executes only 64-bit userspace.
            // ------------------------------------------------

            let user_compat_code_selector =
                gdt.append(
                    Descriptor::UserSegment(
                        DescriptorFlags::USER_CODE32.bits()
                    )
                );

            // ------------------------------------------------
            // User data
            // GDT index 4
            // DPL = 3
            // ------------------------------------------------

            let user_data_selector =
                gdt.append(
                    Descriptor::user_data_segment()
                );

            // ------------------------------------------------
            // User 64-bit code
            // GDT index 5
            // DPL = 3
            // ------------------------------------------------

            let user_code_selector =
                gdt.append(
                    Descriptor::user_code_segment()
                );

            // ------------------------------------------------
            // TSS
            //
            // The TSS descriptor occupies two GDT entries.
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
                    user_compat_code_selector,
                    user_data_selector,
                    user_code_selector,
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

    // ========================================================
    // Configure SYSCALL / SYSRET
    // ========================================================
    //
    // The x86-64 SYSCALL mechanism uses:
    //
    //   IA32_STAR
    //   IA32_LSTAR
    //   IA32_SFMASK
    //   IA32_EFER.SCE
    //
    // The x86_64 crate provides wrappers for all of these.
    // ========================================================

    unsafe {
        /*
         * Configure STAR.
         *
         * Arguments:
         *
         *   cs_sysret  = user 64-bit CS
         *   ss_sysret  = user data
         *   cs_syscall = kernel CS
         *   ss_syscall = kernel data
         *
         * SYSRETQ will use:
         *
         *   CS = user_code_selector
         *   SS = user_data_selector
         */
        Star::write(
            selectors.user_code_selector,
            selectors.user_data_selector,
            selectors.kernel_code_selector,
            selectors.kernel_data_selector,
        )
        .expect(
            "invalid GDT layout for SYSCALL/SYSRET"
        );

        /*
         * Enable the SYSCALL/SYSRET instructions.
         *
         * This sets EFER.SCE while preserving the other
         * EFER bits.
         */
        Efer::update(|flags| {
            flags.insert(
                EferFlags::SYSTEM_CALL_EXTENSIONS
            );
        });
    }

    /*
     * LSTAR contains the kernel entry point used by SYSCALL
     * while in 64-bit mode.
     *
     * The actual entry point lives in syscall.rs.
     */
    LStar::write(
        crate::syscall::syscall_entry_address()
    );

    /*
     * SFMASK specifies which RFLAGS bits the processor
     * clears when SYSCALL is executed.
     *
     * IF:
     *   Disable interrupts while entering the syscall
     *   path. The kernel can re-enable them once its state
     *   is safely established.
     *
     * DF:
     *   Rust code requires DF=0.
     *
     * TF:
     *   Prevent userspace single-step state from carrying
     *   directly into the kernel syscall handler.
     */
    SFMask::write(
        RFlags::INTERRUPT_FLAG
            | RFlags::DIRECTION_FLAG
            | RFlags::TRAP_FLAG
    );
}
