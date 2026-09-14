use core::{
    fmt,
    mem::MaybeUninit,
    sync::atomic::{
        AtomicBool,
        Ordering,
    },
};

use crate::{
    fb::{self, Color},
    font,
    shell,
};

use crate::test::{test, TestResult};

const CHAR_WIDTH: usize = 8;
const CHAR_HEIGHT: usize = 16;

const DEFAULT_FOREGROUND: Color = Color::WHITE;
const DEFAULT_BACKGROUND: Color = Color::BLACK;

const INPUT_SIZE: usize = 256;

const MAX_COLUMNS: usize = 1920 / CHAR_WIDTH;
const MAX_ROWS: usize = 1080 / CHAR_HEIGHT;

/*
 * Number of logical terminal lines retained in scrollback.
 *
 * This is a ring buffer. Scrolling never copies terminal cell
 * data around.
 */
const SCROLLBACK_ROWS: usize = MAX_ROWS * 6;

// ============================================================
// Cell
// ============================================================

#[derive(Clone, Copy)]
struct Cell {
    character: u8,
    foreground: Color,
    background: Color,
}

impl Cell {
    const EMPTY: Self = Self {
        character: b' ',
        foreground: DEFAULT_FOREGROUND,
        background: DEFAULT_BACKGROUND,
    };
}

/*
 * Fixed-size logical line ring buffer.
 *
 * Logical line:
 *
 *     line
 *
 * maps to:
 *
 *     (line % SCROLLBACK_ROWS) * MAX_COLUMNS
 */
static mut CELLS: [Cell; MAX_COLUMNS * SCROLLBACK_ROWS] =
    [Cell::EMPTY; MAX_COLUMNS * SCROLLBACK_ROWS];

// ============================================================
// Console
// ============================================================

pub struct Console {
    font: font::Font,

    cursor_x: usize,

    /*
     * Absolute logical line containing the cursor.
     */
    cursor_line: usize,

    /*
     * Absolute logical line displayed at viewport row 0.
     */
    view_top: usize,

    columns: usize,
    rows: usize,

    foreground: Color,
    background: Color,

    cells: *mut Cell,

    input: [u8; INPUT_SIZE],
    input_len: usize,
    line_ready: bool,
}

// ============================================================
// Construction
// ============================================================

impl Console {
    pub fn new(
        width: usize,
        height: usize,
    ) -> Self {
        let columns =
            (width / CHAR_WIDTH).min(MAX_COLUMNS);

        let rows =
            (height / CHAR_HEIGHT).min(MAX_ROWS);

        let cells =
            core::ptr::addr_of_mut!(CELLS)
                as *mut Cell;

        /*
         * Reset the entire static terminal storage.
         */
        unsafe {
            let storage =
                core::slice::from_raw_parts_mut(
                    cells,
                    MAX_COLUMNS * SCROLLBACK_ROWS,
                );

            storage.fill(Cell::EMPTY);
        }

        Self {
            font: font::spleen(),

            cursor_x: 0,
            cursor_line: 0,
            view_top: 0,

            columns,
            rows,

            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,

            cells,

            input: [0; INPUT_SIZE],
            input_len: 0,
            line_ready: false,
        }
    }

    // ========================================================
    // Cell access
    // ========================================================

    #[inline(always)]
    fn cell_index(
        line: usize,
        column: usize,
    ) -> usize {
        (line % SCROLLBACK_ROWS)
            * MAX_COLUMNS
            + column
    }

    #[inline(always)]
    fn get_cell(
        &self,
        line: usize,
        column: usize,
    ) -> Cell {
        unsafe {
            *self.cells.add(
                Self::cell_index(
                    line,
                    column,
                ),
            )
        }
    }

    #[inline(always)]
    fn set_cell(
        &mut self,
        line: usize,
        column: usize,
        cell: Cell,
    ) {
        unsafe {
            self.cells
                .add(Self::cell_index(
                    line,
                    column,
                ))
                .write(cell);
        }
    }

    #[inline]
    fn clear_line(
        &mut self,
        line: usize,
    ) {
        unsafe {
            let start =
                self.cells.add(
                    Self::cell_index(
                        line,
                        0,
                    ),
                );

            let row =
                core::slice::from_raw_parts_mut(
                    start,
                    MAX_COLUMNS,
                );

            row.fill(Cell::EMPTY);
        }
    }

    // ========================================================
    // Clear
    // ========================================================

    pub fn clear(&mut self) {
        unsafe {
            let cells =
                core::slice::from_raw_parts_mut(
                    self.cells,
                    MAX_COLUMNS * SCROLLBACK_ROWS,
                );

            cells.fill(Cell::EMPTY);
        }

        self.cursor_x = 0;
        self.cursor_line = 0;
        self.view_top = 0;

        self.input_len = 0;
        self.line_ready = false;

        fb::clear(self.background);
    }

    // ========================================================
    // Colors
    // ========================================================

    pub fn set_foreground(
        &mut self,
        color: Color,
    ) {
        self.foreground = color;
    }

    pub fn set_background(
        &mut self,
        color: Color,
    ) {
        self.background = color;
    }

    pub fn get_background(
        &mut self,
    ) -> Color {
        self.background
    }

    pub fn get_foreground(
        &mut self,
    ) -> Color {
        self.foreground
    }

    // ========================================================
    // Scrollback bookkeeping
    // ========================================================

    #[inline(always)]
    fn live_view_top(&self) -> usize {
        self.cursor_line
            .saturating_sub(
                self.rows.saturating_sub(1),
            )
    }

    #[inline(always)]
    fn oldest_valid_line(&self) -> usize {
        self.cursor_line
            .saturating_sub(
                SCROLLBACK_ROWS
                    .saturating_sub(1),
            )
    }

