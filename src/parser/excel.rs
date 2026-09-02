//! Excel / CSV parser — table data extraction.
//!
//! Extracts text from spreadsheets:
//! - CSV: simple comma-separated values → text table
//! - XLSX: Excel 2007+ → traverse rows/cells via zip XML parsing

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read, Write};

const MAX_WORKBOOK_XML_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpreadsheetSheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
    /// Merged-cell ranges in 1-based (row, col) inclusive tuples:
    /// `(min_row, min_col, max_row, max_col)` — mirrors openpyxl
    /// `ws.merged_cells.ranges` consumed by `rag/app/table.py`.
    pub merged_ranges: Vec<(u32, u32, u32, u32)>,
}

#[derive(Default)]
pub struct ExcelParser;

impl ExcelParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse CSV data into text table format.
    fn parse_csv(&self, data: &[u8]) -> Result<String> {
        let content =
            String::from_utf8(data.to_vec()).map_err(|e| anyhow::anyhow!("UTF-8 decode: {}", e))?;

        let mut csv_reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(content.as_bytes());

        let mut text = String::new();
        let mut row_count = 0;

        // Headers
        if let Ok(headers) = csv_reader.headers() {
            text.push_str("<!--TABLE_START-->\n");
            text.push_str(&format!(
                "| {} |\n",
                headers.iter().collect::<Vec<_>>().join(" | ")
            ));
            text.push_str(&format!(
                "|{}|\n",
                headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")
            ));
        }

        for result in csv_reader.records() {
            if let Ok(record) = result {
                text.push_str(&format!(
                    "| {} |\n",
                    record.iter().collect::<Vec<_>>().join(" | ")
                ));
                row_count += 1;
            }
            if row_count > 1000 {
                text.push_str("| ... (truncated) |\n");
                break;
            }
        }
        text.push_str("<!--TABLE_END-->\n");

        Ok(text)
    }

    /// Parse XLSX data by reading shared strings and sheet XML directly.
    fn parse_xlsx(&self, data: &[u8]) -> Result<String> {
        let sheets = read_xlsx_sheets(data, 1000)?;
        let mut text = String::new();
        text.push_str("<!--TABLE_START-->\n");
        if let Some(sheet) = sheets.first() {
            for cells in &sheet.rows {
                text.push_str(&format!("| {} |\n", cells.join(" | ")));
            }
            if sheet.truncated {
                text.push_str("| ... (truncated) |\n");
            }
        }
        text.push_str("<!--TABLE_END-->\n");

        Ok(text)
    }
}

pub(crate) fn read_xlsx_sheets(
    data: &[u8],
    max_rows_per_sheet: usize,
) -> Result<Vec<SpreadsheetSheet>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
    let shared_strings = read_zip_text(&mut archive, "xl/sharedStrings.xml")?
        .map(|xml| parse_shared_strings(&xml))
        .transpose()?
        .unwrap_or_default();
    let descriptors = resolve_sheet_descriptors(&mut archive)?;

    let mut sheets = Vec::with_capacity(descriptors.len());
    for (name, path) in descriptors {
        let Some(xml) = read_zip_text(&mut archive, &path)? else {
            continue;
        };
        let (rows, merged_ranges, truncated) =
            parse_sheet_rows(&xml, &shared_strings, max_rows_per_sheet)?;
        sheets.push(SpreadsheetSheet {
            name,
            rows,
            merged_ranges,
            truncated,
        });
    }
    Ok(sheets)
}

