//! Bounded PDF stream decoding.
//!
//! A PDF stream is compressed, and a few kilobytes of input can describe gigabytes of
//! output. `lopdf`'s `decompressed_content()` — behind `get_page_content`,
//! `get_and_decode_page_content` and `get_plain_content` — buffers whatever the stream
//! expands to, so a hostile or simply broken PDF is a memory bomb: the process follows
//! the *stream's* idea of how much memory to use rather than its own.
//!
//! This module decodes the filters RayRAG actually meets in text extraction
//! (`FlateDecode`, `ASCIIHexDecode`, `ASCII85Decode`, and the PNG predictors that ride
//! along with deflate) with a hard output cap, and refuses anything else loudly instead
//! of handing it to an unbounded decoder. The cap is
//! `RAYRAG_PDF_STREAM_LIMIT_BYTES` (default 64 MiB).

use crate::Result;
use lopdf::{Dictionary, Object, Stream};
use std::io::Read;

/// Largest decompressed stream RayRAG will materialise.
pub const DEFAULT_PDF_STREAM_BYTES: u64 = 64 << 20;

/// Output cap for one decoded PDF stream (`RAYRAG_PDF_STREAM_LIMIT_BYTES`).
pub fn stream_limit_bytes() -> u64 {
    std::env::var("RAYRAG_PDF_STREAM_LIMIT_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1024 * 1024)
        .unwrap_or(DEFAULT_PDF_STREAM_BYTES)
}

/// Decode a stream to plain bytes, refusing more than `limit` bytes of output.
pub fn plain_content_limited(stream: &Stream, limit: u64, what: &str) -> Result<Vec<u8>> {
    let filters = stream.filters().unwrap_or_default();
    let params = stream
        .dict
        .get(b"DecodeParms")
        .or_else(|_| stream.dict.get(b"DP"))
        .ok()
        .and_then(|object| object.as_dict().ok());

    let mut content = stream.content.clone();
    for filter in &filters {
        content = match filter.as_str() {
            "FlateDecode" | "Fl" => {
                let decoded = inflate_limited(&content, limit, what)?;
                apply_predictor(decoded, params, limit, what)?
            }
            "ASCIIHexDecode" | "AHx" => decode_ascii_hex(&content, limit, what)?,
            "ASCII85Decode" | "A85" => decode_ascii85(&content, limit, what)?,
            other => {
                anyhow::bail!(
                    "{what} uses the '{other}' filter, which this build does not decode with a size cap"
                );
            }
        };
    }
    if content.len() as u64 > limit {
        anyhow::bail!(
            "{what} expands to {} bytes, above the {} MiB safety limit",
            content.len(),
            limit / (1024 * 1024)
        );
    }
    Ok(content)
}

/// Inflate with a hard output ceiling: the decoder stops as soon as it produced more
/// than `limit`, so a zip-bomb style stream never becomes an allocation.
fn inflate_limited(input: &[u8], limit: u64, what: &str) -> Result<Vec<u8>> {
    let mut decoder = flate2::read::ZlibDecoder::new(input).take(limit + 1);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| anyhow::anyhow!("{what} could not be inflated: {error}"))?;
    if output.len() as u64 > limit {
        anyhow::bail!(
            "{what} inflates past the {} MiB safety limit",
            limit / (1024 * 1024)
        );
    }
    Ok(output)
}

