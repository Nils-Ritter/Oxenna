use core::{
    fmt,
    mem::MaybeUninit,
};

use crate::{
    fb::{self, Color},
    font,
    shell,
};

const CHAR_WIDTH: usize = 8;
const CHAR_HEIGHT: usize = 16;

const DEFAULT_FOREGROUND: Color =
    Color::WHITE;

const DEFAULT_BACKGROUND: Color =
    Color::BLACK;

const INPUT_SIZE: usize = 256;

const MAX_COLUMNS: usize = 1920 / CHAR_WIDTH;
const MAX_ROWS: usize = 1080 / CHAR_HEIGHT;


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
The terminal cell buffer is fixed-size.

At 1920x1080:

    240 columns × 67 rows

No heap allocation.
*/

static mut CELLS:
    [Cell; MAX_COLUMNS * MAX_ROWS] =
    [Cell::EMPTY; MAX_COLUMNS * MAX_ROWS];

// ============================================================
// Console
// ============================================================

pub struct Console {
    font: font::Font,

    cursor_x: usize,
    cursor_y: usize,

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
            (width / CHAR_WIDTH)
                .min(MAX_COLUMNS);

        let rows =
            (height / CHAR_HEIGHT)
                .min(MAX_ROWS);

        let cells =
            core::ptr::addr_of_mut!(CELLS)
                as *mut Cell;

        unsafe {
            let cells =
                core::slice::from_raw_parts_mut(
                    cells,
                    MAX_COLUMNS * MAX_ROWS,
                );

            cells.fill(Cell::EMPTY);
        }

        Self {
            font: font::spleen(),

            cursor_x: 0,
            cursor_y: 0,

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
        row: usize,
        column: usize,
    ) -> usize {
        row * MAX_COLUMNS + column
    }

    #[inline(always)]
    fn get_cell(
        &self,
        row: usize,
        column: usize,
    ) -> Cell {
        unsafe {
            *self.cells.add(
                Self::cell_index(
                    row,
                    column,
                ),
            )
        }
    }

    #[inline(always)]
    fn set_cell(
        &mut self,
        row: usize,
        column: usize,
        cell: Cell,
    ) {
        unsafe {
            self.cells.add(
                Self::cell_index(
                    row,
                    column,
                )
            ).write(cell);
        }
    }