/// Resolve `(sheet name, worksheet path)` pairs for a workbook in document
/// order. Falls back to alphabetical `xl/worksheets/sheetN.xml` paths when
/// workbook.xml / rels are missing or unparsable (mirrors the openpyxl
/// fallback of RAGFlow `_load_excel_to_workbook`).
fn resolve_sheet_descriptors<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<Vec<(String, String)>> {
    let workbook = read_zip_text(archive, "xl/workbook.xml")?;
    let relationships = read_zip_text(archive, "xl/_rels/workbook.xml.rels")?;

    let mut descriptors = match (workbook.as_deref(), relationships.as_deref()) {
        (Some(workbook), Some(relationships)) => {
            let targets = parse_workbook_relationships(relationships)?;
            parse_workbook_sheets(workbook)?
                .into_iter()
                .filter_map(|(name, relation)| {
                    targets
                        .get(&relation)
                        .map(|target| (name, workbook_target_path(target)))
                })
                .collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    if descriptors.is_empty() {
        let mut paths = archive
            .file_names()
            .filter(|name| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        paths.sort();
        descriptors = paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| (format!("Sheet{}", index + 1), path))
            .collect();
    }
    Ok(descriptors)
}

pub(crate) fn write_xlsx_sheets(sheets: &[(String, Vec<Vec<Value>>)]) -> Result<Vec<u8>> {
    let empty_merges: Vec<Vec<(u32, u32, u32, u32)>> = sheets.iter().map(|_| Vec::new()).collect();
    write_xlsx_sheets_impl(sheets, &empty_merges)
}

/// Write an XLSX workbook with merged-cell ranges per sheet. Ranges are
/// 1-based inclusive (min_row, min_col, max_row, max_col) and emitted as
/// `<mergeCells><mergeCell ref="A1:B2"/></mergeCells>` — mirrors
/// xlsxwriter `merge_range` output consumed by RAGFlow `rag/app/table.py`.
pub(crate) fn write_xlsx_sheets_with_merges(
    sheets: &[(String, Vec<Vec<Value>>)],
    merged_ranges: &[Vec<(u32, u32, u32, u32)>],
) -> Result<Vec<u8>> {
    write_xlsx_sheets_impl(sheets, merged_ranges)
}

fn write_xlsx_sheets_impl(
    sheets: &[(String, Vec<Vec<Value>>)],
    merged_ranges: &[Vec<(u32, u32, u32, u32)>],
) -> Result<Vec<u8>> {
    let normalized = normalize_workbook_sheets(sheets);
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    write_zip_entry(
        &mut archive,
        "[Content_Types].xml",
        &content_types_xml(normalized.len()),
        options,
    )?;
    write_zip_entry(&mut archive, "_rels/.rels", PACKAGE_RELS_XML, options)?;
    write_zip_entry(
        &mut archive,
        "xl/workbook.xml",
        &workbook_xml(&normalized),
        options,
    )?;
    write_zip_entry(
        &mut archive,
        "xl/_rels/workbook.xml.rels",
        &workbook_rels_xml(normalized.len()),
        options,
    )?;
    write_zip_entry(&mut archive, "xl/styles.xml", STYLES_XML, options)?;
    for (index, (_, rows)) in normalized.iter().enumerate() {
        let merges = merged_ranges.get(index).map(Vec::as_slice).unwrap_or(&[]);
        write_zip_entry(
            &mut archive,
            &format!("xl/worksheets/sheet{}.xml", index + 1),
            &worksheet_xml(rows, merges),
            options,
        )?;
    }
    Ok(archive.finish()?.into_inner())
}

fn read_zip_text<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Option<String>> {
    let Ok(file) = archive.by_name(path) else {
        return Ok(None);
    };
    if file.size() > MAX_WORKBOOK_XML_BYTES {
        anyhow::bail!("XLSX part '{path}' exceeds the 64 MiB safety limit");
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.take(MAX_WORKBOOK_XML_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_WORKBOOK_XML_BYTES {
        anyhow::bail!("XLSX part '{path}' exceeds the 64 MiB safety limit");
    }
    Ok(Some(String::from_utf8(bytes)?))
}

fn parse_workbook_sheets(xml: &str) -> Result<Vec<(String, String)>> {
    let mut reader = Reader::from_str(xml);
    let mut sheets = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(event) | Event::Empty(event)
                if event.local_name().as_ref() == b"sheet" =>
            {
                let name = attribute_value(&event, b"name", reader.decoder())?;
                let relation = attribute_value(&event, b"id", reader.decoder())?;
                if let (Some(name), Some(relation)) = (name, relation) {
                    sheets.push((name, relation));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sheets)
}

fn parse_workbook_relationships(xml: &str) -> Result<BTreeMap<String, String>> {
    let mut reader = Reader::from_str(xml);
    let mut relationships = BTreeMap::new();
    loop {
        match reader.read_event()? {
            Event::Start(event) | Event::Empty(event)
                if event.local_name().as_ref() == b"Relationship" =>
            {
                let id = attribute_value(&event, b"Id", reader.decoder())?;
                let target = attribute_value(&event, b"Target", reader.decoder())?;
                if let (Some(id), Some(target)) = (id, target) {
                    relationships.insert(id, target);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(relationships)
}

fn workbook_target_path(target: &str) -> String {
    let target = target.trim_start_matches('/');
    if target.starts_with("xl/") {
        target.to_owned()
    } else {
        format!("xl/{target}")
    }
}

type WorkbookRows = Vec<(String, Vec<Vec<Value>>)>;

fn normalize_workbook_sheets(sheets: &[(String, Vec<Vec<Value>>)]) -> WorkbookRows {
    let source = if sheets.is_empty() {
        vec![("Sheet1".to_owned(), Vec::new())]
    } else {
        sheets.to_vec()
    };
    let mut used = HashSet::new();
    source
        .into_iter()
        .enumerate()
        .map(|(index, (name, rows))| {
            let base = sanitize_sheet_name(&name, index + 1);
            let mut unique = base.clone();
            let mut suffix = 2;
            while !used.insert(unique.to_ascii_lowercase()) {
                let marker = format!(" ({suffix})");
                let keep = 31usize.saturating_sub(marker.chars().count());
                unique = format!("{}{}", base.chars().take(keep).collect::<String>(), marker);
                suffix += 1;
            }
            (unique, rows)
        })
        .collect()
}

fn sanitize_sheet_name(name: &str, index: usize) -> String {
    let sanitized = name
        .trim_matches('\'')
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if matches!(character, '[' | ']' | ':' | '*' | '?' | '/' | '\\') {
                '_'
            } else {
                character
            }
        })
        .take(31)
        .collect::<String>();
    if sanitized.is_empty() {
        format!("Sheet{index}")
    } else {
        sanitized
    }
}

fn write_zip_entry<W: Write + std::io::Seek>(
    archive: &mut zip::ZipWriter<W>,
    path: &str,
    content: &str,
    options: zip::write::SimpleFileOptions,
) -> Result<()> {
    archive.start_file(path, options)?;
    archive.write_all(content.as_bytes())?;
    Ok(())
}

fn content_types_xml(sheet_count: usize) -> String {
    let sheets = (1..=sheet_count)
        .map(|index| format!("<Override PartName=\"/xl/worksheets/sheet{index}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"))
        .collect::<String>();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>{sheets}</Types>"
    )
}

fn workbook_xml(sheets: &WorkbookRows) -> String {
    let sheets = sheets
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            format!(
                "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                escape_xml(name),
                index + 1,
                index + 1
            )
        })
        .collect::<String>();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>{sheets}</sheets></workbook>"
    )
}

fn workbook_rels_xml(sheet_count: usize) -> String {
    let mut relationships = (1..=sheet_count)
        .map(|index| format!("<Relationship Id=\"rId{index}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{index}.xml\"/>"))
        .collect::<String>();
    relationships.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>", sheet_count + 1));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
    )
}

fn worksheet_xml(rows: &[Vec<Value>], merged_ranges: &[(u32, u32, u32, u32)]) -> String {
    let rows = rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            let cells = row
                .iter()
                .enumerate()
                .map(|(column_index, value)| {
                    cell_xml(value, &cell_reference(column_index, row_index))
                })
                .collect::<String>();
            format!("<row r=\"{}\">{cells}</row>", row_index + 1)
        })
        .collect::<String>();
    let merge_cells = if merged_ranges.is_empty() {
        String::new()
    } else {
        let entries = merged_ranges
            .iter()
            .map(|(min_row, min_col, max_row, max_col)| {
                format!(
                    "<mergeCell ref=\"{0}:{1}\"/>",
                    cell_reference(*min_col as usize - 1, *min_row as usize - 1),
                    cell_reference(*max_col as usize - 1, *max_row as usize - 1),
                )
            })
            .collect::<String>();
        format!(
            "<mergeCells count=\"{}\">{entries}</mergeCells>",
            merged_ranges.len()
        )
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{merge_cells}<sheetData>{rows}</sheetData></worksheet>"
    )
}