    #[inline(always)]
    fn is_scrolled(&self) -> bool {
        self.view_top != self.live_view_top()
    }

    #[inline(always)]
    fn cursor_row(&self) -> usize {
        self.cursor_line
            .saturating_sub(
                self.view_top,
            )
    }

    /*
     * Return to the live viewport.
     *
     * This only performs a full viewport render if the user
     * actually was viewing history.
     */
    fn follow_bottom(&mut self) {
        if !self.is_scrolled() {
            return;
        }

        self.view_top =
            self.live_view_top();

        self.render();
    }

    // ========================================================
    // Incremental history scrolling
    // ========================================================

    /*
     * Scroll toward older history.
     *
     * IMPORTANT:
     *
     * The framebuffer already contains the current viewport.
     * We therefore move the existing framebuffer pixels DOWN,
     * exposing new rows at the TOP.
     *
     * Only those newly exposed rows are rendered.
     */
    pub fn scroll_up(
        &mut self,
        lines: usize,
    ) {
        if lines == 0
            || self.rows == 0
        {
            return;
        }

        let live_top =
            self.live_view_top();

        let oldest =
            self.oldest_valid_line();

        let current_offset =
            live_top.saturating_sub(
                self.view_top,
            );

        let max_offset =
            live_top.saturating_sub(
                oldest,
            );

        let new_offset =
            current_offset
                .saturating_add(lines)
                .min(max_offset);

        let actual_lines =
            new_offset.saturating_sub(
                current_offset,
            );

        /*
         * Already at the oldest available history.
         *
         * Do absolutely nothing. In particular, don't present
         * the framebuffer again.
         */
        if actual_lines == 0 {
            return;
        }

        self.view_top =
            live_top.saturating_sub(
                new_offset,
            );

        let visible_lines =
            actual_lines.min(
                self.rows,
            );

        /*
         * Move existing framebuffer contents DOWN.
         *
         * This exposes empty/new space at the TOP.
         */
        fb::scroll_down(
            visible_lines * CHAR_HEIGHT,
            self.background,
        );

        /*
         * Only paint the newly exposed rows.
         */
        for row in 0..visible_lines {
            self.render_row(row);
        }
    }

    /*
     * Scroll toward the live cursor.
     *
     * Existing framebuffer pixels move UP, exposing new rows
     * at the BOTTOM.
     */
    pub fn scroll_down(
        &mut self,
        lines: usize,
    ) {
        if lines == 0
            || self.rows == 0
        {
            return;
        }

        let live_top =
            self.live_view_top();

        let old_view =
            self.view_top;

        let new_view =
            self.view_top
                .saturating_add(lines)
                .min(live_top);

        let actual_lines =
            new_view.saturating_sub(
                old_view,
            );

        /*
         * Already at the live view.
         */
        if actual_lines == 0 {
            return;
        }

        self.view_top =
            new_view;

        let visible_lines =
            actual_lines.min(
                self.rows,
            );

        /*
         * Move framebuffer contents UP.
         *
         * New space appears at the bottom.
         */
        fb::scroll_up(
            visible_lines * CHAR_HEIGHT,
            self.background,
        );

        let first_row =
            self.rows.saturating_sub(
                visible_lines,
            );

        /*
         * Only paint the newly exposed bottom rows.
         */
        for row in first_row..self.rows {
            self.render_row(row);
        }
    }

    // ========================================================
    // Rendering
    // ========================================================

    #[inline(always)]
    fn render_cell(
        &self,
        row: usize,
        column: usize,
    ) {
        if row >= self.rows
            || column >= self.columns
        {
            return;
        }

        let line =
            self.view_top + row;

        let cell =
            self.get_cell(
                line,
                column,
            );

        let x =
            column * CHAR_WIDTH;

        let y =
            row * CHAR_HEIGHT;

        /*
         * draw_rect establishes the complete background
         * because the font only paints foreground pixels.
         */
        fb::draw_rect(
            x,
            y,
            CHAR_WIDTH,
            CHAR_HEIGHT,
            cell.background,
        );

        if cell.character != b' ' {
            self.font.draw_char(
                x,
                y,
                cell.character,
                cell.foreground,
            );
        }
    }

    #[inline]
    fn render_row(
        &self,
        row: usize,
    ) {
        if row >= self.rows {
            return;
        }

        let line =
            self.view_top + row;

        let y =
            row * CHAR_HEIGHT;

        for column in 0..self.columns {
            let cell =
                self.get_cell(
                    line,
                    column,
                );

            let x =
                column * CHAR_WIDTH;

            fb::draw_rect(
                x,
                y,
                CHAR_WIDTH,
                CHAR_HEIGHT,
                cell.background,
            );

            if cell.character != b' ' {
                self.font.draw_char(
                    x,
                    y,
                    cell.character,
                    cell.foreground,
                );
            }
        }
    }

    /*
     * Full viewport render.
     *
     * We intentionally don't call fb::clear() here because
     * every terminal cell establishes its own background.
     */
    pub fn render(&self) {
        for row in 0..self.rows {
            self.render_row(row);
        }
    }

    // ========================================================
    // Newline
    // ========================================================

    fn newline(&mut self) {
        self.cursor_x = 0;

        if self.rows == 0 {
            return;
        }

        /*
         * Still room below the cursor.
         */
        if self.cursor_line + 1 < self.rows {
            self.cursor_line += 1;
            return;
        }

        /*
         * Terminal output reached the bottom.
         *
         * The framebuffer performs the expensive pixel movement.
         * The logical terminal buffer does not move.
         */
        fb::scroll_up(
            CHAR_HEIGHT,
            self.background,
        );

        self.cursor_line += 1;

        self.view_top =
            self.live_view_top();

        /*
         * Reused ring-buffer slot may contain stale history.
         */
        self.clear_line(
            self.cursor_line,
        );

        /*
         * Only the newly exposed bottom row needs painting.
         */
        self.render_row(
            self.rows - 1,
        );
    }

