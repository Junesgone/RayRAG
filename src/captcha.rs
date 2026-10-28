//! The image captcha in front of the password-reset flow.
//!
//! Upstream `user_api.py` (`/auth/password/forgot/captcha`) renders a captcha with Pillow, caches the
//! text for 60 seconds and refuses to send a reset code until the caller echoes it back. RayRAG had the
//! OTP half of that flow and not the captcha half, so anyone could ask for reset codes for arbitrary
//! addresses as fast as they liked. This module supplies the missing half in Rust: the image is drawn
//! from a built-in 5x7 glyph table (no font file, nothing to install), and the answer is held in memory
//! with the same 60-second life and a single use.
//!
//! The image is a real PNG. Upstream labels its (PNG-encoded) bytes `image/JPEG`; RayRAG returns
//! `image/png`, which is what the bytes actually are.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Characters in a captcha, matching upstream `OTP_LENGTH`.
pub const CAPTCHA_LENGTH: usize = 4;
/// How long an answer stays valid, matching upstream's `REDIS_CONN.set(..., 60)`.
pub const CAPTCHA_TTL_SECONDS: u64 = 60;
/// Image size, matching upstream's `ImageCaptcha(width=300, height=120)`.
pub const IMAGE_WIDTH: u32 = 300;
pub const IMAGE_HEIGHT: u32 = 120;

const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The 5x7 glyph table. `#` is ink; everything else is background. Digits and letters are drawn from
/// one table so the renderer needs no font file.
fn glyph(character: char) -> Option<[&'static str; 7]> {
    let rows = match character.to_ascii_uppercase() {
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'B' => [
            "11110", "10001", "10001", "11110", "10001", "10001", "11110",
        ],
        'C' => [
            "01110", "10001", "10000", "10000", "10000", "10001", "01110",
        ],
        'D' => [
            "11110", "10001", "10001", "10001", "10001", "10001", "11110",
        ],
        'E' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ],
        'F' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "10000",
        ],
        'G' => [
            "01110", "10001", "10000", "10111", "10001", "10001", "01110",
        ],
        'H' => [
            "10001", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'I' => [
            "01110", "00100", "00100", "00100", "00100", "00100", "01110",
        ],
        'J' => [
            "00111", "00010", "00010", "00010", "00010", "10010", "01100",
        ],
        'K' => [
            "10001", "10010", "10100", "11000", "10100", "10010", "10001",
        ],
        'L' => [
            "10000", "10000", "10000", "10000", "10000", "10000", "11111",
        ],
        'M' => [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'P' => [
            "11110", "10001", "10001", "11110", "10000", "10000", "10000",
        ],
        'Q' => [
            "01110", "10001", "10001", "10001", "10101", "10010", "01101",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        'T' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        'U' => [
            "10001", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'V' => [
            "10001", "10001", "10001", "10001", "10001", "01010", "00100",
        ],
        'W' => [
            "10001", "10001", "10001", "10101", "10101", "11011", "10001",
        ],
        'X' => [
            "10001", "10001", "01010", "00100", "01010", "10001", "10001",
        ],
        'Y' => [
            "10001", "10001", "01010", "00100", "00100", "00100", "00100",
        ],
        'Z' => [
            "11111", "00001", "00010", "00100", "01000", "10000", "11111",
        ],
        '0' => [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11111", "00010", "00100", "00010", "00001", "10001", "01110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "11110", "00001", "00001", "10001", "01110",
        ],
        '6' => [
            "00110", "01000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00010", "01100",
        ],
        _ => return None,
    };
    Some(rows)
}

/// A deterministic pseudo-random source. Seeding it from the answer means the same answer always draws
/// the same picture, which is what makes the renderer testable; the noise still differs between codes.
struct Noise(u64);

impl Noise {
    fn new(seed: u64) -> Self {
        Noise(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 17
    }

    fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            0
        } else {
            (self.next() % u64::from(bound)) as u32
        }
    }
}

fn seed_for(code: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in code.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Draw the answer into a PNG. Every character in the code must have a glyph; anything else is a
/// programming error rather than something to skip silently.
pub fn render_png(code: &str) -> anyhow::Result<Vec<u8>> {
    if code.is_empty() {
        anyhow::bail!("a captcha needs at least one character");
    }
    let mut image =
        image::RgbImage::from_pixel(IMAGE_WIDTH, IMAGE_HEIGHT, image::Rgb([246, 247, 250]));
    let mut noise = Noise::new(seed_for(code));

    // Faint dotted grid, so the background is not a flat colour an OCR filter could threshold away.
    for _ in 0..(IMAGE_WIDTH * IMAGE_HEIGHT / 90) {
        let x = noise.below(IMAGE_WIDTH);
        let y = noise.below(IMAGE_HEIGHT);
        let shade = 215 + noise.below(20) as u8;
        image.put_pixel(x, y, image::Rgb([shade, shade, shade]));
    }

    let characters: Vec<char> = code.chars().collect();
    // Scale the glyphs to fill the width, leaving a margin either side.
    let slot = (IMAGE_WIDTH - 40) / characters.len() as u32;
    let scale = (slot / 6).clamp(2, 14);
    let glyph_width = 5 * scale;
    let glyph_height = 7 * scale;

    for (index, character) in characters.iter().enumerate() {
        let Some(rows) = glyph(*character) else {
            anyhow::bail!("no glyph for {character:?} in the captcha font");
        };
        let jitter_x = noise.below(scale) as i32 - (scale as i32 / 2);
        let jitter_y = noise.below(scale * 2) as i32 - scale as i32;
        let origin_x = 20 + index as u32 * slot + (slot.saturating_sub(glyph_width)) / 2;
        let origin_y = (IMAGE_HEIGHT.saturating_sub(glyph_height)) / 2;
        let ink = image::Rgb([
            20 + noise.below(80) as u8,
            30 + noise.below(70) as u8,
            60 + noise.below(80) as u8,
        ]);
        for (row_index, row) in rows.iter().enumerate() {
            for (column_index, cell) in row.chars().enumerate() {
                if cell != '1' {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x =
                            origin_x as i32 + (column_index as u32 * scale + dx) as i32 + jitter_x;
                        let y = origin_y as i32 + (row_index as u32 * scale + dy) as i32 + jitter_y;
                        if x >= 0 && y >= 0 && (x as u32) < IMAGE_WIDTH && (y as u32) < IMAGE_HEIGHT
                        {
                            image.put_pixel(x as u32, y as u32, ink);
                        }
                    }
                }
            }
        }
    }

    // Two crossing strokes over the text, the usual captcha guard against trivial OCR.
    for offset in 0..IMAGE_HEIGHT {
        let x = (offset * IMAGE_WIDTH / IMAGE_HEIGHT) as i32;
        let y = offset as i32;
        for thickness in 0..2 {
            let px = x + thickness;
            if px >= 0 && (px as u32) < IMAGE_WIDTH {
                image.put_pixel(px as u32, y as u32, image::Rgb([150, 160, 190]));
            }
        }
    }
    for offset in 0..IMAGE_HEIGHT {
        let x = IMAGE_WIDTH as i32 - 1 - (offset * IMAGE_WIDTH / IMAGE_HEIGHT) as i32;
        let y = offset as i32;
        if x >= 0 && (x as u32) < IMAGE_WIDTH {
            image.put_pixel(x as u32, y as u32, image::Rgb([170, 175, 200]));
        }
    }

    let mut bytes: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|error| anyhow::anyhow!("the captcha image could not be encoded: {error}"))?;
    Ok(bytes)
}