fn cell_xml(value: &Value, reference: &str) -> String {
    match value {
        Value::Bool(value) => format!(
            "<c r=\"{reference}\" t=\"b\"><v>{}</v></c>",
            usize::from(*value)
        ),
        Value::Number(value) => format!("<c r=\"{reference}\"><v>{value}</v></c>"),
        Value::String(value) => inline_string_cell(reference, value),
        Value::Null => inline_string_cell(reference, ""),
        value => inline_string_cell(reference, &value.to_string()),
    }
}

fn inline_string_cell(reference: &str, value: &str) -> String {
    format!(
        "<c r=\"{reference}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
        escape_xml(value)
    )
}

fn cell_reference(mut column: usize, row: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push((b'A' + (column % 26) as u8) as char);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    letters.reverse();
    format!("{}{}", letters.into_iter().collect::<String>(), row + 1)
}

fn escape_xml(value: &str) -> Cow<'_, str> {
    if !value
        .chars()
        .any(|character| matches!(character, '&' | '<' | '>' | '\'' | '"'))
    {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "&apos;")
            .replace('"', "&quot;"),
    )
}

const PACKAGE_RELS_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";
const STYLES_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"1\"><fill><patternFill patternType=\"none\"/></fill></fills><borders count=\"1\"><border/></borders><cellStyleXfs count=\"1\"><xf/></cellStyleXfs><cellXfs count=\"1\"><xf xfId=\"0\"/></cellXfs></styleSheet>";