    // ========================================================
    // Character output
    // ========================================================

    #[inline]
    fn put_char(
        &mut self,
        character: u8,
    ) {
        /*
         * If the user was viewing history, normal output returns
         * to the live terminal.
         */
        self.follow_bottom();

        match character {
            b'\n' => {
                self.newline();
            }

            b'\r' => {
                self.cursor_x = 0;
            }

            b'\t' => {
                const TAB_SIZE: usize = 4;

                let next_tab =
                    ((self.cursor_x / TAB_SIZE) + 1)
                        * TAB_SIZE;

                if next_tab >= self.columns {
                    self.newline();
                } else {
                    self.cursor_x =
                        next_tab;
                }
            }

            8 => {
                self.backspace();
            }

            0x20..=0x7e => {
                self.write_cell(
                    character,
                );
            }

            _ => {}
        }
    }

    // ========================================================
    // Write cell
    // ========================================================

    #[inline]
    fn write_cell(
        &mut self,
        character: u8,
    ) {
        if self.columns == 0
            || self.rows == 0
        {
            return;
        }

        if self.cursor_x >= self.columns {
            self.newline();
        }

        let line =
            self.cursor_line;

        let column =
            self.cursor_x;

        self.set_cell(
            line,
            column,
            Cell {
                character,
                foreground: self.foreground,
                background: self.background,
            },
        );

        let row =
            line.saturating_sub(
                self.view_top,
            );

        /*
         * Normal output changes exactly one visible cell.
         */
        self.render_cell(
            row,
            column,
        );

        self.cursor_x += 1;

        if self.cursor_x >= self.columns {
            self.newline();
        }
    }

    // ========================================================
    // Backspace
    // ========================================================

    fn backspace(&mut self) {
        if self.cursor_x == 0 {
            if self.cursor_line == 0 {
                return;
            }

            self.cursor_line -= 1;

            self.view_top =
                self.live_view_top();

            self.cursor_x =
                self.columns
                    .saturating_sub(1);
        } else {
            self.cursor_x -= 1;
        }

        self.set_cell(
            self.cursor_line,
            self.cursor_x,
            Cell::EMPTY,
        );

        let row =
            self.cursor_row();

        self.render_cell(
            row,
            self.cursor_x,
        );
    }

    // ========================================================
    // String output
    // ========================================================

    pub fn write_str(
        &mut self,
        text: &str,
    ) {
        for byte in text.bytes() {
            self.put_char(byte);
        }
    }

    // ========================================================
    // Keyboard
    // ========================================================

    pub fn receive_key(
        &mut self,
        key: char,
    ) {
        match key {
            '\n' | '\r' => {
                self.put_char(b'\n');
                self.line_ready = true;
            }

            '\u{8}' | '\u{7f}' => {
                if self.input_len > 0
                    && self.cursor_x > 2
                {
                    self.input_len -= 1;
                    self.put_char(8);
                }
            }

            character
                if character.is_ascii()
                    && !character.is_ascii_control() =>
            {
                if self.input_len < INPUT_SIZE {
                    self.input[
                        self.input_len
                    ] = character as u8;

                    self.input_len += 1;

                    self.put_char(
                        character as u8,
                    );
                }
            }

            _ => {}
        }
    }

    // ========================================================
    // Read line
    // ========================================================

    pub fn read_line(
        &mut self,
        buffer: &mut [u8],
    ) -> Option<usize> {
        if !self.line_ready {
            return None;
        }

        let length =
            self.input_len.min(
                buffer.len(),
            );

        buffer[..length]
            .copy_from_slice(
                &self.input[..length],
            );

        self.input_len = 0;
        self.line_ready = false;

        Some(length)
    }
}

// ============================================================
// fmt::Write
// ============================================================

impl fmt::Write for Console {
    fn write_str(
        &mut self,
        text: &str,
    ) -> fmt::Result {
        self.write_str(text);
        Ok(())
    }
}

// ============================================================
// Global console
// ============================================================

static mut CONSOLE: MaybeUninit<Console> =
    MaybeUninit::uninit();

static mut CONSOLE_INITIALIZED: bool =
    false;

// ============================================================
// Initialization
// ============================================================

pub fn init() {
    let width =
        fb::width();

    let height =
        fb::height();

    let console =
        Console::new(
            width,
            height,
        );

    unsafe {
        core::ptr::addr_of_mut!(
            CONSOLE
        )
        .write(
            MaybeUninit::new(
                console,
            ),
        );

        core::ptr::addr_of_mut!(
            CONSOLE_INITIALIZED
        )
        .write(true);
    }

    with_console(|console| {
        console.clear();
    });

    with_console(|console| {
        console.put_char(b'$');
        console.put_char(b' ');
    });

    fb::present();
}

// ============================================================
// with_console
// ============================================================

#[inline]
pub fn with_console<F, R>(
    f: F,
) -> R
where
    F: FnOnce(&mut Console) -> R,
{
    unsafe {
        if !CONSOLE_INITIALIZED {
            panic!(
                "console used before console::init()"
            );
        }

        let console =
            core::ptr::addr_of_mut!(
                CONSOLE
            )
            .cast::<Console>()
            .as_mut()
            .unwrap_unchecked();

        f(console)
    }
}

// ============================================================
// Formatted output
// ============================================================

pub fn write_fmt(
    args: fmt::Arguments<'_>,
) {
    use core::fmt::Write;

    with_console(|console| {
        let _ =
            console.write_fmt(args);
    });

    fb::present();
}

