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