/// Generate an answer: four characters from the same alphabet upstream uses.
pub fn generate() -> String {
    let mut noise = Noise::new(seed_for(&format!(
        "{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    )));
    (0..CAPTCHA_LENGTH)
        .map(|_| {
            let index = noise.below(ALPHABET.len() as u32) as usize;
            ALPHABET[index] as char
        })
        .collect()
}

#[derive(Clone)]
struct Entry {
    code: String,
    issued_ms: u64,
}

/// Answers issued per email address, with upstream's 60-second life and a single use.
pub struct CaptchaStore {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for CaptchaStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptchaStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The process-wide store the endpoint and the OTP request share.
    pub fn shared() -> &'static CaptchaStore {
        static STORE: OnceLock<CaptchaStore> = OnceLock::new();
        STORE.get_or_init(CaptchaStore::new)
    }

    /// Issue (and capitalise) an answer for `email`, replacing any previous one.
    pub fn issue_at(&self, email: &str, code: &str, now_ms: u64) -> String {
        let code = code.to_ascii_uppercase();
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, entry| !expired(entry, now_ms));
        entries.insert(
            email.to_ascii_lowercase(),
            Entry {
                code: code.clone(),
                issued_ms: now_ms,
            },
        );
        code
    }

    /// Issue a freshly generated answer for `email`.
    pub fn issue(&self, email: &str) -> String {
        let code = generate();
        self.issue_at(email, &code, now_ms())
    }

    /// Check an answer. A correct answer is consumed; a wrong one leaves the question standing so the
    /// user can retype what the image shows.
    pub fn verify_at(&self, email: &str, answer: &str, now_ms: u64) -> bool {
        let key = email.to_ascii_lowercase();
        let mut entries = self.entries.lock().unwrap();
        let Some(entry) = entries.get(&key) else {
            return false;
        };
        if expired(entry, now_ms) {
            entries.remove(&key);
            return false;
        }
        let matches = entry.code.eq_ignore_ascii_case(answer.trim());
        if matches {
            entries.remove(&key);
        }
        matches
    }

    /// Check an answer against the wall clock.
    pub fn verify(&self, email: &str, answer: &str) -> bool {
        self.verify_at(email, answer, now_ms())
    }

    /// Whether a question is outstanding for this address, without consuming it.
    pub fn has_pending(&self, email: &str) -> bool {
        self.has_pending_at(email, now_ms())
    }

    /// The same question asked against a caller-supplied clock.
    pub fn has_pending_at(&self, email: &str, now_ms: u64) -> bool {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, entry| !expired(entry, now_ms));
        entries.contains_key(&email.to_ascii_lowercase())
    }
}