    fn clear_row(
        &mut self,
        row: usize,
    ) {
        if row >= self.rows {
            return;
        }

        unsafe {
            let start =
                self.cells.add(
                    Self::cell_index(
                        row,
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
                    MAX_COLUMNS * MAX_ROWS,
                );

            cells.fill(Cell::EMPTY);
        }

        self.cursor_x = 0;
        self.cursor_y = 0;

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
    // Render cell
    // ========================================================

    /*
    Draw exactly one terminal cell.

    This is the hot path for typing.
    */

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

        let cell =
            self.get_cell(
                row,
                column,
            );

        let x =
            column * CHAR_WIDTH;

        let y =
            row * CHAR_HEIGHT;

        /*
        Clear the cell first because the font only paints
        foreground pixels.
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

    // ========================================================
    // Render row
    // ========================================================

    #[allow(unused)]
    fn render_row(
        &self,
        row: usize,
    ) {
        if row >= self.rows {
            return;
        }

        for column in 0..self.columns {
            self.render_cell(
                row,
                column,
            );
        }
    }

    // ========================================================
    // Full render
    // ========================================================

    /*
    Full redraw of the terminal.

    This is NOT used when typing normal characters.
    */

    #[allow(unused)]
    pub fn render(&self) {
        fb::clear(
            self.background,
        );

        for row in 0..self.rows {
            self.render_row(row);
        }
    }

    // ========================================================
    // Scroll
    // ========================================================

    fn scroll(&mut self) {
        if self.rows == 0 {
            return;
        }

        /*
        First move the framebuffer pixels upward by exactly
        one character height.

            16 pixels
        */
        fb::scroll_up(
            CHAR_HEIGHT,
            self.background,
        );

        /*
        Now move the terminal's logical cells upward by one row.

            row 1 -> row 0
            row 2 -> row 1
            row 3 -> row 2
            ...
            row N -> row N-1

        This keeps the cell buffer synchronized with the
        framebuffer.
        */

        unsafe {
            let cells =
                core::slice::from_raw_parts_mut(
                    self.cells,
                    MAX_COLUMNS * MAX_ROWS,
                );

            for row in 1..self.rows {
                let src =
                    row * MAX_COLUMNS;

                let dst =
                    (row - 1) * MAX_COLUMNS;

                cells.copy_within(
                    src..src + self.columns,
                    dst,
                );
            }
        }

        /*
        Clear the newly exposed bottom terminal row.
        */

        self.clear_row(
            self.rows - 1,
        );

        /*
        Cursor is now at the beginning of the new bottom row.
        */

        self.cursor_x = 0;

        self.cursor_y =
            self.rows - 1;
    }

    // ========================================================
    // Newline
    // ========================================================

    fn newline(&mut self) {
        self.cursor_x = 0;

        if self.rows == 0 {
            return;
        }

        if self.cursor_y + 1 < self.rows {
            self.cursor_y += 1;
        } else {
            self.scroll();
        }
    }

    // ========================================================
    // Put character
    // ========================================================

    fn put_char(
        &mut self,
        character: u8,
    ) {
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

        let row =
            self.cursor_y;

        let column =
            self.cursor_x;

        let cell = Cell {
            character,

            foreground:
                self.foreground,

            background:
                self.background,
        };

        self.set_cell(
            row,
            column,
            cell,
        );

        /*
        Only this 8x16 cell gets rendered.
        */

        self.render_cell(
            row,
            column,
        );

        self.cursor_x += 1;

        /*
        Wrapping at the right edge.
        */

        if self.cursor_x >= self.columns {
            self.newline();
        }
    }

    // ========================================================
    // Backspace
    // ========================================================

    fn backspace(&mut self) {
        if self.cursor_x == 0 {
            if self.cursor_y == 0 {
                return;
            }

            self.cursor_y -= 1;

            self.cursor_x =
                self.columns
                    .saturating_sub(1);
        } else {
            self.cursor_x -= 1;
        }

        self.set_cell(
            self.cursor_y,
            self.cursor_x,
            Cell::EMPTY,
        );

        /*
        Only redraw the erased character cell.
        */

        self.render_cell(
            self.cursor_y,
            self.cursor_x,
        );
    }

    // ========================================================
    // Write string
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
    pub fn receive_key(&mut self, key: char) {
        if true { //TODO: Remove and indent
            match key {
                '\n' | '\r' => {
                    self.put_char(b'\n');

                    self.line_ready = true;
                }

                '\u{8}' | '\u{7f}' => {
                    if self.input_len > 0 && self.cursor_x > 2 {
                        self.input_len -= 1;
                        self.put_char(8);
                    }
                }

                character
                    if character.is_ascii()
                        && !character.is_ascii_control() =>
                {
                    if self.input_len < INPUT_SIZE {
                        self.input[self.input_len] = character as u8;
                        self.input_len += 1;

                        self.put_char(character as u8);
                    }
                }

                _ => {}
            }
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
            self.input_len
                .min(buffer.len());

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

static mut CONSOLE:
    MaybeUninit<Console> =
    MaybeUninit::uninit();

static mut CONSOLE_INITIALIZED:
    bool = false;

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

    /*
    Don't call present() for every character.

    Formatting "hello world" changes many cells, but we only
    need one final presentation.
    */

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
    with_console(|console| {
        let old_color = console.foreground;

        console.foreground = color;

        let _ = write_fmt(args);

        console.foreground = old_color;
    });

    fb::present();
}

// ============================================================
// Keyboard
// ============================================================
pub fn receive_key(key: char) {
    if key == '\n' || key == '\r' {
        let mut command_buffer = [0u8; INPUT_SIZE];

        let length = with_console(|console| {
            console.receive_key(key);

            console.read_line(&mut command_buffer)
        });

        if let Some(length) = length {
            if let Ok(command) =
                core::str::from_utf8(&command_buffer[..length])
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
// Serial Mirroring
// ============================================================

use core::sync::atomic::{
    AtomicBool,
    Ordering,
};

pub static SERIAL_MIRROR: AtomicBool =
    AtomicBool::new(false);

pub fn set_serial_mirror(enabled: bool) {
    SERIAL_MIRROR.store(
        enabled,
        Ordering::Relaxed,
    );
}

#[inline]
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
        let args = core::format_args!($($arg)*);

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
        let args = core::format_args!(
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
        let args = core::format_args!($($arg)*);

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
        let args = core::format_args!(
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
    use crate::{console, test::{
        TestResult, test,
    }};

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

    fn cell(
        console: &Console,
        row: usize,
        column: usize,
    ) -> Cell {
        console.get_cell(
            row,
            column,
        )
    }

    fn assert_cell_char(
        console: &Console,
        row: usize,
        column: usize,
        expected: u8,
    ) -> TestResult {
        if cell(
            console,
            row,
            column,
        ).character != expected {
            return TestResult::Fail(
                "terminal cell contains incorrect character",
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

        if console.cursor_y != 0 {
            return TestResult::Fail(
                "cursor_y is not initialized to zero",
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

        if console.cursor_y != 0 {
            return TestResult::Fail(
                "writing characters changed cursor row",
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

        if console.cursor_y != 1 {
            return TestResult::Fail(
                "newline did not advance row",
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

        if console.cursor_y != 3 {
            return TestResult::Fail(
                "multiple newlines produced wrong row",
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

        if console.cursor_y != 0 {
            return TestResult::Fail(
                "carriage return changed row",
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

        if console.cursor_y != 1 {
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

        if console.cursor_y != 1 {
            return TestResult::Fail(
                "console did not advance row after wrapping",
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
            || console.cursor_y != 0
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
            || console.cursor_y != 0 {
            return TestResult::Fail(
                "clear did not reset cursor",
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

             row 0 = A
             row 1 = B
             row 2 = C
         */

        console.write_str(
            "A\nB\nC",
        );

        /*
         * Force a newline at the bottom.
         */

        console.write_str(
            "\n",
        );

        /*
         * Expected:

             row 0 = B
             row 1 = C
             row 2 = empty
         */

        if cell(
            &console,
            0,
            0,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll did not move second line to first",
            );
        }

        if cell(
            &console,
            1,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "scroll did not move third line to second",
            );
        }

        if cell(
            &console,
            2,
            0,
        ).character != b' ' {
            return TestResult::Fail(
                "scroll did not clear new bottom row",
            );
        }

        if console.cursor_y != 2 {
            return TestResult::Fail(
                "cursor is not on bottom row after scroll",
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

        if cell(
            &console,
            0,
            0,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll corrupted first character",
            );
        }

        if cell(
            &console,
            0,
            3,
        ).character != b'B' {
            return TestResult::Fail(
                "scroll corrupted first line",
            );
        }

        if cell(
            &console,
            1,
            0,
        ).character != b'C' {
            return TestResult::Fail(
                "scroll corrupted second line",
            );
        }

        if cell(
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

        let y =
            console.cursor_y;

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

        if console.cursor_y != y {
            return TestResult::Fail(
                "unknown control character moved row",
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

        if console.cursor_y
            >= console.rows {
            return TestResult::Fail(
                "cursor escaped console after long output",
            );
        }

        if console.cursor_x
            >= console.columns {
            return TestResult::Fail(
                "cursor escaped columns after long output",
            );
        }

        pass()
    }

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

            if console.cursor_y != 1 {
                return TestResult::Fail(
                    "repeated clear/write corrupted cursor",
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
