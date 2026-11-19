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