/// `DecodeParms` predictor handling (PNG predictors 10-15 plus TIFF predictor 2).
fn apply_predictor(
    data: Vec<u8>,
    params: Option<&Dictionary>,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let Some(params) = params else {
        return Ok(data);
    };
    let predictor = params
        .get(b"Predictor")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(1);
    if predictor <= 1 {
        return Ok(data);
    }
    let columns = params
        .get(b"Columns")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(1)
        .clamp(1, 64 * 1024) as usize;
    let colors = params
        .get(b"Colors")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(1)
        .clamp(1, 32) as usize;
    let bits = params
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(8)
        .clamp(1, 16) as usize;
    let bytes_per_pixel = ((colors * bits) + 7) / 8;
    let row_len = ((colors * bits * columns) + 7) / 8;
    if row_len == 0 {
        return Ok(data);
    }

    if predictor == 2 {
        // TIFF predictor: each sample is the delta from the one before it.
        if bits != 8 {
            anyhow::bail!("{what} uses TIFF prediction with {bits} bits per component");
        }
        let mut out = data;
        for row in out.chunks_mut(row_len) {
            for index in bytes_per_pixel..row.len() {
                row[index] = row[index].wrapping_add(row[index - bytes_per_pixel]);
            }
        }
        return Ok(out);
    }

    // PNG predictors: every row is prefixed with its filter type byte.
    let stride = row_len + 1;
    let rows = data.len() / stride;
    if rows * stride != data.len() {
        anyhow::bail!("{what} has a truncated predictor row");
    }
    let mut out = Vec::with_capacity(rows * row_len);
    if out.capacity() as u64 > limit {
        anyhow::bail!(
            "{what} predicts more than the {} MiB safety limit",
            limit / (1024 * 1024)
        );
    }
    let mut previous = vec![0u8; row_len];
    for row in data.chunks(stride) {
        let filter = row[0];
        let raw = &row[1..];
        let mut current = raw.to_vec();
        match filter {
            0 => {}
            1 => {
                for index in bytes_per_pixel..row_len {
                    current[index] = current[index].wrapping_add(current[index - bytes_per_pixel]);
                }
            }
            2 => {
                for index in 0..row_len {
                    current[index] = current[index].wrapping_add(previous[index]);
                }
            }
            3 => {
                for index in 0..row_len {
                    let left = if index >= bytes_per_pixel {
                        current[index - bytes_per_pixel] as u16
                    } else {
                        0
                    };
                    let up = previous[index] as u16;
                    current[index] = current[index].wrapping_add(((left + up) / 2) as u8);
                }
            }
            4 => {
                for index in 0..row_len {
                    let left = if index >= bytes_per_pixel {
                        current[index - bytes_per_pixel] as i16
                    } else {
                        0
                    };
                    let up = previous[index] as i16;
                    let up_left = if index >= bytes_per_pixel {
                        previous[index - bytes_per_pixel] as i16
                    } else {
                        0
                    };
                    let base = left + up - up_left;
                    let pa = (base - left).abs();
                    let pb = (base - up).abs();
                    let pc = (base - up_left).abs();
                    let value = if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        up_left
                    };
                    current[index] = current[index].wrapping_add(value as u8);
                }
            }
            other => anyhow::bail!("{what} uses unknown PNG predictor {other}"),
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    Ok(out)
}

/// Bounded ASCIIHex decoder.
fn decode_ascii_hex(input: &[u8], limit: u64, what: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    for &byte in input {
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            b'>' => break,
            other => anyhow::bail!("{what} has an invalid ASCIIHex byte 0x{other:02x}"),
        };
        match high.take() {
            Some(previous) => out.push((previous << 4) | value),
            None => high = Some(value),
        }
        if out.len() as u64 > limit {
            anyhow::bail!(
                "{what} expands past the {} MiB safety limit",
                limit / (1024 * 1024)
            );
        }
    }
    if let Some(previous) = high {
        out.push(previous << 4);
    }
    Ok(out)
}

/// Bounded ASCII85 decoder.
fn decode_ascii85(input: &[u8], limit: u64, what: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut group = [0u8; 5];
    let mut count = 0usize;
    let mut index = 0usize;
    let mut bytes = input;
    if bytes.starts_with(b"<~") {
        bytes = &bytes[2..];
    }
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        match byte {
            b'~' => break,
            b' ' | b'\n' | b'\r' | b'\t' | b'\0' | b'\x0c' => continue,
            b'z' if count == 0 => {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
            b'!'..=b'u' => {
                group[count] = byte - b'!';
                count += 1;
                if count == 5 {
                    let value = group.iter().fold(0u32, |acc, digit| {
                        acc.wrapping_mul(85).wrapping_add(*digit as u32)
                    });
                    out.extend_from_slice(&value.to_be_bytes());
                    count = 0;
                }
            }
            other => anyhow::bail!("{what} has an invalid ASCII85 byte 0x{other:02x}"),
        }
        if out.len() as u64 > limit {
            anyhow::bail!(
                "{what} expands past the {} MiB safety limit",
                limit / (1024 * 1024)
            );
        }
    }
    if count > 0 {
        let mut padded = [b'u'; 5];
        padded[..count].copy_from_slice(&group[..count]);
        let value = padded.iter().fold(0u32, |acc, digit| {
            acc.wrapping_mul(85).wrapping_add((*digit - b'!') as u32)
        });
        out.extend_from_slice(&value.to_be_bytes()[..count - 1]);
    }
    Ok(out)
}