pub fn write_fmt_color(
    color: Color,
    args: fmt::Arguments<'_>,
) {
    use core::fmt::Write;

    with_console(|console| {
        let old_color =
            console.foreground;

        console.foreground =
            color;

        let _ =
            console.write_fmt(args);

        console.foreground =
            old_color;
    });

    fb::present();
}

// ============================================================
// Scrolling
// ============================================================

pub fn scroll_up(
    lines: usize,
) {
    with_console(|console| {
        console.scroll_up(lines);
    });

    fb::present();
}

pub fn scroll_down(
    lines: usize,
) {
    with_console(|console| {
        console.scroll_down(lines);
    });

    fb::present();
}

// ============================================================
// Keyboard
// ============================================================

pub fn receive_key(
    key: char,
) {
    if key == '\n'
        || key == '\r'
    {
        let mut command_buffer =
            [0u8; INPUT_SIZE];

        let length =
            with_console(|console| {
                console.receive_key(key);

                console.read_line(
                    &mut command_buffer,
                )
            });

        if let Some(length) = length {
            if let Ok(command) =
                core::str::from_utf8(
                    &command_buffer[..length],
                )
            {
                shell::execute(command);
            }

            with_console(|console| {
                console.put_char(b'$');
                console.put_char(b' ');
            });
        }

        fb::present();

        return;
    }

    with_console(|console| {
        console.receive_key(key);
    });

    fb::present();
}

// ============================================================
// Read line
// ============================================================

#[allow(unused)]
pub fn read_line(
    buffer: &mut [u8],
) -> Option<usize> {
    with_console(|console| {
        console.read_line(buffer)
    })
}

// ============================================================
// Clear
// ============================================================

pub fn clear() {
    with_console(|console| {
        console.clear();
    });

    fb::present();
}

// ============================================================
// Serial mirroring
// ============================================================

pub static SERIAL_MIRROR: AtomicBool =
    AtomicBool::new(false);

pub fn set_serial_mirror(
    enabled: bool,
) {
    SERIAL_MIRROR.store(
        enabled,
        Ordering::Relaxed,
    );
}

#[inline(always)]
pub fn serial_mirror_enabled() -> bool {
    SERIAL_MIRROR.load(
        Ordering::Relaxed,
    )
}

#[allow(unused)]
pub fn write_fmt_mirrored(
    args: core::fmt::Arguments<'_>,
) {
    write_fmt(args);

    if serial_mirror_enabled() {
        crate::serial::write_fmt(args);
    }
}

#[allow(unused)]
pub fn write_fmt_color_mirrored(
    color: crate::fb::Color,
    args: core::fmt::Arguments<'_>,
) {
    write_fmt_color(
        color,
        args,
    );

    if serial_mirror_enabled() {
        crate::serial::write_fmt(args);
    }
}

// ============================================================
// Printing macros
// ============================================================

#[macro_export]
macro_rules! console_print {
    ($($arg:tt)*) => {{
        let args =
            core::format_args!($($arg)*);

        $crate::console::write_fmt(
            args
        );

        if $crate::console::serial_mirror_enabled() {
            $crate::serial_print!(
                $($arg)*
            );
        }
    }};
}

#[macro_export]
macro_rules! console_println {
    () => {{
        $crate::console::write_fmt(
            core::format_args!("\n")
        );

        if $crate::console::serial_mirror_enabled() {
            $crate::serial_println!();
        }
    }};

    ($($arg:tt)*) => {{
        let args =
            core::format_args!(
                "{}\n",
                core::format_args!($($arg)*)
            );

        $crate::console::write_fmt(
            args
        );

        if $crate::console::serial_mirror_enabled() {
            $crate::serial_println!(
                $($arg)*
            );
        }
    }};
}

#[macro_export]
macro_rules! console_print_color {
    ($color:expr, $($arg:tt)*) => {{
        let args =
            core::format_args!($($arg)*);

        $crate::console::write_fmt_color(
            $color,
            args
        );

        if $crate::console::serial_mirror_enabled() {
            $crate::serial_print!(
                $($arg)*
            );
        }
    }};
}

#[macro_export]
macro_rules! console_println_color {
    ($color:expr) => {{
        $crate::console::write_fmt_color(
            $color,
            core::format_args!("\n")
        );

        if $crate::console::serial_mirror_enabled() {
            $crate::serial_println!();
        }
    }};

    ($color:expr, $($arg:tt)*) => {{
        let args =
            core::format_args!(
                "{}\n",
                core::format_args!($($arg)*)
            );

        $crate::console::write_fmt_color(
            $color,
            args
        );

        if $crate::console::serial_mirror_enabled() {
            $crate::serial_println!(
                $($arg)*
            );
        }
    }};
}

// ============================================================
// Tests
// ============================================================

#[cfg(feature = "test")]
#[allow(unused_imports)]
mod tests {
    use crate::test::{
        test,
        TestResult,
    };

    use super::{
        Cell,
        Console,
        CHAR_HEIGHT,
        CHAR_WIDTH,
        DEFAULT_BACKGROUND,
        DEFAULT_FOREGROUND,
        INPUT_SIZE,
        MAX_COLUMNS,
        MAX_ROWS,
    };

    fn pass() -> TestResult {
        TestResult::Pass
    }

    // ========================================================
    // Helpers
    // ========================================================

    /*
     * Access a logical terminal line.
     *
     * `line` is an absolute logical line number, not a visible
     * viewport row.
     */
    fn cell(
        console: &Console,
        line: usize,
        column: usize,
    ) -> Cell {
        console.get_cell(
            line,
            column,
        )
    }

    /*
     * Access a currently visible viewport row.
     */
    fn visible_cell(
        console: &Console,
        row: usize,
        column: usize,
    ) -> Cell {
        console.get_cell(
            console.view_top + row,
            column,
        )
    }

