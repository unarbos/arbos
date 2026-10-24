//! Avatar colours shared by every client, so the Mac and the iPhone agree to
//! the hex digit.
//!
//! * The D79 Telegram palette: seven top → bottom gradients, light and dark,
//!   and the `(sum of the id's UTF-8 bytes) mod 7` rule that picks one.
//! * The sender accent: the dominant vibrant colour of a person's picture, for
//!   the 2 pt bar on the left edge of their bubbles. No picture, or a grey one
//!   (black-and-white photo, logo on white) → the midpoint of their D79
//!   gradient, so the bar matches the initials disc beside it.
//!
//! Pure arithmetic on decoded RGBA pixels; the caller owns image decoding.
//!
//! Agents without a photo draw a leaf instead of initials (D92): see [`leaf`].
//! Which card an agent row is drawn from — roster, own project, or the row
//! itself — is one rule for every surface: see [`identity`].

pub mod identity;
pub mod leaf;

/// One Telegram gradient: light and dark top/bottom pairs (dark = lightness
/// −7 %, the relay's `GET /avatar-colors` table), and its name for the picker.
#[derive(Clone, Copy, Debug)]
pub struct Gradient {
    pub name: &'static str,
    pub top: [u8; 3],
    pub bottom: [u8; 3],
    pub dark_top: [u8; 3],
    pub dark_bottom: [u8; 3],
}

/// The seven Telegram peer colours, in Telegram's order (indices 0–6).
pub const GRADIENTS: [Gradient; 7] = [
    Gradient {
        name: "Red",
        top: [0xFF, 0x84, 0x5E],
        bottom: [0xD4, 0x52, 0x46],
        dark_top: [0xFF, 0x69, 0x3A],
        dark_bottom: [0xC8, 0x3B, 0x2E],
    },
    Gradient {
        name: "Orange",
        top: [0xFE, 0xBB, 0x5B],
        bottom: [0xF6, 0x81, 0x36],
        dark_top: [0xFE, 0xAC, 0x38],
        dark_bottom: [0xF4, 0x6C, 0x14],
    },
    Gradient {
        name: "Violet",
        top: [0xB6, 0x94, 0xF9],
        bottom: [0x6C, 0x61, 0xDF],
        dark_top: [0x9F, 0x72, 0xF7],
        dark_bottom: [0x50, 0x43, 0xD9],
    },
    Gradient {
        name: "Green",
        top: [0x9A, 0xD1, 0x64],
        bottom: [0x46, 0xBA, 0x43],
        dark_top: [0x88, 0xC9, 0x48],
        dark_bottom: [0x3C, 0xA0, 0x3A],
    },
    Gradient {
        name: "Cyan",
        top: [0x53, 0xED, 0xD6],
        bottom: [0x28, 0xC9, 0xB7],
        dark_top: [0x33, 0xEA, 0xCE],
        dark_bottom: [0x22, 0xAB, 0x9C],
    },
    Gradient {
        name: "Blue",
        top: [0x5C, 0xAF, 0xFA],
        bottom: [0x40, 0x8A, 0xCF],
        dark_top: [0x39, 0x9E, 0xF9],
        dark_bottom: [0x2F, 0x78, 0xBC],
    },
    Gradient {
        name: "Pink",
        top: [0xFF, 0x8A, 0xA5],
        bottom: [0xE0, 0x5F, 0x8E],
        dark_top: [0xFF, 0x66, 0x8A],
        dark_bottom: [0xDA, 0x41, 0x79],
    },
];

/// Initials size as a fraction of the disc (Telegram: SF Semibold at 44.5 %,
/// one and two letters alike).
pub const INITIALS_SCALE: f32 = 0.445;

/// `(sum of the id's UTF-8 bytes) mod 7` — the same rule on every client.
pub fn color_index(seed: &str) -> u8 {
    (seed.bytes().map(u32::from).sum::<u32>() % 7) as u8
}

/// The gradient for a seed, or the owner's override when there is one.
pub fn index_for(seed: &str, override_index: Option<u8>) -> u8 {
    match override_index {
        Some(i) if i < 7 => i,
        _ => color_index(seed),
    }
}

/// An opaque sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = h.rem_euclid(1.0) * 6.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let to8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgb(to8(r), to8(g), to8(b))
    }
}

/// Gradient `index`'s top and bottom stop for one appearance.
pub fn gradient_stops(index: u8, dark: bool) -> (Rgb, Rgb) {
    let g = GRADIENTS[(index as usize) % 7];
    let rgb = |c: [u8; 3]| Rgb(c[0], c[1], c[2]);
    if dark {
        (rgb(g.dark_top), rgb(g.dark_bottom))
    } else {
        (rgb(g.top), rgb(g.bottom))
    }
}

/// The accent in both appearances; clients pick by the current theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Accent {
    pub dark: Rgb,
    pub light: Rgb,
}

impl Accent {
    pub fn for_appearance(self, dark: bool) -> Rgb {
        if dark {
            self.dark
        } else {
            self.light
        }
    }
}

/// Bump when the extraction or the clamps change, so cached colours are redone.
pub const ACCENT_VERSION: u32 = 1;

