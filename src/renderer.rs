use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;

use tempfile::tempfile;
use wayland_client::QueueHandle;
use wayland_client::protocol::{wl_buffer, wl_shm};

const GLYPH_SCALE: i32 = 2;

struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

pub struct ShmBarBuffer {
    #[allow(dead_code)]
    file: File,
    pub buffer: wl_buffer::WlBuffer,
}

pub fn render_visible_pixels(
    width: u32,
    height: u32,
    background: [u8; 4],
    text_color: [u8; 4],
    left: &str,
    center: &str,
    right: &str,
) -> Vec<u8> {
    const SIDE_PADDING: i32 = 14;
    const SECTION_GAP: i32 = 24;

    let stride = (width * 4) as usize;
    let size = stride * height as usize;
    let mut pixels = vec![0_u8; size];
    fill_rect(
        &mut pixels,
        width,
        height,
        Rect {
            x: 0,
            y: 0,
            w: width as i32,
            h: height as i32,
        },
        background,
    );

    let y = ((height as i32 - glyph_height()) / 2).max(0);
    draw_text(
        &mut pixels,
        width,
        height,
        SIDE_PADDING,
        y,
        left,
        text_color,
    );

    let right_w = text_width(right);
    let right_x = (width as i32 - SIDE_PADDING - right_w).max(SIDE_PADDING);
    draw_text(&mut pixels, width, height, right_x, y, right, text_color);

    let left_end = SIDE_PADDING + text_width(left);
    let center_min_x = left_end + SECTION_GAP;
    let center_max_x = right_x - SECTION_GAP;
    if let Some((center_text, center_x)) =
        fit_centered_text(center, width as i32, center_min_x, center_max_x)
    {
        draw_text(
            &mut pixels,
            width,
            height,
            center_x,
            y,
            &center_text,
            text_color,
        );
    }

    pixels
}

pub fn create_solid_bar_buffer<State>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<State>,
    width: u32,
    height: u32,
    rgba: [u8; 4],
) -> Option<ShmBarBuffer>
where
    State: wayland_client::Dispatch<wl_shm::WlShm, ()>
        + wayland_client::Dispatch<wl_buffer::WlBuffer, ()>
        + wayland_client::Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()>
        + 'static,
{
    if width == 0 || height == 0 {
        return None;
    }

    let stride = width.checked_mul(4)?;
    let size_u32 = stride.checked_mul(height)?;
    let size = usize::try_from(size_u32).ok()?;

    let mut pixels = vec![0_u8; size];
    for px in pixels.chunks_exact_mut(4) {
        px[0] = rgba[2];
        px[1] = rgba[1];
        px[2] = rgba[0];
        px[3] = rgba[3];
    }

    create_buffer_from_pixels(shm, qh, width, height, &pixels)
}