    fn assert_cell_char(
        console: &Console,
        line: usize,
        column: usize,
        expected: u8,
    ) -> TestResult {
        if cell(
            console,
            line,
            column,
        ).character != expected {
            return TestResult::Fail(
                "terminal cell contains incorrect character",
            );
        }

        TestResult::Pass
    }

    fn assert_visible_cell_char(
        console: &Console,
        row: usize,
        column: usize,
        expected: u8,
    ) -> TestResult {
        if visible_cell(
            console,
            row,
            column,
        ).character != expected {
            return TestResult::Fail(
                "visible terminal cell contains incorrect character",
            );
        }

        TestResult::Pass
    }

    // ========================================================
    // Construction
    // ========================================================

    #[test]
    fn console_initializes_cursor_and_input_state()
        -> TestResult
    {
        let console =
            Console::new(
                800,
                480,
            );

        if console.cursor_x != 0 {
            return TestResult::Fail(
                "cursor_x is not initialized to zero",
            );
        }

        if console.cursor_line != 0 {
            return TestResult::Fail(
                "cursor_line is not initialized to zero",
            );
        }

        if console.view_top != 0 {
            return TestResult::Fail(
                "view_top is not initialized to zero",
            );
        }

        if console.columns != 100 {
            return TestResult::Fail(
                "console column count is incorrect",
            );
        }

        if console.rows != 30 {
            return TestResult::Fail(
                "console row count is incorrect",
            );
        }

        if console.input_len != 0 {
            return TestResult::Fail(
                "input buffer is not empty",
            );
        }

        if console.line_ready {
            return TestResult::Fail(
                "line_ready is initially set",
            );
        }

        pass()
    }

    #[test]
    fn console_calculates_columns_correctly()
        -> TestResult
    {
        let console =
            Console::new(
                801,
                480,
            );

        if console.columns != 100 {
            return TestResult::Fail(
                "console did not floor width correctly",
            );
        }

        pass()
    }

    #[test]
    fn console_calculates_rows_correctly()
        -> TestResult
    {
        let console =
            Console::new(
                800,
                479,
            );

        if console.rows != 29 {
            return TestResult::Fail(
                "console did not floor height correctly",
            );
        }

        pass()
    }

    #[test]
    fn console_handles_zero_dimensions()
        -> TestResult
    {
        let console =
            Console::new(
                0,
                0,
            );

        if console.columns != 0 {
            return TestResult::Fail(
                "zero-width console has columns",
            );
        }

        if console.rows != 0 {
            return TestResult::Fail(
                "zero-height console has rows",
            );
        }

        pass()
    }

    // ========================================================
    // Character output
    // ========================================================