fn parse_shared_strings(xml: &str) -> Result<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut in_si = false;
    let mut in_text = false;

    loop {
        match reader.read_event()? {
            Event::Start(event) if event.local_name().as_ref() == b"si" => {
                current.clear();
                in_si = true;
            }
            Event::Start(event) if in_si && event.local_name().as_ref() == b"t" => {
                in_text = true;
            }
            Event::Text(event) if in_text => current.push_str(&event.unescape()?),
            Event::CData(event) if in_text => {
                current.push_str(&String::from_utf8_lossy(event.as_ref()))
            }
            Event::End(event) if event.local_name().as_ref() == b"t" => in_text = false,
            Event::End(event) if event.local_name().as_ref() == b"si" => {
                strings.push(current.clone());
                in_si = false;
                in_text = false;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(strings)
}

fn parse_sheet_rows(
    xml: &str,
    shared_strings: &[String],
    max_rows: usize,
) -> Result<(Vec<Vec<String>>, Vec<(u32, u32, u32, u32)>, bool)> {
    let mut reader = Reader::from_str(xml);
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let mut cell_kind = String::new();
    let mut cell_column = None;
    let mut cell_value = String::new();
    let mut in_cell_value = false;
    let mut in_row = false;
    // Merged ranges collected from `<mergeCells><mergeCell ref="A1:B2"/>`.
    let mut merged_ranges: Vec<(u32, u32, u32, u32)> = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(event) if event.local_name().as_ref() == b"row" => {
                cells.clear();
                in_row = true;
            }
            Event::Start(event) if event.local_name().as_ref() == b"mergeCell" => {
                // Attribute is `ref` (not `r`): "A1:B2".
                let reference = attribute_value(&event, b"ref", reader.decoder())?;
                if let Some((min_row, min_col, max_row, max_col)) =
                    reference.as_deref().and_then(parse_merge_range)
                {
                    merged_ranges.push((min_row, min_col, max_row, max_col));
                }
            }
            Event::Empty(event) if event.local_name().as_ref() == b"mergeCell" => {
                // Self-closing `<mergeCell ref="A1:B2"/>`.
                let reference = attribute_value(&event, b"ref", reader.decoder())?;
                if let Some((min_row, min_col, max_row, max_col)) =
                    reference.as_deref().and_then(parse_merge_range)
                {
                    merged_ranges.push((min_row, min_col, max_row, max_col));
                }
            }
            Event::Start(event) if in_row && event.local_name().as_ref() == b"c" => {
                cell_kind = attribute_value(&event, b"t", reader.decoder())?.unwrap_or_default();
                cell_column = attribute_value(&event, b"r", reader.decoder())?
                    .as_deref()
                    .and_then(column_index);
                cell_value.clear();
            }
            Event::Start(event)
                if in_row
                    && (event.local_name().as_ref() == b"v"
                        || (cell_kind == "inlineStr" && event.local_name().as_ref() == b"t")) =>
            {
                in_cell_value = true;
            }
            Event::Text(event) if in_cell_value => cell_value.push_str(&event.unescape()?),
            Event::CData(event) if in_cell_value => {
                cell_value.push_str(&String::from_utf8_lossy(event.as_ref()))
            }
            Event::End(event)
                if event.local_name().as_ref() == b"v"
                    || (cell_kind == "inlineStr" && event.local_name().as_ref() == b"t") =>
            {
                in_cell_value = false;
            }
            Event::End(event) if event.local_name().as_ref() == b"c" => {
                let value = match cell_kind.as_str() {
                    "s" => cell_value
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| shared_strings.get(index))
                        .cloned()
                        .unwrap_or_default(),
                    "b" => match cell_value.as_str() {
                        "1" => "true".into(),
                        "0" => "false".into(),
                        _ => cell_value.clone(),
                    },
                    _ => cell_value.clone(),
                };
                let column = cell_column.unwrap_or(cells.len()).min(16_383);
                if cells.len() <= column {
                    cells.resize(column + 1, String::new());
                }
                cells[column] = value;
                in_cell_value = false;
            }
            Event::End(event) if event.local_name().as_ref() == b"row" => {
                in_row = false;
                if cells.iter().any(|value| !value.is_empty()) {
                    if rows.len() == max_rows {
                        return Ok((rows, merged_ranges, true));
                    }
                    rows.push(cells.clone());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((rows, merged_ranges, false))
}

/// Parse an Excel `ref` like `"A1:B2"` (or a single cell `"C5"`) into
/// 1-based inclusive `(min_row, min_col, max_row, max_col)`.
fn parse_merge_range(reference: &str) -> Option<(u32, u32, u32, u32)> {
    let (first, second) = match reference.split_once(':') {
        Some((f, s)) => (f, s),
        None => (reference, reference),
    };
    let (row1, col1) = parse_cell_reference(first)?;
    let (row2, col2) = parse_cell_reference(second)?;
    Some((
        row1.min(row2),
        col1.min(col2),
        row1.max(row2),
        col1.max(col2),
    ))
}

/// Parse `"A1"` into 1-based `(row, col)`.
fn parse_cell_reference(reference: &str) -> Option<(u32, u32)> {
    let letters: String = reference
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits: String = reference
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let mut col = 0u32;
    for byte in letters.bytes() {
        col = col
            .checked_mul(26)?
            .checked_add((byte.to_ascii_uppercase() - b'A' + 1) as u32)?;
    }
    let row = digits.parse::<u32>().ok()?;
    Some((row, col))
}

fn attribute_value(
    event: &BytesStart<'_>,
    name: &[u8],
    decoder: quick_xml::encoding::Decoder,
) -> Result<Option<String>> {
    for attribute in event.attributes() {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == name {
            return Ok(Some(
                attribute.decode_and_unescape_value(decoder)?.into_owned(),
            ));
        }
    }
    Ok(None)
}

fn column_index(reference: &str) -> Option<usize> {
    let mut index = 0usize;
    let mut found = false;
    for byte in reference
        .bytes()
        .take_while(|byte| byte.is_ascii_alphabetic())
    {
        found = true;
        index = index
            .checked_mul(26)?
            .checked_add((byte.to_ascii_uppercase() - b'A' + 1) as usize)?;
    }
    found.then_some(index - 1)
}

impl Parse for ExcelParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = if name.to_lowercase().ends_with(".csv") {
            self.parse_csv(data)?
        } else {
            self.parse_xlsx(data)?
        };

        Ok(new_document(
            name,
            content,
            if name.ends_with(".csv") {
                "text/csv"
            } else {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            },
            data.len(),
        ))
    }
}

// ---------------------------------------------------------------------------
// RAGFlow `RAGFlowExcelParser` multi-sheet semantics — excel_parser.py.
//
// The Python `__call__` emits one line per data row as
// `header：value; header：value` with a trailing ` ——sheetname` suffix unless
// the sheet name contains "sheet" (case-insensitive). `html()` emits
// `<table><caption>{sheet}</caption>` chunks of `chunk_rows` (default 256)
// rows per table. `row_number()` estimates the total data-row count: for
// spreadsheets it sums each sheet's actual last-data-row position (found via
// the same binary-search over the worksheet XML as `_get_actual_row_count`),
// and for CSV/TXT it counts newline-separated lines.
// ---------------------------------------------------------------------------

/// `__call__` — excel_parser.py:263-292. One `header：value; ...` line per
/// data row; ` ——sheetname` appended unless the sheet name contains "sheet".
pub(crate) fn sheet_lines(sheets: &[SpreadsheetSheet]) -> Vec<String> {
    let mut res = Vec::new();
    for sheet in sheets {
        if sheet.rows.is_empty() {
            continue;
        }
        let header = &sheet.rows[0];
        for row in &sheet.rows[1..] {
            let mut fields = Vec::new();
            for (i, cell) in row.iter().enumerate() {
                if cell.trim().is_empty() {
                    continue;
                }
                // Python: `t = str(ti[i].value) if i < len(ti) else ""` — an
                // empty openpyxl cell is None, whose str() is "None".
                let mut t = match header.get(i) {
                    Some(h) if !h.is_empty() => h.clone(),
                    Some(_) => "None".to_string(),
                    None => String::new(),
                };
                if !t.is_empty() {
                    t.push('：');
                }
                t.push_str(cell);
                fields.push(t);
            }
            if fields.is_empty() {
                continue;
            }
            let mut line = fields.join("; ");
            if !sheet.name.to_lowercase().contains("sheet") {
                line.push_str(" ——");
                line.push_str(&sheet.name);
            }
            res.push(line);
        }
    }
    res
}

/// `html()` — excel_parser.py:204-247. One `<table><caption>{sheet}</caption>`
/// per `chunk_rows` (default 256) data rows; header row emitted as `<th>`.
pub(crate) fn sheet_html(sheets: &[SpreadsheetSheet], chunk_rows: usize) -> Vec<String> {
    let chunk_rows = chunk_rows.max(1);
    let mut chunks = Vec::new();
    for sheet in sheets {
        if sheet.rows.is_empty() {
            continue;
        }
        let header = &sheet.rows[0];
        let mut th = String::from("<tr>");
        for cell in header {
            th.push_str(&format!("<th>{}</th>", html_escape(cell)));
        }
        th.push_str("</tr>");

        let data_rows = &sheet.rows[1..];
        for chunk_i in 0..(data_rows.len().div_ceil(chunk_rows)) {
            let mut tb = String::new();
            tb.push_str(&format!(
                "<table><caption>{}</caption>",
                html_escape(&sheet.name)
            ));
            tb.push_str(&th);
            let start = chunk_i * chunk_rows;
            let end = (start + chunk_rows).min(data_rows.len());
            for row in &data_rows[start..end] {
                tb.push_str("<tr>");
                for cell in row {
                    tb.push_str(&format!("<td>{}</td>", html_escape(cell)));
                }
                tb.push_str("</tr>");
            }
            tb.push_str("</table>\n");
            chunks.push(tb);
        }
    }
    chunks
}

/// HTML-escape a cell value like Python `html.escape` (quotes included).
fn html_escape(value: &str) -> Cow<'_, str> {
    if !value
        .chars()
        .any(|c| matches!(c, '&' | '<' | '>' | '"' | '\''))
    {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#x27;"),
    )
}

