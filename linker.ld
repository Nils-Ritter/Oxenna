ENTRY(kmain)

PHDRS
{
    text   PT_LOAD FLAGS(5); /* R + X */
    rodata PT_LOAD FLAGS(4); /* R */
    data   PT_LOAD FLAGS(6); /* R + W */
}

SECTIONS
{
    /*
     * ========================================================
     * KERNEL BASE
     * ========================================================
     *
     * Higher-half kernel virtual address.
     */

    . = 0xffffffff80000000;


    /*
     * ========================================================
     * TEXT
     * ========================================================
     *
     * Executable, read-only kernel code.
     *
     * Page aligned so the page tables can eventually map
     * these pages as:
     *
     *     Supervisor
     *     Read-only
     *     Executable
     */

    .text : ALIGN(4K)
    {
        __text_start = .;

        /*
         * Kernel entry point must come first.
         */
        KEEP(*(.text.entry))

        /*
         * Rest of kernel code.
         */
        *(.text .text.*)

        __text_end = .;

    } :text


    /*
     * ========================================================
     * RODATA
     * ========================================================
     *
     * Read-only, non-executable kernel data.
     */

    . = ALIGN(4K);

    .rodata : ALIGN(4K)
    {
        __rodata_start = .;

        *(.rodata .rodata.*)


        /*
         * ----------------------------------------------------
         * Limine requests
         * ----------------------------------------------------
         */

        KEEP(*(.limine_req_start))
        KEEP(*(.limine_reqs))
        KEEP(*(.limine_req_end))


        __rodata_end = .;

    } :rodata


    /*
     * ========================================================
     * KERNEL TESTS
     * ========================================================
     *
     * The #[test] procedural macro places a Test structure
     * into the .kernel_tests section.
     *
     * These structures are read-only and therefore remain
     * inside the rodata PT_LOAD segment.
     */

    .kernel_tests : ALIGN(8)
    {
        __kernel_tests_start = .;

        KEEP(*(.kernel_tests))

        __kernel_tests_end = .;

    } :rodata


    /*
     * ========================================================
     * DATA
     * ========================================================
     *
     * Writable, non-executable kernel data.
     */

    . = ALIGN(4K);

    .data : ALIGN(4K)
    {
        __data_start = .;

        *(.data .data.*)

        __data_end = .;

    } :data


    /*
     * ========================================================
     * BSS
     * ========================================================
     *
     * Writable, non-executable zero-initialized kernel data.
     */

    . = ALIGN(4K);

    .bss : ALIGN(4K)
    {
        __bss_start = .;

        *(COMMON)

        *(.bss .bss.*)

        __bss_end = .;

    } :data


    /*
     * ========================================================
     * DISCARD
     * ========================================================
     *
     * We don't need unwind information in the kernel.
     */

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}