pub fn create_buffer_from_pixels<State>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<State>,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Option<ShmBarBuffer>
where
    State: wayland_client::Dispatch<wl_shm::WlShm, ()>
        + wayland_client::Dispatch<wl_buffer::WlBuffer, ()>
        + wayland_client::Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()>
        + 'static,
{
    if width == 0 || height == 0 {
        return None;
    }

    let stride = width.checked_mul(4)?;
    let size_u32 = stride.checked_mul(height)?;
    let size = usize::try_from(size_u32).ok()?;
    if pixels.len() != size {
        return None;
    }

    let mut file = tempfile().ok()?;
    file.set_len(u64::from(size_u32)).ok()?;

    file.seek(SeekFrom::Start(0)).ok()?;
    file.write_all(pixels).ok()?;

    let pool = shm.create_pool(file.as_fd(), size_u32 as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();

    Some(ShmBarBuffer { file, buffer })
}

fn fill_rect(pixels: &mut [u8], width: u32, height: u32, rect: Rect, rgba: [u8; 4]) {
    let x0 = rect.x.max(0).min(width as i32);
    let y0 = rect.y.max(0).min(height as i32);
    let x1 = (rect.x + rect.w).max(0).min(width as i32);
    let y1 = (rect.y + rect.h).max(0).min(height as i32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for py in y0..y1 {
        for px in x0..x1 {
            set_pixel(pixels, width, px, py, rgba);
        }
    }
}

fn set_pixel(pixels: &mut [u8], width: u32, x: i32, y: i32, rgba: [u8; 4]) {
    let idx = ((y as u32 * width + x as u32) * 4) as usize;
    if idx + 3 >= pixels.len() {
        return;
    }
    pixels[idx] = rgba[2];
    pixels[idx + 1] = rgba[1];
    pixels[idx + 2] = rgba[0];
    pixels[idx + 3] = rgba[3];
}

fn draw_text(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    mut x: i32,
    y: i32,
    text: &str,
    rgba: [u8; 4],
) {
    for ch in text.chars() {
        draw_glyph(pixels, width, height, x, y, ch, rgba);
        x += glyph_advance();
    }
}

fn text_width(text: &str) -> i32 {
    text.chars().count() as i32 * glyph_advance()
}

fn fit_centered_text(text: &str, width: i32, min_x: i32, max_x: i32) -> Option<(String, i32)> {
    if min_x >= max_x {
        return None;
    }

    let available_w = max_x - min_x;
    if available_w < glyph_advance() * 3 {
        return None;
    }

    let fitted = truncate_text_to_width(text, available_w);
    if fitted.is_empty() {
        return None;
    }

    let text_w = text_width(&fitted);
    let ideal_x = (width - text_w) / 2;
    let x = ideal_x.clamp(min_x, max_x - text_w);
    Some((fitted, x))
}

fn truncate_text_to_width(text: &str, max_width: i32) -> String {
    if max_width <= 0 {
        return String::new();
    }

    if text_width(text) <= max_width {
        return text.to_string();
    }

    let ellipsis = "...";
    let ellipsis_w = text_width(ellipsis);
    if ellipsis_w > max_width {
        return String::new();
    }

    let mut fitted = String::new();
    for ch in text.chars() {
        let next_w = text_width(&fitted) + glyph_advance();
        if next_w + ellipsis_w > max_width {
            break;
        }
        fitted.push(ch);
    }

    if fitted.is_empty() {
        String::new()
    } else {
        fitted.push_str(ellipsis);
        fitted
    }
}

fn glyph_height() -> i32 {
    7 * GLYPH_SCALE
}

fn glyph_advance() -> i32 {
    6 * GLYPH_SCALE
}

fn draw_glyph(pixels: &mut [u8], width: u32, height: u32, x: i32, y: i32, ch: char, rgba: [u8; 4]) {
    let rows = glyph_rows(ch);
    for (ry, row) in rows.iter().enumerate() {
        for rx in 0..5 {
            let bit = 1 << (4 - rx);
            if row & bit == 0 {
                continue;
            }
            fill_rect(
                pixels,
                width,
                height,
                Rect {
                    x: x + rx * GLYPH_SCALE,
                    y: y + ry as i32 * GLYPH_SCALE,
                    w: GLYPH_SCALE,
                    h: GLYPH_SCALE,
                },
                rgba,
            );
        }
    }
}

fn glyph_rows(ch: char) -> [u8; 7] {
    let c = if ch.is_ascii_lowercase() {
        ch.to_ascii_uppercase()
    } else {
        ch
    };
    match c {
        'A' => [0b01110, 0b10001, 0b11111, 0b10001, 0b10001, 0, 0],
        'B' => [0b11110, 0b10001, 0b11110, 0b10001, 0b11110, 0, 0],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b01111, 0, 0],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b11110, 0, 0],
        'E' => [0b11111, 0b10000, 0b11110, 0b10000, 0b11111, 0, 0],
        'F' => [0b11111, 0b10000, 0b11110, 0b10000, 0b10000, 0, 0],
        'G' => [0b01111, 0b10000, 0b10111, 0b10001, 0b01110, 0, 0],
        'H' => [0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0, 0],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b11111, 0, 0],
        'J' => [0b00001, 0b00001, 0b00001, 0b10001, 0b01110, 0, 0],
        'K' => [0b10001, 0b10010, 0b11100, 0b10010, 0b10001, 0, 0],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b11111, 0, 0],
        'M' => [0b10001, 0b11011, 0b10101, 0b10001, 0b10001, 0, 0],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0, 0],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0, 0],
        'P' => [0b11110, 0b10001, 0b11110, 0b10000, 0b10000, 0, 0],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10011, 0b01111, 0, 0],
        'R' => [0b11110, 0b10001, 0b11110, 0b10010, 0b10001, 0, 0],
        'S' => [0b01111, 0b10000, 0b01110, 0b00001, 0b11110, 0, 0],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b01110, 0, 0],
        'V' => [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0, 0],
        'W' => [0b10001, 0b10001, 0b10101, 0b11011, 0b10001, 0, 0],
        'X' => [0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0, 0],
        'Y' => [0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0, 0],
        'Z' => [0b11111, 0b00010, 0b00100, 0b01000, 0b11111, 0, 0],
        '0' => [0b01110, 0b10011, 0b10101, 0b11001, 0b01110, 0, 0],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b01110, 0, 0],
        '2' => [0b01110, 0b10001, 0b00010, 0b00100, 0b11111, 0, 0],
        '3' => [0b11110, 0b00001, 0b01110, 0b00001, 0b11110, 0, 0],
        '4' => [0b00010, 0b00110, 0b01010, 0b11111, 0b00010, 0, 0],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b11110, 0, 0],
        '6' => [0b01110, 0b10000, 0b11110, 0b10001, 0b01110, 0, 0],
        '7' => [0b11111, 0b00010, 0b00100, 0b01000, 0b01000, 0, 0],
        '8' => [0b01110, 0b10001, 0b01110, 0b10001, 0b01110, 0, 0],
        '9' => [0b01110, 0b10001, 0b01111, 0b00001, 0b01110, 0, 0],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01110, 0, 0],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b01110, 0, 0],
        ':' => [0, 0b00100, 0, 0, 0b00100, 0, 0],
        '%' => [0b11001, 0b11010, 0b00100, 0b01011, 0b10011, 0, 0],
        '-' => [0, 0, 0b11111, 0, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0b00100, 0, 0],
        '/' => [0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        '🔊' => [
            0b00010, 0b00111, 0b11110, 0b11110, 0b11110, 0b00111, 0b00010,
        ],
        '🔇' => [
            0b10010, 0b01011, 0b11110, 0b11110, 0b11110, 0b00111, 0b00001,
        ],
        '🔋' => [
            0b01110, 0b11011, 0b10001, 0b10001, 0b10001, 0b11011, 0b01110,
        ],
        '⚡' => [
            0b00110, 0b00100, 0b01111, 0b00110, 0b11100, 0b00100, 0b01100,
        ],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        _ => [0b11111, 0b00001, 0b00110, 0b00000, 0b00100, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::{fit_centered_text, text_width, truncate_text_to_width};

    #[test]
    fn truncate_text_to_width_adds_ellipsis_when_needed() {
        assert_eq!(truncate_text_to_width("ABCDEFGHIJ", 60), "AB...");
    }

    #[test]
    fn fit_centered_text_keeps_text_inside_gap_bounds() {
        let (text, x) = fit_centered_text("SONG NAME", 400, 120, 260).expect("center text");
        assert_eq!(text, "SONG NAME");
        assert!(x >= 120);
        assert!(x + text_width(&text) <= 260);
    }
}
