//! Bounded PDF stream decoding.
//!
//! A PDF stream is compressed, and a few kilobytes of input can describe gigabytes of
//! output. `lopdf`'s `decompressed_content()` — behind `get_page_content`,
//! `get_and_decode_page_content` and `get_plain_content` — buffers whatever the stream
//! expands to, so a hostile or simply broken PDF is a memory bomb: the process follows
//! the *stream's* idea of how much memory to use rather than its own.
//!
//! This module decodes the filters RayRAG actually meets in text extraction
//! (`FlateDecode`, `LZWDecode`, `RunLengthDecode`, `ASCIIHexDecode`, `ASCII85Decode`,
//! and the PNG predictors that ride
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

/// Load metadata streams with the same explicit bound used for page parsing.
pub fn load_document(data: &[u8]) -> Result<lopdf::Document> {
    load_document_limited(data, stream_limit_bytes())
}

fn load_document_limited(data: &[u8], limit: u64) -> Result<lopdf::Document> {
    let limit = usize::try_from(limit)
        .map_err(|_| anyhow::anyhow!("PDF stream limit exceeds this platform address space"))?;
    lopdf::Document::load_mem_with_options(
        data, lopdf::LoadOptions::with_max_decompressed_size(limit),
    ).map_err(|error| anyhow::anyhow!(
        "PDF loading failed under RAYRAG_PDF_STREAM_LIMIT_BYTES={limit}: {error}. Repair or export an unencrypted PDF and upload it again. For a trusted large document, increase the PDF resource limit in deployment setup or the environment file."
    ))
}

/// Decode a stream to plain bytes, refusing more than `limit` bytes of output.
pub fn plain_content_limited(stream: &Stream, limit: u64, what: &str) -> Result<Vec<u8>> {
    decode_stream(None, stream, limit, what)
}

/// Resolve indirect filter settings through the same document as the stream.
pub fn plain_content_limited_in(
    doc: &lopdf::Document,
    stream: &Stream,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>> {
    decode_stream(Some(doc), stream, limit, what)
}

fn resolve_stream_value<'a>(
    doc: Option<&'a lopdf::Document>,
    mut value: &'a Object,
    what: &str,
) -> Result<&'a Object> {
    for _ in 0..32 {
        let Object::Reference(id) = value else {
            return Ok(value);
        };
        value = doc.and_then(|doc| doc.objects.get(id)).ok_or_else(|| {
            anyhow::anyhow!("{what} reference {} {} cannot be resolved", id.0, id.1)
        })?;
    }
    anyhow::ensure!(
        !matches!(value, Object::Reference(_)),
        "{what} reference chain is cyclic or exceeds 32 levels"
    );
    Ok(value)
}

