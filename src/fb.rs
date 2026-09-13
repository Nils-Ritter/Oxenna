/*
Framebuffer:
    one frontbuffer
    one backbuffer

The frontbuffer is what is currently shown to the user.

The backbuffer is what the kernel draws to.

When the kernel is done drawing, fb::present() should be called.

present() copies only the region of the backbuffer that has
changed.

No heap allocation is used by this framebuffer.
*/

use core::ptr;

use limine::request::FramebufferRequest;

#[used]
#[unsafe(link_section = ".limine_reqs")]
static FRAMEBUFFER_REQUEST: FramebufferRequest =
    FramebufferRequest::new();

const MAX_WIDTH: usize = 1920;
const MAX_HEIGHT: usize = 1080;

const BYTES_PER_PIXEL: usize = 4;
const MAX_PIXELS: usize = MAX_WIDTH * MAX_HEIGHT;

static mut BACKBUFFER: [u32; MAX_PIXELS] =
    [0; MAX_PIXELS];

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color {
        r: 0,
        g: 0,
        b: 0,
    };

    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
    };

    pub const RED: Color = Color {
        r: 255,
        g: 0,
        b: 0,
    };

    pub const GREEN: Color = Color {
        r: 0,
        g: 255,
        b: 0,
    };

    pub const BLUE: Color = Color {
        r: 0,
        g: 0,
        b: 255,
    };

    pub fn from_name(name: &str) -> Option<Color> {
        match name {
            "black" => Some(Color::BLACK),
            "red" => Some(Color::RED),
            "green" => Some(Color::GREEN),
            "white" => Some(Color::WHITE),
            "blue" => Some(Color::BLUE),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Info {
    front: *mut u8,

    width: usize,
    height: usize,

    front_pitch: usize,

    red_shift: u8,
    green_shift: u8,
    blue_shift: u8,
}

unsafe impl Sync for Info {}

static mut INFO: Option<Info> = None;

/*
Dirty rectangle.

The rectangle is:

    [min_x, max_x)
    [min_y, max_y)

If DIRTY is false, there is nothing to present.
*/

static mut DIRTY: bool = false;

static mut DIRTY_MIN_X: usize = 0;
static mut DIRTY_MIN_Y: usize = 0;
static mut DIRTY_MAX_X: usize = 0;
static mut DIRTY_MAX_Y: usize = 0;

// ============================================================
// Initialization
// ============================================================

pub fn init() {
    let response = FRAMEBUFFER_REQUEST
        .response()
        .expect("No framebuffer response");

    let framebuffer = response
        .framebuffers()
        .first()
        .expect("No framebuffer");

    let width = framebuffer.width as usize;
    let height = framebuffer.height as usize;
    let pitch = framebuffer.pitch as usize;
    let bpp = framebuffer.bpp as usize;

    if bpp != 32 {
        panic!("Framebuffer must be 32bpp");
    }

    if width == 0 || height == 0 {
        panic!("Framebuffer has invalid dimensions");
    }

    if width > MAX_WIDTH {
        panic!("Framebuffer width too large");
    }

    if height > MAX_HEIGHT {
        panic!("Framebuffer height too large");
    }

    let info = Info {
        front: framebuffer.address() as *mut u8,

        width,
        height,

        front_pitch: pitch,

        red_shift: framebuffer.red_mask_shift,
        green_shift: framebuffer.green_mask_shift,
        blue_shift: framebuffer.blue_mask_shift,
    };

    unsafe {
        ptr::addr_of_mut!(INFO).write(Some(info));

        DIRTY = false;
        DIRTY_MIN_X = 0;
        DIRTY_MIN_Y = 0;
        DIRTY_MAX_X = 0;
        DIRTY_MAX_Y = 0;
    }

    clear(Color::BLACK);
    present();
}

// ============================================================
// Internal
// ============================================================

#[inline(always)]
fn info() -> Info {
    unsafe {
        ptr::addr_of!(INFO)
            .read()
            .expect("Framebuffer not initialized")
    }
}

/*
Marks a rectangle as dirty.

Coordinates are clipped to the framebuffer.
*/

#[inline(always)]
fn mark_dirty(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) {
    let info = info();

    if width == 0 || height == 0 {
        return;
    }

    if x >= info.width || y >= info.height {
        return;
    }

    let max_x = x
        .saturating_add(width)
        .min(info.width);

    let max_y = y
        .saturating_add(height)
        .min(info.height);

    if max_x <= x || max_y <= y {
        return;
    }

    unsafe {
        if !DIRTY {
            DIRTY = true;

            DIRTY_MIN_X = x;
            DIRTY_MIN_Y = y;
            DIRTY_MAX_X = max_x;
            DIRTY_MAX_Y = max_y;
        } else {
            DIRTY_MIN_X = DIRTY_MIN_X.min(x);
            DIRTY_MIN_Y = DIRTY_MIN_Y.min(y);
            DIRTY_MAX_X = DIRTY_MAX_X.max(max_x);
            DIRTY_MAX_Y = DIRTY_MAX_Y.max(max_y);
        }
    }
}

// ============================================================
// Color
// ============================================================

#[inline(always)]
fn color_to_u32(color: Color) -> u32 {
    let info = info();

    ((color.r as u32) << info.red_shift)
        | ((color.g as u32) << info.green_shift)
        | ((color.b as u32) << info.blue_shift)
}

// ============================================================
// Pixel
// ============================================================

#[inline(always)]
pub fn put_pixel(
    x: usize,
    y: usize,
    color: Color,
) {
    let info = info();

    if x >= info.width || y >= info.height {
        return;
    }

    let pixel = color_to_u32(color);

    unsafe {
        let back =
            ptr::addr_of_mut!(BACKBUFFER) as *mut u32;

        *back.add(
            y * info.width + x
        ) = pixel;
    }

    mark_dirty(x, y, 1, 1);
}

// ============================================================
// Clear
// ============================================================

pub fn clear(color: Color) {
    let info = info();

    let pixel = color_to_u32(color);

    unsafe {
        let back =
            ptr::addr_of_mut!(BACKBUFFER) as *mut u32;

        let buffer =
            core::slice::from_raw_parts_mut(
                back,
                info.width * info.height,
            );

        buffer.fill(pixel);
    }

    mark_dirty(
        0,
        0,
        info.width,
        info.height,
    );
}

// ============================================================
// Rectangle
// ============================================================

pub fn draw_rect(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: Color,
) {
    let info = info();

    if width == 0 || height == 0 {
        return;
    }

    if x >= info.width || y >= info.height {
        return;
    }

    let end_x = x
        .saturating_add(width)
        .min(info.width);

    let end_y = y
        .saturating_add(height)
        .min(info.height);

    if end_x <= x || end_y <= y {
        return;
    }

    let pixel = color_to_u32(color);

    unsafe {
        let back =
            ptr::addr_of_mut!(BACKBUFFER) as *mut u32;

        for py in y..end_y {
            let row =
                back.add(
                    py * info.width + x
                );

            let row =
                core::slice::from_raw_parts_mut(
                    row,
                    end_x - x,
                );

            row.fill(pixel);
        }
    }

    mark_dirty(
        x,
        y,
        end_x - x,
        end_y - y,
    );
}

// ============================================================
// Copy rectangle
// ============================================================

/*
Copies a rectangle inside the backbuffer.

ptr::copy() is overlap-safe, making this suitable for
scrolling.
*/

pub fn copy_rect(
    src_x: usize,
    src_y: usize,
    dst_x: usize,
    dst_y: usize,
    width: usize,
    height: usize,
) {
    let info = info();

    if width == 0 || height == 0 {
        return;
    }

    if src_x >= info.width
        || src_y >= info.height
        || dst_x >= info.width
        || dst_y >= info.height
    {
        return;
    }

    let width = width
        .min(info.width - src_x)
        .min(info.width - dst_x);

    let height = height
        .min(info.height - src_y)
        .min(info.height - dst_y);

    if width == 0 || height == 0 {
        return;
    }

    unsafe {
        let back =
            ptr::addr_of_mut!(BACKBUFFER) as *mut u32;

        /*
         * ptr::copy() handles overlapping source/destination
         * regions correctly.
         */
        for row in 0..height {
            let src =
                back.add(
                    (src_y + row) * info.width + src_x
                );

            let dst =
                back.add(
                    (dst_y + row) * info.width + dst_x
                );

            ptr::copy(
                src,
                dst,
                width,
            );
        }
    }

    mark_dirty(
        dst_x,
        dst_y,
        width,
        height,
    );
}

// ============================================================
// Scroll up
// ============================================================

/*
Scrolls the framebuffer backbuffer upward by `pixels`.

The newly exposed area at the bottom is filled with `color`.
*/

pub fn scroll_up(
    pixels: usize,
    color: Color,
) {
    let info = info();

    if pixels == 0 {
        return;
    }

    if pixels >= info.height {
        clear(color);
        return;
    }

    copy_rect(
        0,
        pixels,
        0,
        0,
        info.width,
        info.height - pixels,
    );

    draw_rect(
        0,
        info.height - pixels,
        info.width,
        pixels,
        color,
    );
}

// ============================================================
// Present
// ============================================================

/*
Copies only the dirty region from the backbuffer into the
actual framebuffer.

For a single terminal character this is approximately:

    8 × 16 × 4 = 512 bytes

instead of copying the entire 1920×1080 framebuffer.
*/

pub fn present() {
    let info = info();

    let (
        min_x,
        min_y,
        max_x,
        max_y,
    ) = unsafe {
        if !DIRTY {
            return;
        }

        let result = (
            DIRTY_MIN_X,
            DIRTY_MIN_Y,
            DIRTY_MAX_X,
            DIRTY_MAX_Y,
        );

        DIRTY = false;

        result
    };

    if min_x >= max_x || min_y >= max_y {
        return;
    }

    let width =
        max_x - min_x;

    let row_size =
        width * BYTES_PER_PIXEL;

    unsafe {
        let back =
            ptr::addr_of!(BACKBUFFER) as *const u8;

        let front =
            info.front;

        for y in min_y..max_y {
            let src =
                back.add(
                    y * info.width
                        * BYTES_PER_PIXEL
                        + min_x
                            * BYTES_PER_PIXEL,
                );

            let dst =
                front.add(
                    y * info.front_pitch
                        + min_x
                            * BYTES_PER_PIXEL,
                );

            ptr::copy_nonoverlapping(
                src,
                dst,
                row_size,
            );
        }
    }
}

// ============================================================
// Full Present
// ============================================================

#[allow(unused)]
pub fn present_full() {
    let info = info();

    mark_dirty(
        0,
        0,
        info.width,
        info.height,
    );

    present();
}

// ============================================================
// Info
// ============================================================

#[inline]
pub fn width() -> usize {
    info().width
}

#[inline]
pub fn height() -> usize {
    info().height
}

#[allow(unused)]
#[inline]
pub fn pitch() -> usize {
    info().front_pitch
}

// ============================================================
// Tests
// ============================================================

#[cfg(feature = "test")]
mod tests {
    use super::*;

    use crate::test::{
        test,
        TestResult,
    };

    fn pass() -> TestResult {
        TestResult::Pass
    }

    // --------------------------------------------------------
    // Helpers
    // --------------------------------------------------------

    #[inline]
    fn read_pixel(
        x: usize,
        y: usize,
    ) -> u32 {
        let info = super::info();

        unsafe {
            let back =
                core::ptr::addr_of!(
                    BACKBUFFER
                ) as *const u32;

            *back.add(
                y * info.width + x
            )
        }
    }

    #[inline]
    fn pixel_is(
        x: usize,
        y: usize,
        color: Color,
    ) -> bool {
        read_pixel(x, y)
            == super::color_to_u32(color)
    }

    fn fill_test_pattern(
        width: usize,
        height: usize,
    ) {
        let black =
            color_to_u32(Color::BLACK);

        unsafe {
            let back =
                core::ptr::addr_of_mut!(
                    BACKBUFFER
                ) as *mut u32;

            for y in 0..height {
                for x in 0..width {
                    /*
                     * Each pixel gets a unique-ish value.

                     This makes copy/scroll bugs much easier
                     to detect than using a single color.
                     */

                    let value =
                        ((y as u32) << 16)
                        | ((x as u32) & 0xffff);

                    back.add(
                        y * super::info().width + x
                    ).write(
                        if value == 0 {
                            black
                        } else {
                            value
                        }
                    );
                }
            }
        }
    }

    // --------------------------------------------------------
    // Initialization
    // --------------------------------------------------------

    #[test]
    fn framebuffer_has_valid_dimensions()
        -> TestResult
    {
        let width = width();
        let height = height();

        if width == 0 {
            return TestResult::Fail(
                "framebuffer width is zero",
            );
        }

        if height == 0 {
            return TestResult::Fail(
                "framebuffer height is zero",
            );
        }

        if width > MAX_WIDTH {
            return TestResult::Fail(
                "framebuffer exceeds maximum width",
            );
        }

        if height > MAX_HEIGHT {
            return TestResult::Fail(
                "framebuffer exceeds maximum height",
            );
        }

        pass()
    }

    #[test]
    fn framebuffer_pitch_is_large_enough()
        -> TestResult
    {
        let info = info();

        let minimum =
            info.width * BYTES_PER_PIXEL;

        if info.front_pitch < minimum {
            return TestResult::Fail(
                "framebuffer pitch is smaller than row size",
            );
        }

        pass()
    }

    // --------------------------------------------------------
    // Color conversion
    // --------------------------------------------------------

    #[test]
    fn framebuffer_black_color_is_correct()
        -> TestResult
    {
        if color_to_u32(Color::BLACK) != 0 {
            return TestResult::Fail(
                "black does not convert to zero",
            );
        }

        pass()
    }

    #[test]
    fn framebuffer_colors_have_distinct_values()
        -> TestResult
    {
        let black =
            color_to_u32(Color::BLACK);

        let white =
            color_to_u32(Color::WHITE);

        let red =
            color_to_u32(Color::RED);

        let green =
            color_to_u32(Color::GREEN);

        let blue =
            color_to_u32(Color::BLUE);

        if black == white {
            return TestResult::Fail(
                "black and white have same pixel value",
            );
        }

        if red == green {
            return TestResult::Fail(
                "red and green have same pixel value",
            );
        }

        if red == blue {
            return TestResult::Fail(
                "red and blue have same pixel value",
            );
        }

        if green == blue {
            return TestResult::Fail(
                "green and blue have same pixel value",
            );
        }

        pass()
    }

    // --------------------------------------------------------
    // Clear
    // --------------------------------------------------------

    #[test]
    fn framebuffer_clear_fills_entire_backbuffer()
        -> TestResult
    {
        clear(Color::RED);

        let info = info();

        /*
         * Checking every pixel makes this a genuinely useful
         * test rather than only checking the corners.
         */

        for y in 0..info.height {
            for x in 0..info.width {
                if !pixel_is(
                    x,
                    y,
                    Color::RED,
                ) {
                    return TestResult::Fail(
                        "clear did not fill entire backbuffer",
                    );
                }
            }
        }

        pass()
    }

    #[test]
    fn framebuffer_clear_black_works()
        -> TestResult
    {
        clear(Color::BLACK);

        let info = info();

        /*
         * Check a representative set of pixels rather than
         * redundantly checking every pixel again.
         */

        let points = [
            (0, 0),
            (info.width - 1, 0),
            (0, info.height - 1),
            (
                info.width - 1,
                info.height - 1,
            ),
            (
                info.width / 2,
                info.height / 2,
            ),
        ];

        for &(x, y) in &points {
            if !pixel_is(
                x,
                y,
                Color::BLACK,
            ) {
                return TestResult::Fail(
                    "clear black left non-black pixels",
                );
            }
        }

        pass()
    }

    // --------------------------------------------------------
    // put_pixel
    // --------------------------------------------------------

    #[test]
    fn framebuffer_put_pixel_changes_exact_pixel()
        -> TestResult
    {
        clear(Color::BLACK);

        let x = 10;
        let y = 20;

        put_pixel(
            x,
            y,
            Color::RED,
        );

        if !pixel_is(
            x,
            y,
            Color::RED,
        ) {
            return TestResult::Fail(
                "put_pixel did not change target pixel",
            );
        }

        pass()
    }

    #[test]
    fn framebuffer_put_pixel_does_not_change_neighbors()
        -> TestResult
    {
        clear(Color::BLACK);

        let x = 10;
        let y = 20;

        put_pixel(
            x,
            y,
            Color::RED,
        );

        let neighbors = [
            (x - 1, y),
            (x + 1, y),
            (x, y - 1),
            (x, y + 1),
        ];

        for &(nx, ny) in &neighbors {
            if !pixel_is(
                nx,
                ny,
                Color::BLACK,
            ) {
                return TestResult::Fail(
                    "put_pixel modified neighboring pixel",
                );
            }
        }

        pass()
    }

    #[test]
    fn framebuffer_put_pixel_out_of_bounds_is_safe()
        -> TestResult
    {
        clear(Color::BLACK);

        let info = info();

        put_pixel(
            info.width,
            0,
            Color::RED,
        );

        put_pixel(
            0,
            info.height,
            Color::RED,
        );

        put_pixel(
            info.width + 100,
            info.height + 100,
            Color::RED,
        );

        pass()
    }

    // --------------------------------------------------------
    // draw_rect
    // --------------------------------------------------------

    #[test]
    fn framebuffer_draw_rect_fills_rectangle()
        -> TestResult
    {
        clear(Color::BLACK);

        draw_rect(
            10,
            20,
            30,
            40,
            Color::GREEN,
        );

        for y in 20..60 {
            for x in 10..40 {
                if !pixel_is(
                    x,
                    y,
                    Color::GREEN,
                ) {
                    return TestResult::Fail(
                        "draw_rect failed to fill rectangle",
                    );
                }
            }
        }

        pass()
    }

    #[test]
    fn framebuffer_draw_rect_does_not_modify_outside()
        -> TestResult
    {
        clear(Color::BLACK);

        draw_rect(
            10,
            20,
            30,
            40,
            Color::GREEN,
        );

        let points = [
            (9, 20),
            (40, 20),
            (10, 19),
            (10, 60),
            (0, 0),
        ];

        for &(x, y) in &points {
            if !pixel_is(
                x,
                y,
                Color::BLACK,
            ) {
                return TestResult::Fail(
                    "draw_rect modified outside rectangle",
                );
            }
        }

        pass()
    }

    #[test]
    fn framebuffer_draw_rect_clips_right_edge()
        -> TestResult
    {
        clear(Color::BLACK);

        let info = info();

        draw_rect(
            info.width - 10,
            0,
            100,
            10,
            Color::BLUE,
        );

        for x in info.width - 10..info.width {
            if !pixel_is(
                x,
                0,
                Color::BLUE,
            ) {
                return TestResult::Fail(
                    "rectangle was not clipped correctly",
                );
            }
        }

        pass()
    }

    #[test]
    fn framebuffer_draw_rect_clips_bottom_edge()
        -> TestResult
    {
        clear(Color::BLACK);

        let info = info();

        draw_rect(
            0,
            info.height - 10,
            10,
            100,
            Color::BLUE,
        );

        for y in info.height - 10..info.height {
            if !pixel_is(
                0,
                y,
                Color::BLUE,
            ) {
                return TestResult::Fail(
                    "rectangle was not clipped at bottom edge",
                );
            }
        }

        pass()
    }

    #[test]
    fn framebuffer_zero_sized_rect_does_nothing()
        -> TestResult
    {
        clear(Color::BLACK);

        draw_rect(
            10,
            10,
            0,
            100,
            Color::RED,
        );

        draw_rect(
            10,
            10,
            100,
            0,
            Color::RED,
        );

        if !pixel_is(
            10,
            10,
            Color::BLACK,
        ) {
            return TestResult::Fail(
                "zero-sized rectangle modified framebuffer",
            );
        }

        pass()
    }

    // --------------------------------------------------------
    // copy_rect
    // --------------------------------------------------------

    #[test]
    fn framebuffer_copy_rect_copies_pixels()
        -> TestResult
    {
        clear(Color::BLACK);

        /*
         * Create a small known pattern.
         */

        put_pixel(
            10,
            10,
            Color::RED,
        );

        put_pixel(
            11,
            10,
            Color::GREEN,
        );

        put_pixel(
            10,
            11,
            Color::BLUE,
        );

        put_pixel(
            11,
            11,
            Color::WHITE,
        );

        copy_rect(
            10,
            10,
            100,
            100,
            2,
            2,
        );

        if !pixel_is(
            100,
            100,
            Color::RED,
        ) {
            return TestResult::Fail(
                "copy_rect failed first pixel",
            );
        }

        if !pixel_is(
            101,
            100,
            Color::GREEN,
        ) {
            return TestResult::Fail(
                "copy_rect failed second pixel",
            );
        }

        if !pixel_is(
            100,
            101,
            Color::BLUE,
        ) {
            return TestResult::Fail(
                "copy_rect failed third pixel",
            );
        }

        if !pixel_is(
            101,
            101,
            Color::WHITE,
        ) {
            return TestResult::Fail(
                "copy_rect failed fourth pixel",
            );
        }

        pass()
    }

    #[test]
    fn framebuffer_copy_rect_handles_overlap()
        -> TestResult
    {
        clear(Color::BLACK);

        put_pixel(
            10,
            10,
            Color::RED,
        );

        put_pixel(
            11,
            10,
            Color::GREEN,
        );

        put_pixel(
            12,
            10,
            Color::BLUE,
        );

        /*
         * Shift the row one pixel right.

             R G B
              ↓
             R R G B
         */

        copy_rect(
            10,
            10,
            11,
            10,
            3,
            1,
        );

        if !pixel_is(
            11,
            10,
            Color::RED,
        ) {
            return TestResult::Fail(
                "overlapping copy corrupted first pixel",
            );
        }

        if !pixel_is(
            12,
            10,
            Color::GREEN,
        ) {
            return TestResult::Fail(
                "overlapping copy corrupted second pixel",
            );
        }

        if !pixel_is(
            13,
            10,
            Color::BLUE,
        ) {
            return TestResult::Fail(
                "overlapping copy corrupted third pixel",
            );
        }

        pass()
    }

    // --------------------------------------------------------
    // Scroll
    // --------------------------------------------------------

    #[test]
    fn framebuffer_scroll_moves_pixels_up()
        -> TestResult
    {
        clear(Color::BLACK);

        /*
         * Make four horizontal color bands.

             RED
             GREEN
             BLUE
             WHITE
         */

        draw_rect(
            0,
            0,
            10,
            10,
            Color::RED,
        );

        draw_rect(
            0,
            10,
            10,
            10,
            Color::GREEN,
        );

        draw_rect(
            0,
            20,
            10,
            10,
            Color::BLUE,
        );

        draw_rect(
            0,
            30,
            10,
            10,
            Color::WHITE,
        );

        scroll_up(
            10,
            Color::BLACK,
        );

        /*
         * After scrolling:

             GREEN
             BLUE
             WHITE
             BLACK
         */

        if !pixel_is(
            0,
            0,
            Color::GREEN,
        ) {
            return TestResult::Fail(
                "scroll did not move second row upward",
            );
        }

        if !pixel_is(
            0,
            10,
            Color::BLUE,
        ) {
            return TestResult::Fail(
                "scroll did not move third row upward",
            );
        }

        if !pixel_is(
            0,
            20,
            Color::WHITE,
        ) {
            return TestResult::Fail(
                "scroll did not move fourth row upward",
            );
        }

        if !pixel_is(
            0,
            30,
            Color::BLACK,
        ) {
            return TestResult::Fail(
                "scroll did not clear exposed bottom area",
            );
        }

        pass()
    }

    #[test]
    fn framebuffer_full_scroll_clears_screen()
        -> TestResult
    {
        clear(Color::RED);

        let info = info();

        scroll_up(
            info.height,
            Color::BLACK,
        );

        if !pixel_is(
            0,
            0,
            Color::BLACK,
        ) {
            return TestResult::Fail(
                "full scroll did not clear screen",
            );
        }

        if !pixel_is(
            info.width - 1,
            info.height - 1,
            Color::BLACK,
        ) {
            return TestResult::Fail(
                "full scroll left pixels behind",
            );
        }

        pass()
    }

    #[test]
    fn framebuffer_scroll_by_zero_does_nothing()
        -> TestResult
    {
        clear(Color::RED);

        scroll_up(
            0,
            Color::BLACK,
        );

        if !pixel_is(
            0,
            0,
            Color::RED,
        ) {
            return TestResult::Fail(
                "zero-pixel scroll modified framebuffer",
            );
        }

        pass()
    }

    // --------------------------------------------------------
    // Dirty tracking
    // --------------------------------------------------------

    #[test]
    fn framebuffer_draw_operations_mark_dirty()
        -> TestResult
    {
        clear(Color::BLACK);

        unsafe {
            DIRTY = false;
        }

        put_pixel(
            50,
            60,
            Color::RED,
        );

        unsafe {
            if !DIRTY {
                return TestResult::Fail(
                    "put_pixel did not mark framebuffer dirty",
                );
            }

            DIRTY = false;
        }

        draw_rect(
            100,
            100,
            20,
            20,
            Color::GREEN,
        );

        unsafe {
            if !DIRTY {
                return TestResult::Fail(
                    "draw_rect did not mark framebuffer dirty",
                );
            }

            DIRTY = false;
        }

        pass()
    }

    #[test]
    fn framebuffer_dirty_rectangle_expands()
        -> TestResult
    {
        clear(Color::BLACK);

        unsafe {
            DIRTY = false;
        }

        draw_rect(
            100,
            200,
            10,
            20,
            Color::RED,
        );

        draw_rect(
            50,
            150,
            30,
            40,
            Color::GREEN,
        );

        unsafe {
            if !DIRTY {
                return TestResult::Fail(
                    "dirty flag was not set",
                );
            }

            if DIRTY_MIN_X != 50 {
                return TestResult::Fail(
                    "dirty minimum X is incorrect",
                );
            }

            if DIRTY_MIN_Y != 150 {
                return TestResult::Fail(
                    "dirty minimum Y is incorrect",
                );
            }

            if DIRTY_MAX_X != 110 {
                return TestResult::Fail(
                    "dirty maximum X is incorrect",
                );
            }

            if DIRTY_MAX_Y != 220 {
                return TestResult::Fail(
                    "dirty maximum Y is incorrect",
                );
            }

            DIRTY = false;
        }

        pass()
    }

    #[test]
    fn framebuffer_present_clears_dirty_state()
        -> TestResult
    {
        clear(Color::BLACK);

        put_pixel(
            20,
            20,
            Color::RED,
        );

        unsafe {
            if !DIRTY {
                return TestResult::Fail(
                    "test setup did not mark dirty",
                );
            }
        }

        present();

        unsafe {
            if DIRTY {
                return TestResult::Fail(
                    "present did not clear dirty state",
                );
            }
        }

        pass()
    }

    // --------------------------------------------------------
    // Stress / consistency
    // --------------------------------------------------------

    #[test]
    fn framebuffer_repeated_clear_is_stable()
        -> TestResult
    {
        for _ in 0..10 {
            clear(Color::BLACK);
            clear(Color::WHITE);
            clear(Color::BLACK);
        }

        if !pixel_is(
            0,
            0,
            Color::BLACK,
        ) {
            return TestResult::Fail(
                "repeated clear produced incorrect result",
            );
        }

        pass()
    }
}