/// Plain bytes of a page's content stream(s), decoded with a cap.
///
/// Replaces `Document::get_page_content`, which concatenates
/// `decompressed_content()` for every content stream without a ceiling.
pub fn page_content_limited(
    doc: &lopdf::Document,
    page_id: lopdf::ObjectId,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    for object_id in doc.get_page_contents(page_id) {
        let Ok(stream) = doc.get_object(object_id).and_then(Object::as_stream) else {
            continue;
        };
        let remaining = limit.saturating_sub(content.len() as u64);
        if remaining == 0 {
            anyhow::bail!(
                "{what} reaches the {} MiB safety limit",
                limit / (1024 * 1024)
            );
        }
        content.extend_from_slice(&plain_content_limited(stream, remaining, what)?);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;

    fn stream_with(content: Vec<u8>, filter: &str) -> Stream {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(filter.as_bytes().to_vec()));
        Stream::new(dict, content)
    }

    #[test]
    fn protected_streams_are_refused_instead_of_expanded() {
        // 64 MiB of zeros deflates to a few kilobytes: the classic stream bomb.
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        use std::io::Write;
        encoder.write_all(&vec![0u8; 64 * 1024 * 1024]).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(
            compressed.len() < 200 * 1024,
            "the bomb should be small on the wire"
        );

        let stream = stream_with(compressed, "FlateDecode");
        let error = plain_content_limited(&stream, 1 << 20, "test stream")
            .unwrap_err()
            .to_string();
        assert!(error.contains("safety limit"), "{error}");
    }

    #[test]
    fn ordinary_flate_streams_decode_whole() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        encoder.write_all(b"BT /F1 12 Tf (hello) Tj ET").unwrap();
        let compressed = encoder.finish().unwrap();
        let stream = stream_with(compressed, "FlateDecode");
        let decoded = plain_content_limited(&stream, 1 << 20, "test stream").unwrap();
        assert_eq!(decoded, b"BT /F1 12 Tf (hello) Tj ET");
    }

    #[test]
    fn unfiltered_streams_pass_through() {
        let stream = stream_with(b"plain content".to_vec(), "FlateDecode");
        // No filter in the dictionary at all.
        let raw = Stream::new(Dictionary::new(), b"raw".to_vec());
        assert_eq!(plain_content_limited(&raw, 1 << 20, "raw").unwrap(), b"raw");
        drop(stream);
    }

    #[test]
    fn png_predictor_rows_are_reconstructed() {
        // Two rows of one byte-per-pixel data with the "sub" filter (type 1).
        let data = vec![1u8, 5, 7, 1, 3, 4];
        let mut params = Dictionary::new();
        params.set("Predictor", 15);
        params.set("Columns", 2);
        params.set("Colors", 1);
        params.set("BitsPerComponent", 8);
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        dict.set("DecodeParms", Object::Dictionary(params));
        // Body is the raw predictor rows; feed them through an identity inflate.
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        encoder.write_all(&data).unwrap();
        let stream = Stream::new(dict, encoder.finish().unwrap());
        let decoded = plain_content_limited(&stream, 1 << 20, "predictor").unwrap();
        assert_eq!(decoded, vec![5, 12, 3, 7]);
    }

    #[test]
    fn exotic_filters_are_reported_rather_than_decoded_unbounded() {
        let stream = stream_with(vec![1, 2, 3], "LZWDecode");
        let error = plain_content_limited(&stream, 1 << 20, "test stream")
            .unwrap_err()
            .to_string();
        assert!(error.contains("'LZWDecode' filter"), "{error}");
    }
}