/// `_get_actual_row_count` — excel_parser.py:156-195. Returns the position of
/// the last row that contains data (0 when the sheet is empty). Sheets with
/// `max_row <= 10000` short-circuit; larger sheets use the binary search with
/// a 10-row window, exactly like the upstream implementation.
pub(crate) fn actual_row_count_from_xml(xml: &str, shared_strings: &[String]) -> usize {
    // One pass: max_row (from <dimension> or row r attrs), max_col (≤ 50
    // cells checked per row like `min(ws.max_column or 1, 50)`), and the set
    // of rows holding any non-empty cell value.
    let mut reader = Reader::from_str(xml);
    let mut max_row = 0usize;
    let mut max_col = 0usize;
    let mut data_rows: HashSet<usize> = HashSet::new();

    let mut row_index = 0usize;
    let mut row_attr = None;
    let mut cell_kind = String::new();
    let mut cell_column = None;
    let mut cell_value = String::new();
    let mut in_cell_value = false;
    let mut in_row = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event))
                if event.local_name().as_ref() == b"dimension" =>
            {
                if let Some(reference) =
                    attribute_value(&event, b"ref", reader.decoder()).unwrap_or(None)
                    && let Some((row, col)) =
                        reference.rsplit(':').next().and_then(parse_cell_reference)
                    {
                        max_row = max_row.max(row as usize);
                        max_col = max_col.max(col as usize);
                    }
            }
            Ok(Event::Start(event)) if event.local_name().as_ref() == b"row" => {
                in_row = true;
                cell_value.clear();
                row_attr = attribute_value(&event, b"r", reader.decoder())
                    .unwrap_or(None)
                    .and_then(|r| r.parse::<usize>().ok());
                row_index = row_attr.unwrap_or(row_index + 1);
                max_row = max_row.max(row_index);
            }
            Ok(Event::Start(event)) if in_row && event.local_name().as_ref() == b"c" => {
                cell_kind = attribute_value(&event, b"t", reader.decoder())
                    .unwrap_or_default()
                    .unwrap_or_default();
                cell_column = attribute_value(&event, b"r", reader.decoder())
                    .unwrap_or(None)
                    .as_deref()
                    .and_then(column_index);
                cell_value.clear();
            }
            Ok(Event::Start(event))
                if in_row
                    && (event.local_name().as_ref() == b"v"
                        || (cell_kind == "inlineStr" && event.local_name().as_ref() == b"t")) =>
            {
                in_cell_value = true;
            }
            Ok(Event::Text(event)) if in_cell_value => {
                if let Ok(text) = event.unescape() {
                    cell_value.push_str(&text);
                }
            }
            Ok(Event::End(event))
                if event.local_name().as_ref() == b"v"
                    || (cell_kind == "inlineStr" && event.local_name().as_ref() == b"t") =>
            {
                in_cell_value = false;
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == b"c" => {
                let value = match cell_kind.as_str() {
                    "s" => cell_value
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| shared_strings.get(index))
                        .cloned()
                        .unwrap_or_default(),
                    "b" => match cell_value.as_str() {
                        "1" => "true".into(),
                        "0" => "false".into(),
                        _ => cell_value.clone(),
                    },
                    _ => cell_value.clone(),
                };
                let column = cell_column.unwrap_or(0).min(49); // only first 50 cols checked
                max_col = max_col.max(column + 1);
                if !value.trim().is_empty() {
                    data_rows.insert(row_index);
                }
                in_cell_value = false;
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == b"row" => {
                in_row = false;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }

    if max_row == 0 {
        return 0;
    }
    if max_row <= 10_000 {
        return max_row;
    }
    let check_cols = max_col.clamp(1, 50);
    let row_has_data = |row: usize| {
        if !data_rows.contains(&row) {
            return false;
        }
        // Column bound is not tracked per cell here; the upstream check
        // limits to min(max_column, 50) — our scan already caps at 50.
        let _ = check_cols;
        true
    };
    // If none of the first 100 rows has data, the sheet counts as empty.
    if !(1..=(100.min(max_row))).any(row_has_data) {
        return 0;
    }

    let mut left = 1usize;
    let mut right = max_row;
    let mut last_data_row = 1usize;
    while left <= right {
        let mid = (left + right) / 2;
        let mut found = false;
        for r in mid..(mid + 10).min(max_row + 1) {
            if row_has_data(r) {
                found = true;
                last_data_row = last_data_row.max(r);
                break;
            }
        }
        if found {
            left = mid + 1;
        } else {
            right = mid - 1;
        }
    }
    for r in last_data_row..(last_data_row + 500).min(max_row + 1) {
        if row_has_data(r) {
            last_data_row = r;
        }
    }
    last_data_row
}

