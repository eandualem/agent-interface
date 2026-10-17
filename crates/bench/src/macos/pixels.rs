//! Reading captured images: how much of a region shows a colour, and whether
//! a region changed between frames.

/// A borrowed image of 32-bit pixels in blue, green, red, alpha byte order,
/// as ScreenCaptureKit delivers them.
#[derive(Clone, Copy, Debug)]
pub struct Bgra<'a> {
    /// The width, in pixels.
    pub width: usize,
    /// The height, in pixels.
    pub height: usize,
    /// The bytes from the start of one row to the start of the next, which
    /// can be more than four times the width.
    pub bytes_per_row: usize,
    /// The rows, top first.
    pub pixels: &'a [u8],
}

/// A rectangle of pixels, from the top left of an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// The leftmost column.
    pub x: usize,
    /// The top row.
    pub y: usize,
    /// The width, in pixels.
    pub width: usize,
    /// The height, in pixels.
    pub height: usize,
}

/// A colour as captured, without alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Color {
    /// Returns whether every channel is within `tolerance` of `other`'s.
    ///
    /// Captured colours pass through colour-space conversion, so a colour
    /// that was drawn exactly rarely comes back exactly.
    pub fn near(self, other: Color, tolerance: u8) -> bool {
        self.r.abs_diff(other.r) <= tolerance
            && self.g.abs_diff(other.g) <= tolerance
            && self.b.abs_diff(other.b) <= tolerance
    }
}

impl Bgra<'_> {
    /// Returns the part of `rect` that lies inside the image.
    pub fn clip(&self, rect: Rect) -> Rect {
        let x = rect.x.min(self.width);
        let y = rect.y.min(self.height);
        Rect {
            x,
            y,
            width: rect.width.min(self.width - x),
            height: rect.height.min(self.height - y),
        }
    }

    /// Returns the bytes of row `y` within `rect`, four per pixel. `rect`
    /// must already be clipped.
    fn row(&self, rect: Rect, y: usize) -> &[u8] {
        let start = y * self.bytes_per_row + rect.x * 4;
        &self.pixels[start..start + rect.width * 4]
    }
}

/// Returns the fraction of the pixels of `rect` for which `predicate` holds,
/// from 0 to 1. The part of `rect` outside the image is ignored; an empty
/// region gives 0.
pub fn fraction_matching(image: &Bgra<'_>, rect: Rect, predicate: impl Fn(Color) -> bool) -> f64 {
    let rect = image.clip(rect);
    let total = rect.width * rect.height;
    if total == 0 {
        return 0.0;
    }
    let mut matching = 0usize;
    for y in rect.y..rect.y + rect.height {
        for pixel in image.row(rect, y).as_chunks::<4>().0 {
            let color = Color {
                r: pixel[2],
                g: pixel[1],
                b: pixel[0],
            };
            if predicate(color) {
                matching += 1;
            }
        }
    }
    matching as f64 / total as f64
}

/// Returns a hash of the pixels of `rect`, to tell whether the region changed
/// between two frames. Every pixel counts, so a change of a single glyph
/// changes the hash. The part of `rect` outside the image is ignored.
pub fn region_digest(image: &Bgra<'_>, rect: Rect) -> u64 {
    // A multiply-rotate hash over eight bytes at a time: fast enough for a
    // full window on every frame, and a change shows up in the result with
    // near certainty. It is not meant to resist deliberate collisions.
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let rect = image.clip(rect);
    let mix = |hash: u64, word: u64| (hash.rotate_left(5) ^ word).wrapping_mul(K);
    let mut hash = mix(mix(0, rect.width as u64), rect.height as u64);
    for y in rect.y..rect.y + rect.height {
        let (words, rest) = image.row(rect, y).as_chunks::<8>();
        for word in words {
            hash = mix(hash, u64::from_le_bytes(*word));
        }
        let mut tail = [0u8; 8];
        tail[..rest.len()].copy_from_slice(rest);
        hash = mix(hash, u64::from_le_bytes(tail));
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` by `height` image filled with `fill`, with two bytes of
    /// padding at the end of every row.
    fn image(width: usize, height: usize, fill: Color) -> (Vec<u8>, usize) {
        let bytes_per_row = width * 4 + 2;
        let mut pixels = vec![0xee; bytes_per_row * height];
        for y in 0..height {
            for x in 0..width {
                set(&mut pixels, bytes_per_row, x, y, fill);
            }
        }
        (pixels, bytes_per_row)
    }

    fn set(pixels: &mut [u8], bytes_per_row: usize, x: usize, y: usize, color: Color) {
        let at = y * bytes_per_row + x * 4;
        pixels[at..at + 4].copy_from_slice(&[color.b, color.g, color.r, 0xff]);
    }

    const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    const MAGENTA: Color = Color {
        r: 255,
        g: 0,
        b: 255,
    };

    #[test]
    fn counts_the_pixels_near_a_colour() {
        let (mut pixels, bytes_per_row) = image(10, 4, BLACK);
        // The left half of the top two rows, slightly off magenta, as a
        // colour-space conversion leaves it.
        for y in 0..2 {
            for x in 0..5 {
                let color = Color {
                    r: 250,
                    g: 6,
                    b: 247,
                };
                set(&mut pixels, bytes_per_row, x, y, color);
            }
        }
        let image = Bgra {
            width: 10,
            height: 4,
            bytes_per_row,
            pixels: &pixels,
        };
        let magenta = |color: Color| color.near(MAGENTA, 10);
        let all = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        assert_eq!(fraction_matching(&image, all, magenta), 0.25);
        let corner = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 2,
        };
        assert_eq!(fraction_matching(&image, corner, magenta), 1.0);
        assert_eq!(
            fraction_matching(&image, corner, |color| color.near(MAGENTA, 2)),
            0.0
        );
        // Outside the image is ignored, and an empty region matches nothing.
        let overhanging = Rect {
            x: 5,
            y: 2,
            width: 100,
            height: 100,
        };
        assert_eq!(fraction_matching(&image, overhanging, magenta), 0.0);
        let empty = Rect {
            x: 20,
            y: 0,
            width: 3,
            height: 3,
        };
        assert_eq!(fraction_matching(&image, empty, |_| true), 0.0);
    }

    #[test]
    fn the_digest_changes_with_any_pixel_of_the_region_only() {
        let (mut pixels, bytes_per_row) = image(9, 5, BLACK);
        let region = Rect {
            x: 1,
            y: 1,
            width: 7,
            height: 3,
        };
        let digest = |pixels: &[u8]| {
            let image = Bgra {
                width: 9,
                height: 5,
                bytes_per_row,
                pixels,
            };
            region_digest(&image, region)
        };
        let before = digest(&pixels);
        assert_eq!(digest(&pixels), before);

        // Outside the region, and in the row padding: no change.
        set(&mut pixels, bytes_per_row, 0, 0, MAGENTA);
        set(&mut pixels, bytes_per_row, 8, 4, MAGENTA);
        pixels[bytes_per_row - 1] = 0;
        assert_eq!(digest(&pixels), before);

        // Every pixel inside the region changes the digest, including the
        // last one of a row, which falls in the unaligned tail.
        for y in 1..4 {
            for x in 1..8 {
                let mut changed = pixels.clone();
                let at = y * bytes_per_row + x * 4;
                changed[at] ^= 1;
                assert_ne!(digest(&changed), before, "pixel {x},{y}");
            }
        }
    }
}