    #[test]
    fn console_writes_characters()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abc",
        );

        if console.cursor_x != 3 {
            return TestResult::Fail(
                "writing characters does not advance cursor",
            );
        }

        if console.cursor_line != 0 {
            return TestResult::Fail(
                "writing characters changed cursor line",
            );
        }

        if assert_cell_char(
            &console,
            0,
            0,
            b'a',
        ).passed() == false {
            return TestResult::Fail(
                "first cell does not contain a",
            );
        }

        if assert_cell_char(
            &console,
            0,
            1,
            b'b',
        ).passed() == false {
            return TestResult::Fail(
                "second cell does not contain b",
            );
        }

        if assert_cell_char(
            &console,
            0,
            2,
            b'c',
        ).passed() == false {
            return TestResult::Fail(
                "third cell does not contain c",
            );
        }

        pass()
    }

    #[test]
    fn console_writes_multiple_lines()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                64,
            );

        console.write_str(
            "abc\ndef",
        );

        if cell(
            &console,
            0,
            0,
        ).character != b'a' {
            return TestResult::Fail(
                "first line is incorrect",
            );
        }

        if cell(
            &console,
            1,
            0,
        ).character != b'd' {
            return TestResult::Fail(
                "second line is incorrect",
            );
        }

        if console.cursor_line != 1 {
            return TestResult::Fail(
                "cursor line after multiline output is incorrect",
            );
        }

        pass()
    }

    // ========================================================
    // Newline
    // ========================================================

    #[test]
    fn console_newline_moves_to_next_row()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abc\ndef",
        );

        if console.cursor_x != 3 {
            return TestResult::Fail(
                "cursor column after newline is incorrect",
            );
        }

        if console.cursor_line != 1 {
            return TestResult::Fail(
                "newline did not advance logical line",
            );
        }

        pass()
    }

    #[test]
    fn console_multiple_newlines()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                64,
            );

        console.write_str(
            "\n\n\n",
        );

        if console.cursor_x != 0 {
            return TestResult::Fail(
                "multiple newlines changed column",
            );
        }

        if console.cursor_line != 3 {
            return TestResult::Fail(
                "multiple newlines produced wrong logical line",
            );
        }

        pass()
    }

    // ========================================================
    // Carriage return
    // ========================================================

    #[test]
    fn console_carriage_return_resets_column()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abcdef\rxy",
        );

        if console.cursor_x != 2 {
            return TestResult::Fail(
                "carriage return did not reset column",
            );
        }

        if console.cursor_line != 0 {
            return TestResult::Fail(
                "carriage return changed logical line",
            );
        }

        pass()
    }

    #[test]
    fn console_carriage_return_overwrites_cells()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abcdef\rxy",
        );

        if cell(
            &console,
            0,
            0,
        ).character != b'x' {
            return TestResult::Fail(
                "carriage return did not overwrite first cell",
            );
        }

        if cell(
            &console,
            0,
            1,
        ).character != b'y' {
            return TestResult::Fail(
                "carriage return did not overwrite second cell",
            );
        }

        pass()
    }

    // ========================================================
    // Tabs
    // ========================================================

    #[test]
    fn console_tab_advances_to_next_tab_stop()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "a\t",
        );

        if console.cursor_x != 4 {
            return TestResult::Fail(
                "tab did not advance to next tab stop",
            );
        }

        console.write_str(
            "b\t",
        );

        if console.cursor_x != 8 {
            return TestResult::Fail(
                "second tab did not advance correctly",
            );
        }

        pass()
    }

    #[test]
    fn console_tab_from_tab_stop_advances()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abcd\t",
        );

        if console.cursor_x != 8 {
            return TestResult::Fail(
                "tab from existing stop is incorrect",
            );
        }

        pass()
    }

    #[test]
    fn console_tab_at_right_edge_wraps()
        -> TestResult
    {
        let mut console =
            Console::new(
                32,
                32,
            );

        console.write_str(
            "abc\t",
        );

        if console.cursor_line != 1 {
            return TestResult::Fail(
                "tab did not wrap at right edge",
            );
        }

        if console.cursor_x != 0 {
            return TestResult::Fail(
                "tab wrap did not reset column",
            );
        }

        pass()
    }

    // ========================================================
    // Wrapping
    // ========================================================

    #[test]
    fn console_wraps_at_right_edge()
        -> TestResult
    {
        let mut console =
            Console::new(
                16,
                32,
            );

        console.write_str(
            "ab",
        );

        if console.cursor_x != 0 {
            return TestResult::Fail(
                "console did not wrap at right edge",
            );
        }

        if console.cursor_line != 1 {
            return TestResult::Fail(
                "console did not advance logical line after wrapping",
            );
        }

        pass()
    }

    #[test]
    fn console_wrap_preserves_characters()
        -> TestResult
    {
        let mut console =
            Console::new(
                16,
                32,
            );

        console.write_str(
            "ab",
        );

        if cell(
            &console,
            0,
            0,
        ).character != b'a' {
            return TestResult::Fail(
                "wrapped line lost first character",
            );
        }

        if cell(
            &console,
            0,
            1,
        ).character != b'b' {
            return TestResult::Fail(
                "wrapped line lost second character",
            );
        }

        pass()
    }

    // ========================================================
    // Backspace
    // ========================================================

    #[test]
    fn console_backspace_moves_cursor_back()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abc",
        );

        console.write_str(
            "\u{8}",
        );

        if console.cursor_x != 2 {
            return TestResult::Fail(
                "backspace did not move cursor backwards",
            );
        }

        if console.cursor_line != 0 {
            return TestResult::Fail(
                "backspace changed logical line",
            );
        }

        pass()
    }

    #[test]
    fn console_backspace_clears_cell()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "abc",
        );

        console.write_str(
            "\u{8}",
        );

        if cell(
            &console,
            0,
            2,
        ).character != b' ' {
            return TestResult::Fail(
                "backspace did not clear cell",
            );
        }

        pass()
    }

    #[test]
    fn console_backspace_at_origin_is_safe()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.write_str(
            "\u{8}",
        );

        if console.cursor_x != 0
            || console.cursor_line != 0
        {
            return TestResult::Fail(
                "backspace at origin moved cursor",
            );
        }

        pass()
    }

    // ========================================================
    // Clear
    // ========================================================

    #[test]
    fn console_clear_resets_cursor()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.write_str(
            "hello\nworld",
        );

        console.clear();

        if console.cursor_x != 0
            || console.cursor_line != 0
            || console.view_top != 0
        {
            return TestResult::Fail(
                "clear did not reset cursor/view",
            );
        }

        pass()
    }

    #[test]
    fn console_clear_resets_input()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.input[..5]
            .copy_from_slice(
                b"hello",
            );

        console.input_len = 5;
        console.line_ready = true;

        console.clear();

        if console.input_len != 0 {
            return TestResult::Fail(
                "clear did not reset input length",
            );
        }

        if console.line_ready {
            return TestResult::Fail(
                "clear did not reset line_ready",
            );
        }

        pass()
    }

    #[test]
    fn console_clear_resets_cells()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.write_str(
            "hello world",
        );

        console.clear();

        for row in 0..console.rows {
            for column in 0..console.columns {
                if cell(
                    &console,
                    row,
                    column,
                ).character != b' ' {
                    return TestResult::Fail(
                        "clear left non-empty terminal cell",
                    );
                }
            }
        }

        pass()
    }

    // ========================================================
    // Scrolling
    // ========================================================

    #[test]
    fn console_scroll_moves_logical_lines_up()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                3 * CHAR_HEIGHT,
            );

        /*
         * Three terminal rows:
         *
         *     row 0 = A
         *     row 1 = B
         *     row 2 = C
         */

        console.write_str(
            "A\nB\nC",
        );

        /*
         * Force a newline at the bottom.
         *
         * Logical lines:
         *
         *     line 0 = A
         *     line 1 = B
         *     line 2 = C
         *     line 3 = empty
         *
         * Viewport:
         *
         *     row 0 = B
         *     row 1 = C
         *     row 2 = empty
         */

        console.write_str(
            "\n",
        );

        if console.view_top != 1 {
            return TestResult::Fail(
                "viewport did not move to the correct line",
            );
        }

        if visible_cell(
            &console,
            0,
            0,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll did not move second line to first visible row",
            );
        }

        if visible_cell(
            &console,
            1,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "scroll did not move third line to second visible row",
            );
        }

        if visible_cell(
            &console,
            2,
            0,
        ).character != b' ' {
            return TestResult::Fail(
                "scroll did not clear new bottom row",
            );
        }

        if console.cursor_line != 3 {
            return TestResult::Fail(
                "cursor logical line is incorrect after scroll",
            );
        }

        if console.cursor_row() != 2 {
            return TestResult::Fail(
                "cursor is not on bottom visible row after scroll",
            );
        }

        if console.cursor_x != 0 {
            return TestResult::Fail(
                "cursor column is incorrect after scroll",
            );
        }

        pass()
    }

    #[test]
    fn console_scroll_preserves_entire_lines()
        -> TestResult
    {
        let mut console =
            Console::new(
                40,
                3 * CHAR_HEIGHT,
            );

        console.write_str(
            "AAAA\nBBBB\nCCCC\n",
        );

        /*
         * Viewport:
         *
         *     row 0 = BBBB
         *     row 1 = CCCC
         *     row 2 = empty
         */

        if visible_cell(
            &console,
            0,
            0,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll corrupted first character",
            );
        }

        if visible_cell(
            &console,
            0,
            3,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll corrupted first line",
            );
        }

        if visible_cell(
            &console,
            1,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "scroll corrupted second line",
            );
        }

        if visible_cell(
            &console,
            1,
            3,
        ).character != b'C' {
            return TestResult::Fail(
                "scroll corrupted second line end",
            );
        }

        pass()
    }

    #[test]
    fn console_manual_scrolling_changes_viewport_only()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                3 * CHAR_HEIGHT,
            );

        console.write_str(
            "A\nB\nC\nD\nE",
        );

        /*
         * Logical lines:
         *
         *     0 = A
         *     1 = B
         *     2 = C
         *     3 = D
         *     4 = E
         *
         * Live viewport:
         *
         *     C
         *     D
         *     E
         */

        if console.view_top != 2 {
            return TestResult::Fail(
                "initial live viewport is incorrect",
            );
        }

        if console.cursor_line != 4 {
            return TestResult::Fail(
                "initial cursor logical line is incorrect",
            );
        }

        console.scroll_up(
            1,
        );

        /*
         * Viewport is now:
         *
         *     B
         *     C
         *     D
         */

        if console.view_top != 1 {
            return TestResult::Fail(
                "scroll_up moved to incorrect viewport",
            );
        }

        if visible_cell(
            &console,
            0,
            0,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll_up exposed incorrect line",
            );
        }

        if console.cursor_line != 4 {
            return TestResult::Fail(
                "scroll_up changed cursor logical line",
            );
        }

        console.scroll_down(
            1,
        );

        if console.view_top != 2 {
            return TestResult::Fail(
                "scroll_down did not return to live viewport",
            );
        }

        if visible_cell(
            &console,
            0,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "scroll_down exposed incorrect line",
            );
        }

        pass()
    }

    #[test]
    fn console_scroll_up_clamps_at_oldest_line()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                3 * CHAR_HEIGHT,
            );

        console.write_str(
            "A\nB\nC\nD\nE",
        );

        console.scroll_up(
            usize::MAX,
        );

        if console.view_top != 0 {
            return TestResult::Fail(
                "scroll_up did not clamp at oldest line",
            );
        }

        if visible_cell(
            &console,
            0,
            0,
        ).character != b'A' {
            return TestResult::Fail(
                "oldest visible line is incorrect",
            );
        }

        if visible_cell(
            &console,
            1,
            0,
        ).character != b'B' {
            return TestResult::Fail(
                "second visible history line is incorrect",
            );
        }

        if visible_cell(
            &console,
            2,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "third visible history line is incorrect",
            );
        }

        if console.cursor_line != 4 {
            return TestResult::Fail(
                "scroll_up changed cursor logical line",
            );
        }

        pass()
    }

    #[test]
    fn console_scroll_down_clamps_at_live_view()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                3 * CHAR_HEIGHT,
            );

        console.write_str(
            "A\nB\nC\nD\nE",
        );

        console.scroll_up(
            usize::MAX,
        );

        console.scroll_down(
            usize::MAX,
        );

        if console.view_top != 2 {
            return TestResult::Fail(
                "scroll_down did not clamp at live viewport",
            );
        }

        if visible_cell(
            &console,
            0,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "live viewport has incorrect first line",
            );
        }

        if visible_cell(
            &console,
            1,
            0,
        ).character != b'D' {
            return TestResult::Fail(
                "live viewport has incorrect second line",
            );
        }

        if visible_cell(
            &console,
            2,
            0,
        ).character != b'E' {
            return TestResult::Fail(
                "live viewport has incorrect third line",
            );
        }

        pass()
    }

    // ========================================================
    // Colors
    // ========================================================

    #[test]
    fn console_default_colors_are_correct()
        -> TestResult
    {
        let console =
            Console::new(
                800,
                480,
            );

        if console.foreground
            != DEFAULT_FOREGROUND {
            return TestResult::Fail(
                "default foreground color is incorrect",
            );
        }

        if console.background
            != DEFAULT_BACKGROUND {
            return TestResult::Fail(
                "default background color is incorrect",
            );
        }

        pass()
    }

    #[test]
    fn console_set_foreground_changes_color()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.set_foreground(
            crate::fb::Color::RED,
        );

        if console.foreground
            != crate::fb::Color::RED {
            return TestResult::Fail(
                "set_foreground failed",
            );
        }

        pass()
    }

    #[test]
    fn console_set_background_changes_color()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.set_background(
            crate::fb::Color::BLUE,
        );

        if console.background
            != crate::fb::Color::BLUE {
            return TestResult::Fail(
                "set_background failed",
            );
        }

        pass()
    }

    #[test]
    fn console_cells_store_colors()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.set_foreground(
            crate::fb::Color::RED,
        );

        console.set_background(
            crate::fb::Color::BLUE,
        );

        console.write_str(
            "X",
        );

        let cell =
            cell(
                &console,
                0,
                0,
            );

        if cell.foreground
            != crate::fb::Color::RED {
            return TestResult::Fail(
                "cell did not store foreground color",
            );
        }

        if cell.background
            != crate::fb::Color::BLUE {
            return TestResult::Fail(
                "cell did not store background color",
            );
        }

        pass()
    }

    // ========================================================
    // Input
    // ========================================================

    #[test]
    fn console_read_line_returns_none_until_ready()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        let mut buffer =
            [0u8; 16];

        if console
            .read_line(
                &mut buffer,
            )
            .is_some() {
            return TestResult::Fail(
                "read_line returned line before Enter",
            );
        }

        pass()
    }

    #[test]
    fn console_receive_key_stores_input()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.receive_key(
            'h',
        );

        console.receive_key(
            'i',
        );

        if console.input_len != 2 {
            return TestResult::Fail(
                "receive_key stored incorrect input length",
            );
        }

        if console.input[0] != b'h'
            || console.input[1] != b'i' {
            return TestResult::Fail(
                "receive_key stored incorrect characters",
            );
        }

        pass()
    }

    #[test]
    fn console_enter_marks_line_ready()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.receive_key(
            'h',
        );

        console.receive_key(
            'i',
        );

        console.receive_key(
            '\n',
        );

        if !console.line_ready {
            return TestResult::Fail(
                "Enter did not mark line ready",
            );
        }

        pass()
    }

    #[test]
    fn console_read_line_consumes_input()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.input[..5]
            .copy_from_slice(
                b"hello",
            );

        console.input_len = 5;
        console.line_ready = true;

        let mut buffer =
            [0u8; 16];

        let length =
            match console.read_line(
                &mut buffer,
            ) {
                Some(length) => length,

                None => {
                    return TestResult::Fail(
                        "read_line did not return ready line",
                    );
                }
            };

        if length != 5 {
            return TestResult::Fail(
                "read_line returned incorrect length",
            );
        }

        if &buffer[..5]
            != b"hello" {
            return TestResult::Fail(
                "read_line returned incorrect contents",
            );
        }

        if console.input_len != 0 {
            return TestResult::Fail(
                "read_line did not consume input",
            );
        }

        if console.line_ready {
            return TestResult::Fail(
                "read_line did not clear line_ready",
            );
        }

        pass()
    }

    #[test]
    fn console_read_line_truncates_destination()
        -> TestResult
    {
        let mut console =
            Console::new(
                80,
                32,
            );

        console.input[..6]
            .copy_from_slice(
                b"abcdef",
            );

        console.input_len = 6;
        console.line_ready = true;

        let mut buffer =
            [0u8; 3];

        let length =
            match console.read_line(
                &mut buffer,
            ) {
                Some(length) => length,

                None => {
                    return TestResult::Fail(
                        "read_line did not return ready line",
                    );
                }
            };

        if length != 3 {
            return TestResult::Fail(
                "read_line did not truncate",
            );
        }

        if &buffer != b"abc" {
            return TestResult::Fail(
                "truncated line has incorrect contents",
            );
        }

        pass()
    }

    // ========================================================
    // Input limits
    // ========================================================

    #[test]
    fn console_input_cannot_exceed_input_size()
        -> TestResult
    {
        let mut console =
            Console::new(
                1920,
                1080,
            );

        for _ in 0..(INPUT_SIZE + 100) {
            console.receive_key(
                'a',
            );
        }

        if console.input_len
            != INPUT_SIZE {
            return TestResult::Fail(
                "input buffer exceeded INPUT_SIZE",
            );
        }

        pass()
    }

    // ========================================================
    // Control characters
    // ========================================================

    #[test]
    fn console_ignores_unknown_control_characters()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        console.write_str(
            "abc",
        );

        let x =
            console.cursor_x;

        let line =
            console.cursor_line;

        console.put_char(
            0x01,
        );

        console.put_char(
            0x02,
        );

        console.put_char(
            0x7f,
        );

        if console.cursor_x != x {
            return TestResult::Fail(
                "unknown control character moved cursor",
            );
        }

        if console.cursor_line != line {
            return TestResult::Fail(
                "unknown control character moved logical line",
            );
        }

        pass()
    }

    // ========================================================
    // Long output
    // ========================================================

    #[test]
    fn console_handles_long_output()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        /*
         * Deliberately produce much more output than the
         * visible screen can hold.
         */

        for _ in 0..500 {
            console.write_str(
                "hello world\n",
            );
        }

        /*
         * cursor_line is absolute and is therefore expected to
         * be much larger than the number of visible rows.
         */
        if console.cursor_x
            >= console.columns {
            return TestResult::Fail(
                "cursor escaped columns after long output",
            );
        }

        if console.cursor_row()
            >= console.rows {
            return TestResult::Fail(
                "cursor escaped visible rows after long output",
            );
        }

        if console.cursor_line
            < console.view_top {
            return TestResult::Fail(
                "cursor moved above viewport",
            );
        }

        if console.view_top
            != console.live_view_top() {
            return TestResult::Fail(
                "long output left viewport away from live view",
            );
        }

        pass()
    }

    // ========================================================
    // Repeated clear/write
    // ========================================================

    #[test]
    fn console_repeated_clear_and_write_is_stable()
        -> TestResult
    {
        let mut console =
            Console::new(
                800,
                480,
            );

        for _ in 0..20 {
            console.clear();

            console.write_str(
                "hello\nworld",
            );

            if console.cursor_line != 1 {
                return TestResult::Fail(
                    "repeated clear/write corrupted cursor line",
                );
            }

            if console.cursor_x != 5 {
                return TestResult::Fail(
                    "repeated clear/write corrupted cursor column",
                );
            }

            if console.view_top != 0 {
                return TestResult::Fail(
                    "repeated clear/write corrupted viewport",
                );
            }

            if cell(
                &console,
                0,
                0,
            ).character != b'h' {
                return TestResult::Fail(
                    "repeated clear/write corrupted cells",
                );
            }
        }

        pass()
    }
}