fn decode_stream(
    doc: Option<&lopdf::Document>,
    stream: &Stream,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let filter = stream
        .dict
        .get(b"Filter")
        .ok()
        .map(|value| resolve_stream_value(doc, value, what))
        .transpose()?;
    let filters = match filter {
        None | Some(Object::Null) => Vec::new(),
        Some(Object::Name(name)) => vec![name.as_slice()],
        Some(Object::Array(names)) => names
            .iter()
            .map(|value| {
                resolve_stream_value(doc, value, what)?
                    .as_name()
                    .map_err(|_| anyhow::anyhow!("{what} Filter entry must be a name"))
            })
            .collect::<Result<Vec<_>>>()?,
        _ => anyhow::bail!("{what} Filter must be a name or array"),
    };
    let params = stream
        .dict
        .get(b"DecodeParms")
        .or_else(|_| stream.dict.get(b"DP"))
        .ok()
        .map(|value| resolve_stream_value(doc, value, what))
        .transpose()?;
    match params {
        Some(Object::Array(values)) => anyhow::ensure!(
            values.len() == filters.len(),
            "{what} DecodeParms length does not match Filter count"
        ),
        Some(Object::Dictionary(_)) => anyhow::ensure!(
            filters.len() == 1,
            "{what} DecodeParms dictionary requires a single filter"
        ),
        None | Some(Object::Null) => {}
        _ => anyhow::bail!("{what} DecodeParms must be a dictionary or array"),
    }
    if filters.is_empty() {
        anyhow::ensure!(
            stream.content.len() as u64 <= limit,
            "{what} exceeds the safety limit before copying"
        );
    }
    let mut content = stream.content.clone();
    for (index, filter) in filters.iter().enumerate() {
        let value = match params {
            Some(Object::Array(values)) => values.get(index),
            other => other,
        };
        let value = value
            .map(|value| resolve_stream_value(doc, value, what))
            .transpose()?;
        let params = match value {
            None | Some(Object::Null) => None,
            Some(Object::Dictionary(dictionary)) => Some(dictionary),
            _ => anyhow::bail!("{what} DecodeParms entry {index} must be a dictionary or null"),
        };
        content = match *filter {
            b"FlateDecode" | b"Fl" => {
                let decoded = inflate_limited(&content, limit, what)?;
                apply_predictor(doc, decoded, params, limit, what)?
            }
            b"LZWDecode" | b"LZW" => {
                let early_change = params
                    .and_then(|params| params.get(b"EarlyChange").ok())
                    .map(|value| resolve_stream_value(doc, value, what))
                    .transpose()?;
                let early_change = match early_change {
                    None | Some(Object::Null) | Some(Object::Integer(1)) => true,
                    Some(Object::Integer(0)) => false,
                    _ => anyhow::bail!("{what} LZW EarlyChange must be 0 or 1"),
                };
                let decoded = decode_lzw(&content, early_change, limit, what)?;
                apply_predictor(doc, decoded, params, limit, what)?
            }
            b"RunLengthDecode" | b"RL" => decode_run_length(&content, limit, what)?,
            b"ASCIIHexDecode" | b"AHx" => decode_ascii_hex(&content, limit, what)?,
            b"ASCII85Decode" | b"A85" => decode_ascii85(&content, limit, what)?,
            other => {
                let other = String::from_utf8_lossy(other);
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

/// Decode through a fixed scratch buffer and reject partial or over-limit output.
fn decode_lzw(input: &[u8], early_change: bool, limit: u64, what: &str) -> Result<Vec<u8>> {
    use weezl::{BitOrder, LzwStatus, decode::Decoder};
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Decoder::new(BitOrder::Msb, 8)
    };
    let mut input = input;
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let result = decoder.decode_bytes(input, &mut buffer);
        let next = (output.len() as u64).saturating_add(result.consumed_out as u64);
        anyhow::ensure!(next <= limit, "{what} LZW output exceeds the safety limit");
        output.extend_from_slice(&buffer[..result.consumed_out]);
        input = &input[result.consumed_in..];
        match result.status {
            Ok(LzwStatus::Done) => return Ok(output),
            Ok(LzwStatus::NoProgress) => {
                anyhow::bail!("{what} LZW stream is truncated or has no end code")
            }
            Err(error) => anyhow::bail!("{what} LZW decoding failed: {error}"),
            Ok(LzwStatus::Ok) => anyhow::ensure!(
                result.consumed_in > 0 || result.consumed_out > 0,
                "{what} LZW decoder made no progress"
            ),
        }
    }
}

fn decode_run_length(input: &[u8], limit: u64, what: &str) -> Result<Vec<u8>> {
    let mut offset = 0;
    let mut output = Vec::new();
    while let Some(&length) = input.get(offset) {
        offset += 1;
        if length == 128 {
            return Ok(output);
        }
        let count = if length <= 127 {
            usize::from(length) + 1
        } else {
            257 - usize::from(length)
        };
        anyhow::ensure!(
            (output.len() as u64).saturating_add(count as u64) <= limit,
            "{what} RunLength output exceeds the safety limit"
        );
        if length <= 127 {
            let literal = input
                .get(offset..offset + count)
                .ok_or_else(|| anyhow::anyhow!("{what} RunLength literal is truncated"))?;
            output.extend_from_slice(literal);
            offset += count;
        } else {
            let byte = *input
                .get(offset)
                .ok_or_else(|| anyhow::anyhow!("{what} RunLength repeat is truncated"))?;
            output.resize(output.len() + count, byte);
            offset += 1;
        }
    }
    anyhow::bail!("{what} RunLength stream has no end marker")
}

/// Inflate with a hard output ceiling: the decoder stops as soon as it produced more
/// than `limit`, so a zip-bomb style stream never becomes an allocation.
///
/// Some PDF writers emit raw DEFLATE bytes under /FlateDecode instead of a zlib
/// wrapper. When the data does not start with a valid zlib header, retry as raw
/// DEFLATE; a corrupt stream that does start with a zlib header stays an error.
fn inflate_limited(input: &[u8], limit: u64, what: &str) -> Result<Vec<u8>> {
    let start = input
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(input.len());
    let candidate = &input[start..];
    let zlib_header = candidate.len() >= 2
        && (candidate[0] & 0x0f) == 0x08
        && ((u16::from(candidate[0]) << 8 | u16::from(candidate[1])) % 31) == 0;
    let zlib = inflate_reader(flate2::read::ZlibDecoder::new(candidate), limit, what);
    match zlib {
        Ok(output) => Ok(output),
        Err(zlib_error) if !zlib_header => {
            let raw = inflate_reader(flate2::read::DeflateDecoder::new(candidate), limit, what);
            match raw {
                Ok(output) if !output.is_empty() => Ok(output),
                Ok(_) => anyhow::bail!(
                    "{what} could not be inflated: zlib: {zlib_error}; raw deflate produced no output"
                ),
                Err(raw_error) => anyhow::bail!(
                    "{what} could not be inflated: zlib: {zlib_error}; raw deflate: {raw_error}"
                ),
            }
        }
        Err(error) => anyhow::bail!("{what} could not be inflated: {error}"),
    }
}

fn inflate_reader<R: std::io::Read>(
    reader: R,
    limit: u64,
    what: &str,
) -> std::result::Result<Vec<u8>, std::io::Error> {
    let mut decoder = reader.take(limit + 1);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output)?;
    if output.len() as u64 > limit {
        return Err(std::io::Error::other(format!(
            "{what} inflates past the {} MiB safety limit",
            limit / (1024 * 1024)
        )));
    }
    Ok(output)
}

/// `DecodeParms` predictor handling (PNG predictors 10-15 plus TIFF predictor 2).
fn apply_predictor(
    doc: Option<&lopdf::Document>,
    data: Vec<u8>,
    params: Option<&Dictionary>,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>> {
    let Some(params) = params else {
        return Ok(data);
    };
    let parameter = |key: &[u8], default| -> Result<u64> {
        let value = params
            .get(key)
            .ok()
            .map(|value| resolve_stream_value(doc, value, what))
            .transpose()?;
        match value {
            None | Some(Object::Null) => Ok(default),
            Some(value) => value
                .as_i64()
                .ok()
                .and_then(|value| u64::try_from(value).ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{what} predictor /{} must be a nonnegative integer",
                        String::from_utf8_lossy(key)
                    )
                }),
        }
    };
    let predictor = parameter(b"Predictor", 1)?;
    if predictor == 1 {
        return Ok(data);
    }
    anyhow::ensure!(
        predictor == 2 || (10..=15).contains(&predictor),
        "{what} uses invalid predictor {predictor}"
    );
    let columns = parameter(b"Columns", 1)?;
    let colors = parameter(b"Colors", 1)?;
    let bits = parameter(b"BitsPerComponent", 8)?;
    anyhow::ensure!(
        columns > 0 && colors > 0 && [1, 2, 4, 8, 16].contains(&bits),
        "{what} has invalid predictor geometry"
    );
    let pixel_bits = colors
        .checked_mul(bits)
        .ok_or_else(|| anyhow::anyhow!("{what} predictor dimensions overflow"))?;
    let row_bits = pixel_bits
        .checked_mul(columns)
        .ok_or_else(|| anyhow::anyhow!("{what} predictor dimensions overflow"))?;
    let bytes_per_pixel = usize::try_from(pixel_bits.div_ceil(8))
        .map_err(|_| anyhow::anyhow!("{what} predictor pixel exceeds platform range"))?;
    let row_len = row_bits.div_ceil(8);
    anyhow::ensure!(
        row_len <= limit,
        "{what} predictor row exceeds the safety limit"
    );
    let row_len = usize::try_from(row_len)
        .map_err(|_| anyhow::anyhow!("{what} predictor row exceeds platform range"))?;

    if predictor == 2 {
        // TIFF differences are per color component, with samples packed MSB first.
        anyhow::ensure!(
            data.len().is_multiple_of(row_len),
            "{what} has a truncated TIFF predictor row"
        );
        let samples = usize::try_from(row_bits / bits)
            .map_err(|_| anyhow::anyhow!("{what} TIFF sample count exceeds platform range"))?;
        let colors = usize::try_from(colors)
            .map_err(|_| anyhow::anyhow!("{what} TIFF color count exceeds platform range"))?;
        let bits = bits as usize;
        let mut out = data;
        for row in out.chunks_mut(row_len) {
            match bits {
                8 => {
                    for index in colors..samples {
                        row[index] = row[index].wrapping_add(row[index - colors]);
                    }
                }
                16 => {
                    for sample in colors..samples {
                        let index = sample * 2;
                        let previous = (sample - colors) * 2;
                        let delta = u16::from_be_bytes([row[index], row[index + 1]]);
                        let left = u16::from_be_bytes([row[previous], row[previous + 1]]);
                        row[index..index + 2]
                            .copy_from_slice(&delta.wrapping_add(left).to_be_bytes());
                    }
                }
                _ => {
                    let mask = (1u8 << bits) - 1;
                    for sample in colors..samples {
                        let bit = sample * bits;
                        let previous = (sample - colors) * bits;
                        let shift = 8 - bits - bit % 8;
                        let previous_shift = 8 - bits - previous % 8;
                        let delta = (row[bit / 8] >> shift) & mask;
                        let left = (row[previous / 8] >> previous_shift) & mask;
                        let value = delta.wrapping_add(left) & mask;
                        row[bit / 8] = (row[bit / 8] & !(mask << shift)) | (value << shift);
                    }
                }
            }
        }
        return Ok(out);
    }

    // PNG predictors: every row is prefixed with its filter type byte.
    let stride = row_len
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("{what} predictor stride overflows"))?;
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
        let mut padded = [84u8; 5];
        padded[..count].copy_from_slice(&group[..count]);
        let value = padded.iter().fold(0u32, |acc, digit| {
            acc.wrapping_mul(85).wrapping_add(*digit as u32)
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
    let page = doc
        .get_dictionary(page_id)
        .map_err(|error| anyhow::anyhow!("{what} page dictionary cannot be read: {error}"))?;
    let Some(contents) = page.get(b"Contents").ok() else {
        return Ok(Vec::new());
    };
    let contents = resolve_stream_value(Some(doc), contents, what)?;
    let streams = match contents {
        Object::Null => return Ok(Vec::new()),
        Object::Array(values) => values.as_slice(),
        other => std::slice::from_ref(other),
    };
    let mut content = Vec::new();
    for (index, object) in streams.iter().enumerate() {
        let object = resolve_stream_value(Some(doc), object, what)?;
        let stream = object
            .as_stream()
            .map_err(|_| anyhow::anyhow!("{what} Contents entry {index} is not a stream"))?;
        let remaining = limit.saturating_sub(content.len() as u64);
        let decoded = plain_content_limited_in(doc, stream, remaining, what)?;
        content.extend_from_slice(&decoded);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;
    use lopdf::dictionary;

    fn stream_with(content: Vec<u8>, filter: &str) -> Stream {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(filter.as_bytes().to_vec()));
        Stream::new(dict, content)
    }

    fn compressed(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn raw_deflate(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn raw_deflate_content_streams_are_accepted() {
        let raw = raw_deflate(b"raw deflate page content");
        let decoded = inflate_limited(&raw, 1024, "PDF page 1 content").unwrap();
        assert_eq!(decoded, b"raw deflate page content");
    }

    #[test]
    fn leading_whitespace_before_zlib_header_is_tolerated() {
        let mut data = b"\r\n  ".to_vec();
        data.extend_from_slice(&compressed(b"zlib content after whitespace"));
        let decoded = inflate_limited(&data, 1024, "PDF page 1 content").unwrap();
        assert_eq!(decoded, b"zlib content after whitespace");
    }

    #[test]
    fn corrupt_zlib_header_does_not_fall_back_to_garbage() {
        // Valid zlib header (78 9c) followed by garbage: raw DEFLATE must not
        // be attempted just because inflate failed.
        let data = [0x78, 0x9c, 0xff, 0xff, 0xff, 0xff];
        let error = inflate_limited(&data, 1024, "PDF page 1 content")
            .unwrap_err()
            .to_string();
        assert!(error.contains("could not be inflated"), "{error}");
    }

    #[test]
    fn metadata_object_streams_obey_loader_limit_and_can_be_retried() {
        use std::io::Write;
        let mut bytes = b"%PDF-1.5\n".to_vec();
        let root = bytes.len();
        write!(
            bytes,
            "1 0 obj << /Type /Catalog /Pages 2 0 R /MetadataValue 5 0 R >> endobj\n"
        )
        .unwrap();
        let pages = bytes.len();
        write!(
            bytes,
            "2 0 obj << /Type /Pages /Count 0 /Kids [] >> endobj\n"
        )
        .unwrap();
        let object_stream = bytes.len();
        let decoded = format!("5 0 << /Value ({}) >>", "x".repeat(4096));
        let content = compressed(decoded.as_bytes());
        write!(
            bytes,
            "3 0 obj << /Type /ObjStm /N 1 /First 4 /Filter /FlateDecode /Length {} >>\nstream\n",
            content.len()
        )
        .unwrap();
        bytes.extend_from_slice(&content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        let mut table = Vec::new();
        for number in 0..6 {
            let (kind, field, generation): (u8, u32, u16) = match number {
                0 => (0, 0, 65535),
                1 => (1, root as u32, 0),
                2 => (1, pages as u32, 0),
                3 => (1, object_stream as u32, 0),
                4 => (1, xref as u32, 0),
                _ => (2, 3, 0),
            };
            table.push(kind);
            table.extend_from_slice(&field.to_be_bytes());
            table.extend_from_slice(&generation.to_be_bytes());
        }
        write!(
            bytes,
            "4 0 obj << /Type /XRef /Size 6 /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
            table.len()
        )
        .unwrap();
        bytes.extend_from_slice(&table);
        write!(bytes, "\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").unwrap();
        let error = load_document_limited(&bytes, 512).unwrap_err().to_string();
        for detail in [
            "PDF loading failed",
            "RAYRAG_PDF_STREAM_LIMIT_BYTES=512",
            "upload it again",
        ] {
            assert!(error.contains(detail), "{error}");
        }
        let loaded = load_document_limited(&bytes, 8192).unwrap();
        assert_eq!(
            loaded
                .get_dictionary((5, 0))
                .unwrap()
                .get(b"Value")
                .unwrap()
                .as_str()
                .unwrap()
                .len(),
            4096
        );
    }

    #[test]
    fn compressed_cross_reference_stream_obeys_loader_limit() {
        use std::io::Write;
        let mut bytes = b"%PDF-1.5\n".to_vec();
        let root = bytes.len();
        write!(bytes, "1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n").unwrap();
        let pages = bytes.len();
        write!(
            bytes,
            "2 0 obj << /Type /Pages /Count 0 /Kids [] >> endobj\n"
        )
        .unwrap();
        let xref = bytes.len();
        let mut table = Vec::new();
        for number in 0..600 {
            let offset = match number {
                1 => root,
                2 => pages,
                3 => xref,
                _ => 0,
            };
            table.push(if offset == 0 { 0 } else { 1 });
            table.extend_from_slice(&(offset as u32).to_be_bytes());
            table.extend_from_slice(&0u16.to_be_bytes());
        }
        let content = compressed(&table);
        write!(bytes, "3 0 obj << /Type /XRef /Size 600 /W [1 4 2] /Root 1 0 R /Length {} /Filter /FlateDecode >>\nstream\n", content.len()).unwrap();
        bytes.extend_from_slice(&content);
        write!(bytes, "\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").unwrap();
        let error = load_document_limited(&bytes, 512).unwrap_err().to_string();
        assert!(
            error.contains("RAYRAG_PDF_STREAM_LIMIT_BYTES=512"),
            "{error}"
        );
        assert!(load_document_limited(&bytes, 8192).is_ok());
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
    fn ascii85_partial_groups_decode_without_overflow() {
        assert_eq!(
            decode_ascii85(b"87cURD_*#TDfTZ)+T~>", 1024, "ascii85 test").unwrap(),
            b"Hello, world!"
        );
    }

    #[test]
    fn indirect_parameter_reference_limit_accepts_exactly_32_hops() {
        for hops in [31, 32, 33] {
            let mut doc = lopdf::Document::with_version("1.7");
            let mut value = Object::Name(b"FlateDecode".to_vec());
            for _ in 0..hops {
                value = Object::Reference(doc.add_object(value));
            }
            let stream = Stream::new(dictionary! { "Filter" => value }, compressed(b"Body"));
            let result = plain_content_limited_in(&doc, &stream, 4096, "hop boundary");
            if hops <= 32 {
                assert_eq!(result.unwrap(), b"Body");
            } else {
                assert!(result.unwrap_err().to_string().contains("32 levels"));
            }
        }
    }

    #[test]
    fn indirect_filter_parameters_report_broken_and_cyclic_references() {
        for field in ["Filter", "DecodeParms"] {
            for cyclic in [false, true] {
                let mut doc = lopdf::Document::with_version("1.7");
                let id = doc.new_object_id();
                if cyclic {
                    doc.objects.insert(id, Object::Reference(id));
                }
                let mut stream = stream_with(compressed(b"Body"), "FlateDecode");
                stream.dict.set(field, id);
                let error = plain_content_limited_in(&doc, &stream, 4096, "PDF page 2")
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("PDF page 2") && error.contains("reference"),
                    "{error}"
                );
            }
        }
        let mut doc = lopdf::Document::with_version("1.7");
        let null = doc.add_object(Object::Null);
        let stream = Stream::new(
            dictionary! { "Filter" => null, "DecodeParms" => null },
            b"Body".to_vec(),
        );
        assert_eq!(
            plain_content_limited_in(&doc, &stream, 4, "null").unwrap(),
            b"Body"
        );
        assert!(
            plain_content_limited(&stream, 4, "standalone")
                .unwrap_err()
                .to_string()
                .contains("reference")
        );
    }

    #[test]
    fn filter_chain_uses_each_decode_parameters_entry() {
        let payload = compressed(&[1, 10, 10, 10, 2, 1, 1, 1]);
        let encoded = payload
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            + ">";
        let stream = Stream::new(
            lopdf::dictionary! {
                "Filter" => vec![Object::Name(b"ASCIIHexDecode".to_vec()), Object::Name(b"FlateDecode".to_vec())],
                "DecodeParms" => vec![Object::Null, Object::Dictionary(lopdf::dictionary! { "Predictor" => 15, "Columns" => 3 })],
            },
            encoded.into_bytes(),
        );
        assert_eq!(
            plain_content_limited(&stream, 4096, "chain").unwrap(),
            [10, 20, 30, 11, 21, 31]
        );
    }

    #[test]
    fn damaged_filter_parameters_and_predictor_geometry_are_visible_errors() {
        for dictionary in [
            lopdf::dictionary! { "Filter" => 17 },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => Vec::<Object>::new() },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => vec![Object::Integer(17)] },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => lopdf::dictionary! { "Predictor" => 3 } },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => lopdf::dictionary! { "Predictor" => "bad" } },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => lopdf::dictionary! { "Predictor" => 15, "Columns" => 0 } },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => lopdf::dictionary! { "Predictor" => 15, "Columns" => i64::MAX, "Colors" => 32, "BitsPerComponent" => 16 } },
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => lopdf::dictionary! { "Predictor" => 2, "Columns" => 3 } },
        ] {
            let stream = Stream::new(dictionary, compressed(&[1, 2]));
            let error = plain_content_limited(&stream, 4096, "test parameters")
                .unwrap_err()
                .to_string();
            assert!(error.contains("test parameters"), "{error}");
        }
    }

    #[test]
    fn tiff_predictor_packed_samples_preserve_padding_and_reset_each_row() {
        for (bits, encoded, decoded) in [
            (1, vec![0xf5], vec![0xb5]),
            (2, vec![0x5e], vec![0x66]),
            (4, vec![0x11, 0xfd], vec![0x12, 0x1d]),
            (8, vec![250, 10, 255], vec![250, 4, 3]),
            (
                16,
                vec![0xff, 0xff, 0, 2, 0xff, 0xff],
                vec![0xff, 0xff, 0, 1, 0, 0],
            ),
        ] {
            for lzw in [false, true] {
                let two_rows = encoded.repeat(2);
                let data = if lzw {
                    lzw_encoded(&two_rows, true)
                } else {
                    compressed(&two_rows)
                };
                let stream = Stream::new(
                    dictionary! { "Filter" => if lzw { "LZWDecode" } else { "FlateDecode" }, "DecodeParms" => dictionary! { "Predictor" => 2, "BitsPerComponent" => bits, "Columns" => 3 } },
                    data,
                );
                assert_eq!(
                    plain_content_limited(&stream, two_rows.len() as u64, "TIFF sample").unwrap(),
                    decoded.repeat(2),
                    "bits={bits}, lzw={lzw}"
                );
                assert!(
                    plain_content_limited(&stream, two_rows.len() as u64 - 1, "TIFF capped")
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn tiff_predictor_keeps_color_channels_separate_across_packed_bytes() {
        for (bits, encoded, decoded) in [
            (1, vec![0xba], vec![0xae]),
            (2, vec![0x6d, 0xab], vec![0x6e, 0x1b]),
            (4, vec![0x12, 0x31, 0x11], vec![0x12, 0x32, 0x34]),
            (8, vec![1, 2, 3, 1, 1, 1], vec![1, 2, 3, 2, 3, 4]),
            (
                16,
                vec![0xff, 0xff, 0, 1, 1, 0, 0, 2, 0xff, 0xff, 1, 0],
                vec![0xff, 0xff, 0, 1, 1, 0, 0, 1, 0, 0, 2, 0],
            ),
        ] {
            let mut doc = lopdf::Document::with_version("1.7");
            let bits = doc.add_object(Object::Integer(bits));
            let colors = doc.add_object(Object::Integer(3));
            let parameters = doc.add_object(dictionary! { "Predictor" => 2, "BitsPerComponent" => bits, "Colors" => colors, "Columns" => 2 });
            let stream = Stream::new(
                dictionary! { "Filter" => "FlateDecode", "DecodeParms" => parameters },
                compressed(&encoded),
            );
            assert_eq!(
                plain_content_limited_in(&doc, &stream, 4096, "color TIFF").unwrap(),
                decoded
            );
        }
    }

    #[test]
    fn tiff_predictor_rejects_truncated_rows_at_every_supported_depth() {
        for bits in [1, 2, 4, 8, 16] {
            let row_bytes = (17 * bits as usize).div_ceil(8);
            let data = vec![0; row_bytes - 1];
            let stream = Stream::new(
                dictionary! { "Filter" => "FlateDecode", "DecodeParms" => dictionary! { "Predictor" => 2, "BitsPerComponent" => bits, "Columns" => 17 } },
                compressed(&data),
            );
            let error = plain_content_limited(&stream, 4096, "TIFF truncated")
                .unwrap_err()
                .to_string();
            assert!(error.contains("truncated TIFF"), "bits={bits}: {error}");
        }
    }

    #[test]
    fn large_predictor_rows_are_not_silently_clamped() {
        let row = vec![7; 70_000];
        let mut data = vec![0];
        data.extend_from_slice(&row);
        let stream = Stream::new(
            lopdf::dictionary! { "Filter" => "FlateDecode", "DecodeParms" => lopdf::dictionary! { "Predictor" => 15, "Columns" => 70_000 } },
            compressed(&data),
        );
        assert_eq!(
            plain_content_limited(&stream, 1 << 20, "large row").unwrap(),
            row
        );
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

    fn lzw_encoded(bytes: &[u8], early_change: bool) -> Vec<u8> {
        use weezl::{BitOrder, encode::Encoder};
        let mut encoder = if early_change {
            Encoder::with_tiff_size_switch(BitOrder::Msb, 8)
        } else {
            Encoder::new(BitOrder::Msb, 8)
        };
        encoder.encode(bytes).unwrap()
    }

    #[test]
    fn lzw_early_change_modes_roundtrip_and_obey_exact_output_limits() {
        let plain: Vec<u8> = (0..32_000)
            .map(|index| ((index * 17 + index / 113) % 251) as u8)
            .collect();
        for early_change in [false, true] {
            for name in ["LZWDecode", "LZW"] {
                let mut stream = stream_with(lzw_encoded(&plain, early_change), name);
                stream.dict.set(
                    "DecodeParms",
                    dictionary! { "EarlyChange" => i64::from(early_change) },
                );
                assert_eq!(
                    plain_content_limited(&stream, plain.len() as u64, "LZW normal").unwrap(),
                    plain
                );
                assert!(
                    plain_content_limited(&stream, plain.len() as u64 - 1, "LZW capped")
                        .unwrap_err()
                        .to_string()
                        .contains("safety limit")
                );
            }
        }
        let bytes = lzw_encoded(b"", true);
        assert!(decode_lzw(&bytes, true, 0, "empty").unwrap().is_empty());
    }

    #[test]
    fn lzw_truncation_invalid_codes_and_parameters_never_return_partial_text() {
        let plain = b"A useful body repeated A useful body repeated";
        let mut encoded = lzw_encoded(plain, true);
        encoded.truncate(encoded.len() - 2);
        for bytes in [encoded, vec![0xff], Vec::new()] {
            let error = decode_lzw(&bytes, true, 4096, "PDF page 1")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("PDF page 1") && error.contains("LZW"),
                "{error}"
            );
        }
        for value in [
            Object::Integer(2),
            Object::Integer(-1),
            Object::Real(1.0),
            Object::Name(b"bad".to_vec()),
        ] {
            let mut stream = stream_with(lzw_encoded(plain, true), "LZWDecode");
            stream
                .dict
                .set("DecodeParms", dictionary! { "EarlyChange" => value });
            assert!(
                plain_content_limited(&stream, 4096, "parameter")
                    .unwrap_err()
                    .to_string()
                    .contains("EarlyChange")
            );
        }
    }

    #[test]
    fn lzw_indirect_early_change_and_prediction_work_in_filter_chain() {
        let mut doc = lopdf::Document::with_version("1.7");
        let early = doc.add_object(Object::Integer(0));
        let columns = doc.add_object(Object::Integer(3));
        let params = doc.add_object(
            dictionary! { "EarlyChange" => early, "Predictor" => 15, "Columns" => columns },
        );
        let payload = lzw_encoded(&[1, 10, 10, 10, 2, 1, 1, 1], false);
        let encoded = payload
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            + ">";
        let stream = Stream::new(
            dictionary! { "Filter" => vec![Object::Name(b"AHx".to_vec()), Object::Name(b"LZW".to_vec())], "DecodeParms" => vec![Object::Null, params.into()] },
            encoded.into_bytes(),
        );
        assert_eq!(
            plain_content_limited_in(&doc, &stream, 4096, "chain").unwrap(),
            [10, 20, 30, 11, 21, 31]
        );
    }

    #[test]
    fn run_length_literal_repeat_end_marker_and_output_boundaries() {
        for name in ["RunLengthDecode", "RL"] {
            let stream = stream_with(vec![2, b'A', b'B', b'C', 254, b'Z', 128], name);
            assert_eq!(
                plain_content_limited(&stream, 6, "RL normal").unwrap(),
                b"ABCZZZ"
            );
            assert!(
                plain_content_limited(&stream, 5, "RL capped")
                    .unwrap_err()
                    .to_string()
                    .contains("safety limit")
            );
        }
        for bytes in [vec![128], vec![128, 3, 4]] {
            assert!(decode_run_length(&bytes, 0, "empty").unwrap().is_empty());
        }
        for bytes in [vec![2, b'A'], vec![255], vec![0, b'A'], Vec::new()] {
            let error = decode_run_length(&bytes, 4096, "PDF Form")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("PDF Form") && error.contains("RunLength"),
                "{error}"
            );
        }
        let expanded = [vec![129, 0].repeat(4096), vec![128]].concat();
        assert!(
            decode_run_length(&expanded, 1024, "RL expansion")
                .unwrap_err()
                .to_string()
                .contains("safety limit")
        );
    }

    #[test]
    fn exotic_filters_are_reported_rather_than_decoded_unbounded() {
        let stream = stream_with(vec![1, 2, 3], "CCITTFaxDecode");
        let error = plain_content_limited(&stream, 1 << 20, "test stream")
            .unwrap_err()
            .to_string();
        assert!(error.contains("'CCITTFaxDecode' filter"), "{error}");
    }
}