/// `row_number` — excel_parser.py:294-312. Sum of per-sheet actual row counts
/// for spreadsheets; newline count for CSV/TXT payloads.
pub(crate) fn row_number(name: &str, data: &[u8]) -> Result<usize> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext.contains("xls") {
        let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
        let shared_strings = read_zip_text(&mut archive, "xl/sharedStrings.xml")?
            .map(|xml| parse_shared_strings(&xml))
            .transpose()?
            .unwrap_or_default();
        let descriptors = resolve_sheet_descriptors(&mut archive)?;
        let mut total = 0usize;
        for (_, path) in descriptors {
            if let Some(xml) = read_zip_text(&mut archive, &path)? {
                total += actual_row_count_from_xml(&xml, &shared_strings);
            }
        }
        return Ok(total);
    }
    if ext == "csv" || ext == "txt" {
        let text = crate::parser::txt::decode_text(data)?;
        return Ok(text.split('\n').count());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn xlsx_xml_preserves_cell_order_and_rich_shared_strings() {
        let shared = parse_shared_strings(
            r#"<sst><si><r><t>Hello &amp; </t></r><r><t>world</t></r></si></sst>"#,
        )
        .unwrap();
        let (rows, merged_ranges, truncated) = parse_sheet_rows(
            r#"<worksheet><mergeCells count="1"><mergeCell ref="B2:C3"/></mergeCells><sheetData><row><c r="A1"><v>42</v></c><c r="B1" t="s"><v>0</v></c><c r="D1" t="inlineStr"><is><t>tail</t></is></c></row></sheetData></worksheet>"#,
            &shared,
            1000,
        )
        .unwrap();
        assert!(!truncated);
        assert_eq!(rows, vec![vec!["42", "Hello & world", "", "tail"]]);
        // mergeCell ref="B2:C3" → 1-based inclusive range
        assert_eq!(merged_ranges, vec![(2, 2, 3, 3)]);
    }

    #[test]
    fn merge_range_parsing_handles_single_and_multi_cell_refs() {
        assert_eq!(parse_merge_range("A1:B2"), Some((1, 1, 2, 2)));
        assert_eq!(parse_merge_range("C5"), Some((5, 3, 5, 3)));
        assert_eq!(parse_merge_range("B2:A1"), Some((1, 1, 2, 2)));
        assert_eq!(parse_merge_range(""), None);
        assert_eq!(parse_merge_range("12"), None);
    }

    #[test]
    fn merged_ranges_round_trip_through_xlsx_writer() {
        let bytes = write_xlsx_sheets_with_merges(
            &[(
                "Merged".into(),
                vec![
                    vec![json!("name"), json!("value")],
                    vec![json!("a"), json!("1")],
                ],
            )],
            &[vec![(1, 1, 2, 1), (1, 2, 1, 2)]],
        )
        .unwrap();
        let sheets = read_xlsx_sheets(&bytes, 100).unwrap();
        assert_eq!(sheets[0].merged_ranges, vec![(1, 1, 2, 1), (1, 2, 1, 2)]);
        // Anchor cells survive: A1="name", B1="value", A2="a".
        assert_eq!(sheets[0].rows[0][0], "name");
        assert_eq!(sheets[0].rows[0][1], "value");
        assert_eq!(sheets[0].rows[1][0], "a");
    }

    #[test]
    fn xlsx_writer_round_trips_multiple_sheets_and_native_cells() {
        let bytes = write_xlsx_sheets(&[
            (
                "Alpha".into(),
                vec![
                    vec![json!("name"), json!("score"), json!("active")],
                    vec![json!("A&B"), json!(2), json!(true)],
                ],
            ),
            (
                "Beta/Unsafe".into(),
                vec![vec![json!("value")], vec![json!({"nested": 1})]],
            ),
        ])
        .unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");

        let sheets = read_xlsx_sheets(&bytes, 100).unwrap();
        assert_eq!(
            sheets
                .iter()
                .map(|sheet| sheet.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Beta_Unsafe"]
        );
        assert_eq!(
            sheets[0].rows,
            vec![vec!["name", "score", "active"], vec!["A&B", "2", "true"]]
        );
        assert_eq!(sheets[1].rows[1], [r#"{"nested":1}"#]);
    }

    #[test]
    fn xlsx_writer_keeps_an_empty_default_sheet() {
        let bytes = write_xlsx_sheets(&[]).unwrap();
        let sheets = read_xlsx_sheets(&bytes, 100).unwrap();
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].name, "Sheet1");
        assert!(sheets[0].rows.is_empty());
    }

    #[test]
    fn sheet_lines_emit_header_value_pairs_with_sheet_suffix() {
        // "__call__" — excel_parser.py:263-292.
        let sheets = vec![
            SpreadsheetSheet {
                name: "Sheet1".into(), // contains "sheet" → no suffix
                rows: vec![
                    vec!["name".into(), "qty".into()],
                    vec!["bolt".into(), "3".into()],
                    vec!["".into(), "7".into()], // empty cell skipped
                ],
                truncated: false,
                merged_ranges: vec![],
            },
            SpreadsheetSheet {
                name: "2024 销售".into(),
                rows: vec![
                    vec!["月份".into(), "金额".into()],
                    vec!["1月".into(), "100".into()],
                ],
                truncated: false,
                merged_ranges: vec![],
            },
        ];
        let lines = sheet_lines(&sheets);
        assert_eq!(
            lines,
            vec![
                "name：bolt; qty：3".to_string(),
                "qty：7".to_string(), // empty leading cell skipped, header "qty" used
                "月份：1月; 金额：100 ——2024 销售".to_string(),
            ]
        );
    }

    #[test]
    fn sheet_html_chunks_rows_and_escapes_content() {
        // "html()" — excel_parser.py:204-247.
        let sheets = vec![SpreadsheetSheet {
            name: "A&B".into(),
            rows: vec![
                vec!["k".into(), "v".into()],
                vec!["a".into(), "1".into()],
                vec!["b".into(), "2".into()],
                vec!["c".into(), "<3>".into()],
            ],
            truncated: false,
            merged_ranges: vec![],
        }];
        let chunks = sheet_html(&sheets, 2);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("<table><caption>A&amp;B</caption>"));
        assert!(chunks[0].contains("<th>k</th><th>v</th>"));
        assert!(chunks[0].contains("<td>a</td><td>1</td>"));
        assert!(chunks[1].contains("<td>c</td><td>&lt;3&gt;</td>"));
        assert!(chunks[1].ends_with("</table>\n"));
    }

    #[test]
    fn actual_row_count_uses_binary_search_for_large_sheets() {
        // "_get_actual_row_count" — excel_parser.py:156-195. The upstream
        // heuristic assumes dense data: the 10-row-window binary search only
        // converges when rows above the last data row are populated.
        let mut xml = String::from(r#"<worksheet><dimension ref="A1:XFD1048576"/><sheetData>"#);
        for r in 1..=20000u32 {
            xml.push_str(&format!(
                r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>d</t></is></c></row>"#
            ));
        }
        xml.push_str("</sheetData></worksheet>");
        assert_eq!(actual_row_count_from_xml(&xml, &[]), 20000);

        // Empty first 100 rows → counts as empty (0), even with data below.
        let empty = r#"<worksheet><dimension ref="A1:XFD1048576"/><sheetData><row r="50000"><c r="A50000"><v>5</v></c></row></sheetData></worksheet>"#;
        assert_eq!(actual_row_count_from_xml(empty, &[]), 0);

        // max_row <= 10000 short-circuits to max_row.
        let small = r#"<worksheet><dimension ref="A1:D120"/><sheetData><row r="120"><c r="A120"><v>1</v></c></row></sheetData></worksheet>"#;
        assert_eq!(actual_row_count_from_xml(small, &[]), 120);

        // Shared-string cells resolve through the sst like openpyxl values.
        let shared = r#"<worksheet><dimension ref="A1:B3"/><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>0</v></c></row><row r="3"><c r="B3"><v>42</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            actual_row_count_from_xml(shared, &["hello".to_string(), "world".to_string()]),
            3
        );
    }

    #[test]
    fn row_number_sums_sheets_and_counts_csv_lines() {
        // "row_number" — excel_parser.py:294-312.
        let bytes = write_xlsx_sheets(&[
            (
                "S1".into(),
                vec![vec![json!("a")], vec![json!("1")], vec![json!("2")]],
            ),
            ("S2".into(), vec![vec![json!("b")], vec![json!("3")]]),
        ])
        .unwrap();
        // openpyxl max_row = last data row: S1 → 3, S2 → 2.
        assert_eq!(row_number("book.xlsx", &bytes).unwrap(), 5);
        // Python: len(txt.split("\n")) — trailing newline yields an extra "".
        assert_eq!(row_number("data.csv", b"a,b\n1,2\n3,4\n").unwrap(), 4);
    }
}