fn expired(entry: &Entry, now_ms: u64) -> bool {
    now_ms.saturating_sub(entry.issued_ms) >= CAPTCHA_TTL_SECONDS * 1000
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_in_the_alphabet_can_be_drawn() {
        assert_eq!(ALPHABET.len(), 36);
        for byte in ALPHABET {
            let character = *byte as char;
            let rows = glyph(character).unwrap_or_else(|| panic!("no glyph for {character}"));
            assert_eq!(rows.len(), 7);
            for row in rows {
                assert_eq!(
                    row.len(),
                    5,
                    "{character} row {row:?} is not five cells wide"
                );
                assert!(row.chars().all(|cell| cell == '0' || cell == '1'));
            }
            // A blank glyph would render nothing and leave the answer unreadable.
            let ink: usize = rows
                .iter()
                .map(|row| row.chars().filter(|cell| *cell == '1').count())
                .sum();
            assert!(ink >= 8, "{character} has only {ink} ink cells");
        }
        // Lower case is drawn as the upper-case glyph, and unknown characters are refused.
        assert!(glyph('a').is_some());
        assert!(glyph('-').is_none());
        assert!(glyph('中').is_none());
    }

    #[test]
    fn the_image_is_a_real_png_of_the_expected_size() {
        let png = render_png("A7K2").unwrap();
        // PNG signature, so this is not a placeholder string pretending to be an image.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        let decoded = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (IMAGE_WIDTH, IMAGE_HEIGHT));
        // The glyphs put ink on the canvas, and the canvas is not one flat colour.
        let distinct: std::collections::HashSet<[u8; 3]> = decoded
            .pixels()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        assert!(
            distinct.len() > 20,
            "only {} distinct colours",
            distinct.len()
        );
        assert!(
            decoded
                .pixels()
                .any(|pixel| pixel[0] < 100 && pixel[2] < 140),
            "no dark ink was drawn"
        );
        // The noise is seeded from the answer, so one answer always draws one picture.
        assert_eq!(png, render_png("A7K2").unwrap());
        assert_ne!(png, render_png("A7K3").unwrap());
        assert!(render_png("").is_err());
        // A code that cannot be drawn is an error, not a blank image.
        assert!(render_png("A-").is_err());
    }

    #[test]
    fn an_answer_is_case_insensitive_single_use_and_short_lived() {
        let store = CaptchaStore::new();
        let issued = store.issue_at("Admin@Example.com", "abcd", 1_000);
        assert_eq!(issued, "ABCD", "answers are stored upper case");
        assert!(
            store.has_pending_at("admin@example.com", 1_500),
            "addresses match case-insensitively"
        );
        // A wrong answer leaves the question standing.
        assert!(!store.verify_at("admin@example.com", "ZZZZ", 2_000));
        assert!(store.has_pending_at("admin@example.com", 2_100));
        // The right answer, in any case, with surrounding spaces, is accepted once.
        assert!(store.verify_at("admin@example.com", " abcd ", 3_000));
        assert!(
            !store.verify_at("admin@example.com", "abcd", 3_001),
            "a used answer is spent"
        );
        assert!(!store.has_pending_at("admin@example.com", 3_100));
        // Another address never inherits the answer.
        store.issue_at("someone@example.com", "WXYZ", 1_000);
        assert!(!store.verify_at("other@example.com", "WXYZ", 1_100));
        // And the answer expires on time, not later.
        assert!(store.has_pending_at(
            "someone@example.com",
            1_000 + CAPTCHA_TTL_SECONDS * 1000 - 1
        ));
        assert!(!store.verify_at(
            "someone@example.com",
            "WXYZ",
            1_000 + CAPTCHA_TTL_SECONDS * 1000
        ));
        // The wall-clock helpers agree with the explicit-clock ones for a fresh question.
        let fresh = CaptchaStore::new();
        fresh.issue("now@example.com");
        assert!(fresh.has_pending("NOW@example.com"));
        assert!(!fresh.has_pending("nobody@example.com"));
    }

    #[test]
    fn generated_answers_use_the_alphabet_and_the_documented_length() {
        for _ in 0..50 {
            let code = generate();
            assert_eq!(code.chars().count(), CAPTCHA_LENGTH);
            assert!(
                code.chars()
                    .all(|character| ALPHABET.contains(&(character as u8)))
            );
        }
        // Two answers in a row are not the same picture of the same text.
        let answers: std::collections::HashSet<String> = (0..20).map(|_| generate()).collect();
        assert!(
            answers.len() > 1,
            "the generator returned one value twenty times"
        );
    }
}