/// Downscale target: 48 × 48 keeps the big areas and drops texture noise.
const SIDE: u32 = 48;
/// Lightness below this is near-black: skipped.
const L_BLACK: f32 = 0.12;
/// Lightness above this is near-white: skipped.
const L_WHITE: f32 = 0.92;
/// Saturation below this is grey: skipped.
const S_GREY: f32 = 0.22;
/// Share of the circle that must carry colour, else the picture counts as grey.
const MIN_KEEP: f32 = 0.04;
/// 24 hue bins of 15°.
const BINS: usize = 24;
/// Saturation range the bar may use (both appearances).
const S_CLAMP: (f32, f32) = (0.45, 0.85);
/// Lightness range on the dark theme: light enough to read on `#232323`.
const L_CLAMP_DARK: (f32, f32) = (0.50, 0.66);
/// Lightness range on the light theme: dark enough to read on a pale bubble.
const L_CLAMP_LIGHT: (f32, f32) = (0.36, 0.52);

/// The D79 colour a sender gets without a usable picture: the midpoint of
/// their gradient, i.e. the disc's middle row.
pub fn fallback_accent(seed: &str, override_index: Option<u8>) -> Accent {
    let index = index_for(seed, override_index);
    let mid = |(a, b): (Rgb, Rgb)| {
        Rgb(
            ((a.0 as u16 + b.0 as u16) / 2) as u8,
            ((a.1 as u16 + b.1 as u16) / 2) as u8,
            ((a.2 as u16 + b.2 as u16) / 2) as u8,
        )
    };
    Accent {
        dark: mid(gradient_stops(index, true)),
        light: mid(gradient_stops(index, false)),
    }
}

/// The dominant vibrant colour of a picture, clamped for each appearance, or
/// `None` when the picture is (nearly) colourless. `rgba` is `width × height`
/// 8-bit RGBA, row-major; any size, the function downscales.
pub fn dominant_accent(rgba: &[u8], width: u32, height: u32) -> Option<Accent> {
    let (h, s, l) = dominant_hsl(rgba, width, height)?;
    let clamp = |v: f32, (lo, hi): (f32, f32)| v.clamp(lo, hi);
    let s = clamp(s, S_CLAMP);
    Some(Accent {
        dark: Rgb::from_hsl(h, s, clamp(l, L_CLAMP_DARK)),
        light: Rgb::from_hsl(h, s, clamp(l, L_CLAMP_LIGHT)),
    })
}

/// The accent to draw: the picture's colour when there is one, else D79.
pub fn sender_accent(
    picture: Option<(&[u8], u32, u32)>,
    seed: &str,
    override_index: Option<u8>,
) -> Accent {
    picture
        .and_then(|(rgba, w, h)| dominant_accent(rgba, w, h))
        .unwrap_or_else(|| fallback_accent(seed, override_index))
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

/// Box-average `rgba` down to `SIDE × SIDE`. Returns `None` for a malformed buffer.
fn downscale(rgba: &[u8], width: u32, height: u32) -> Option<Vec<[u8; 4]>> {
    if width == 0 || height == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let side = SIDE as usize;
    let mut out = Vec::with_capacity(side * side);
    for j in 0..side {
        let y0 = j * h / side;
        let y1 = ((j + 1) * h / side).max(y0 + 1).min(h);
        for i in 0..side {
            let x0 = i * w / side;
            let x1 = ((i + 1) * w / side).max(x0 + 1).min(w);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    for (acc, v) in sum.iter_mut().zip(p) {
                        *acc += *v as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out.push([
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ]);
        }
    }
    Some(out)
}

/// Hue, saturation, lightness of the dominant vibrant colour, unclamped.
fn dominant_hsl(rgba: &[u8], width: u32, height: u32) -> Option<(f32, f32, f32)> {
    let small = downscale(rgba, width, height)?;
    let side = SIDE as f32;
    let (cx, cy, r) = (side / 2.0, side / 2.0, side / 2.0);
    let mut kept: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut total = 0usize;
    for (ix, px) in small.iter().enumerate() {
        let x = (ix % SIDE as usize) as f32 + 0.5;
        let y = (ix / SIDE as usize) as f32 + 0.5;
        // Only the inscribed circle: the part of the picture the user sees.
        if (x - cx).powi(2) + (y - cy).powi(2) > r * r {
            continue;
        }
        total += 1;
        if px[3] < 128 {
            continue;
        }
        let (h, s, l) = rgb_to_hsl(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        if !(L_BLACK..=L_WHITE).contains(&l) || s < S_GREY {
            continue;
        }
        // Vibrancy: saturated, mid-lightness pixels count most.
        let weight = s * (1.0 - ((l - 0.5).abs() * 1.6).min(1.0));
        kept.push((h, s, l, weight));
    }
    if total == 0 || (kept.len() as f32) / (total as f32) < MIN_KEEP {
        return None;
    }
    let bin = |h: f32| ((h * BINS as f32) as usize) % BINS;
    let mut hist = [0.0f32; BINS];
    for &(h, _, _, w) in &kept {
        hist[bin(h)] += w;
    }
    let top = (0..BINS)
        .max_by(|a, b| hist[*a].total_cmp(&hist[*b]))
        .unwrap_or(0);
    let chosen = [(top + BINS - 1) % BINS, top, (top + 1) % BINS];
    let (mut sx, mut sy, mut ss, mut sl, mut sw) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &(h, s, l, w) in kept.iter().filter(|p| chosen.contains(&bin(p.0))) {
        let angle = h * std::f32::consts::TAU;
        sx += angle.cos() * w;
        sy += angle.sin() * w;
        ss += s * w;
        sl += l * w;
        sw += w;
    }
    if sw <= 0.0 {
        return None;
    }
    // Circular mean of hue, so 350° and 10° average to 0°, not 180°.
    let hue = (sy.atan2(sx) / std::f32::consts::TAU).rem_euclid(1.0);
    Some((hue, ss / sw, sl / sw))
}
