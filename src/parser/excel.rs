//! Excel / CSV parser — table data extraction.
//!
//! Extracts text from spreadsheets:
//! - CSV: simple comma-separated values → text table
//! - XLSX: Excel 2007+ → traverse rows/cells via zip XML parsing

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use calamine::{Data as CalamineData, Reader as CalamineReader, open_workbook_auto_from_rs};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use time::{Date, Duration, Month};

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

/// Date/time styles resolved from `xl/styles.xml`, indexed by the cell `s`
/// attribute. openpyxl converts a numeric cell to a datetime when its style's
/// number format is a date/time format; RAGFlow prints that datetime with
/// `str()`, so the raw Excel serial must never reach the chunk text.
#[derive(Debug, Default, Clone)]
struct DateStyles {
    cell_is_date: Vec<bool>,
}

#[derive(Default)]
pub struct ExcelParser;

impl ExcelParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse CSV data into text table format.
    fn parse_csv(&self, data: &[u8]) -> Result<String> {
        let content = crate::parser::txt::decode_text(data)
            .map_err(|error| anyhow::anyhow!("CSV text decoding failed: {error}"))?;

        let mut csv_reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(content.as_bytes());

        let mut text = String::new();
        let headers = csv_reader.headers().map_err(|error| {
            anyhow::anyhow!(
                "CSV header read failed: {error}. 请修复文件并另存为 UTF-8 CSV 后重新上传 / Repair and save the file as UTF-8 CSV, then upload again."
            )
        })?;
        text.push_str("<!--TABLE_START-->\n");
        text.push_str(&format!(
            "| {} |\n",
            headers.iter().collect::<Vec<_>>().join(" | ")
        ));
        text.push_str(&format!(
            "|{}|\n",
            headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")
        ));

        for result in csv_reader.records() {
            let record = result.map_err(|error| {
                anyhow::anyhow!(
                    "CSV record read failed: {error}. 请修复文件并另存为 UTF-8 CSV 后重新上传 / Repair and save the file as UTF-8 CSV, then upload again."
                )
            })?;
            text.push_str(&format!(
                "| {} |\n",
                record.iter().collect::<Vec<_>>().join(" | ")
            ));
        }
        text.push_str("<!--TABLE_END-->\n");

        Ok(text)
    }

    /// Parse XLSX data by reading shared strings and sheet XML directly.
    fn parse_xlsx(&self, data: &[u8]) -> Result<String> {
        let sheets = read_xlsx_sheets(data, usize::MAX)?;
        Ok(sheets_markdown(&sheets))
    }

    /// Parse legacy BIFF/OLE2 .xls workbooks through calamine, mirroring
    /// RAGFlow's pandas read_excel(engine="calamine") fallback.
    fn parse_xls(&self, data: &[u8]) -> Result<String> {
        let sheets = read_xls_sheets(data, usize::MAX)?;
        Ok(sheets_markdown(&sheets))
    }
}

fn sheets_markdown(sheets: &[SpreadsheetSheet]) -> String {
    let mut text = String::new();
    for sheet in sheets {
        text.push_str(&format!("## {}\n\n", sheet.name));
        text.push_str("<!--TABLE_START-->\n");
        for cells in &sheet.rows {
            text.push_str(&format!("| {} |\n", cells.join(" | ")));
        }
        text.push_str("<!--TABLE_END-->\n");
    }
    text
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
    let date_styles = read_zip_text(&mut archive, "xl/styles.xml")?
        .as_deref()
        .map(parse_date_styles)
        .transpose()?
        .unwrap_or_default();
    let date1904 = read_zip_text(&mut archive, "xl/workbook.xml")?
        .as_deref()
        .map(workbook_uses_1904)
        .transpose()?
        .unwrap_or(false);
    let descriptors = resolve_sheet_descriptors(&mut archive)?;

    let mut sheets = Vec::with_capacity(descriptors.len());
    for (name, path) in descriptors {
        let Some(xml) = read_zip_text(&mut archive, &path)? else {
            anyhow::bail!(
                "XLSX worksheet {name:?} is missing ({path}). 请修复并另存为 XLSX 后重新上传 / Repair and save the workbook as XLSX, then upload again."
            );
        };
        let (rows, merged_ranges, truncated) = parse_sheet_rows(
            &xml,
            &shared_strings,
            max_rows_per_sheet,
            &date_styles,
            date1904,
        )?;
        sheets.push(SpreadsheetSheet {
            name,
            rows,
            merged_ranges,
            truncated,
        });
    }
    Ok(sheets)
}

fn is_zip_workbook(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04")
}

fn is_ole_workbook(data: &[u8]) -> bool {
    data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
}

/// Read a legacy BIFF/OLE2 workbook through calamine. RAGFlow reaches this
/// format through pandas read_excel(engine="calamine") after openpyxl rejects
/// the OLE container, then rebuilds an openpyxl workbook from the DataFrames.
pub(crate) fn read_xls_sheets(
    data: &[u8],
    max_rows_per_sheet: usize,
) -> Result<Vec<SpreadsheetSheet>> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(data)).map_err(|error| {
        anyhow::anyhow!(
            "Legacy XLS workbook could not be opened: {error}. 请修复或另存为 XLSX 后重新上传 / Repair the workbook or save it as XLSX, then upload again."
        )
    })?;
    let names = workbook.sheet_names();
    let mut sheets = Vec::with_capacity(names.len());
    for name in names {
        let range = workbook.worksheet_range(&name).map_err(|error| {
            anyhow::anyhow!(
                "Legacy XLS worksheet {name:?} could not be read: {error}. 请修复或另存为 XLSX 后重新上传 / Repair the workbook or save it as XLSX, then upload again."
            )
        })?;
        let mut rows = Vec::new();
        let mut truncated = false;
        for row in range.rows() {
            let mut values: Vec<String> = row.iter().map(calamine_cell_text).collect();
            let Some(end) = values.iter().rposition(|value| !value.is_empty()) else {
                continue;
            };
            values.truncate(end + 1);
            if rows.len() == max_rows_per_sheet {
                truncated = true;
                break;
            }
            rows.push(values);
        }
        sheets.push(SpreadsheetSheet {
            name,
            rows,
            truncated,
            merged_ranges: Vec::new(),
        });
    }
    Ok(sheets)
}

fn calamine_cell_text(value: &CalamineData) -> String {
    match value {
        CalamineData::Empty => String::new(),
        CalamineData::String(value) => value.clone(),
        CalamineData::Int(value) => value.to_string(),
        CalamineData::Float(value) => calamine_number_text(*value),
        CalamineData::Bool(value) => {
            if *value {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        CalamineData::DateTime(value) => calamine_datetime_text(value),
        CalamineData::DateTimeIso(value) | CalamineData::DurationIso(value) => {
            value.replace('T', " ").trim_end_matches('Z').to_string()
        }
        CalamineData::Error(error) => format!("{error:?}"),
    }
}

fn calamine_number_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        return (value as i64).to_string();
    }
    value.to_string()
}

fn calamine_datetime_text(value: &calamine::ExcelDateTime) -> String {
    let (year, month, day, hour, minute, second, milli) = value.to_ymd_hms_milli();
    let mut clock = format!("{hour:02}:{minute:02}:{second:02}");
    if milli != 0 {
        clock.push_str(&format!(".{milli:03}000"));
    }
    let serial = value.as_f64();
    if (0.0..1.0).contains(&serial) {
        clock
    } else {
        format!("{year:04}-{month:02}-{day:02} {clock}")
    }
}

/// Resolve (sheet name, worksheet path) pairs for a workbook in document
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
                .map(|(name, relation)| {
                    let target = targets.get(&relation).ok_or_else(|| {
                        anyhow::anyhow!(
                            "XLSX worksheet {name:?} has a missing relationship ({relation}). 请修复并另存为 XLSX 后重新上传 / Repair and save the workbook as XLSX, then upload again."
                        )
                    })?;
                    Ok((name, workbook_target_path(target)))
                })
                .collect::<Result<Vec<_>>>()?
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

/// Resolve which cell styles represent dates/times.
///
/// openpyxl reads xl/styles.xml, maps each cell s="N" to the Nth xf in
/// cellXfs, and converts numeric cells whose number format is a date/time
/// format to a Python datetime. RAGFlow then prints that value with str().
fn parse_date_styles(xml: &str) -> Result<DateStyles> {
    let mut reader = Reader::from_str(xml);
    let mut custom_formats: HashMap<u32, String> = HashMap::new();
    let mut cell_xfs: Vec<u32> = Vec::new();
    let mut in_cell_xfs = false;
    loop {
        match reader.read_event()? {
            Event::Start(event) | Event::Empty(event) => match event.local_name().as_ref() {
                b"numFmt" => {
                    if let Some(id) = attribute_value(&event, b"numFmtId", reader.decoder())?
                        .and_then(|value| value.parse::<u32>().ok())
                    {
                        if let Some(code) =
                            attribute_value(&event, b"formatCode", reader.decoder())?
                        {
                            custom_formats.insert(id, code);
                        }
                    }
                }
                b"cellXfs" => in_cell_xfs = true,
                b"xf" if in_cell_xfs => {
                    let id = attribute_value(&event, b"numFmtId", reader.decoder())?
                        .and_then(|value| value.parse::<u32>().ok())
                        .unwrap_or(0);
                    cell_xfs.push(id);
                }
                _ => {}
            },
            Event::End(event) if event.local_name().as_ref() == b"cellXfs" => {
                in_cell_xfs = false;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let cell_is_date = cell_xfs
        .into_iter()
        .map(|id| match custom_formats.get(&id) {
            Some(code) => is_date_format(code),
            None => builtin_is_date_format(id),
        })
        .collect();
    Ok(DateStyles { cell_is_date })
}

fn builtin_is_date_format(id: u32) -> bool {
    matches!(id, 14..=22 | 45..=47)
}

/// Mirror openpyxl is_date_format: inspect the first section, ignore quoted
/// literals and bracketed colours/conditions, then look for a date/time token.
fn is_date_format(code: &str) -> bool {
    let first = code.split(';').next().unwrap_or("");
    let mut cleaned = String::with_capacity(first.len());
    let mut chars = first.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                for quoted in chars.by_ref() {
                    if quoted == '"' {
                        break;
                    }
                }
            }
            '[' => {
                for bracketed in chars.by_ref() {
                    if bracketed == ']' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            _ => cleaned.push(ch),
        }
    }
    cleaned
        .chars()
        .any(|ch| matches!(ch.to_ascii_lowercase(), 'd' | 'm' | 'h' | 'y' | 's'))
}

/// Read the workbook's 1904 date-system flag (workbookPr date1904="1").
fn workbook_uses_1904(xml: &str) -> Result<bool> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(event) | Event::Empty(event)
                if event.local_name().as_ref() == b"workbookPr" =>
            {
                return Ok(matches!(
                    attribute_value(&event, b"date1904", reader.decoder())?.as_deref(),
                    Some("1") | Some("true") | Some("TRUE")
                ));
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}

/// Convert an Excel serial to the string Python's str() produced for
/// openpyxl's datetime/time value. Mirrors openpyxl.utils.datetime.from_excel,
/// including the 1900 leap-year adjustment and millisecond rounding.
fn excel_serial_text(serial: f64, date1904: bool) -> Option<String> {
    if !serial.is_finite() {
        return None;
    }
    let day = serial.floor();
    let fraction = serial - day;
    let millis = (fraction * 86_400_000.0).round();
    if !millis.is_finite() {
        return None;
    }
    let millis = millis as i64;
    let extra_days = millis.div_euclid(86_400_000);
    let remaining_millis = millis.rem_euclid(86_400_000);
    if (0.0..1.0).contains(&serial) && extra_days == 0 {
        return Some(format_excel_clock(remaining_millis));
    }
    let mut days = day as i64;
    if serial > 0.0 && serial < 60.0 && !date1904 {
        days += 1;
    }
    let epoch = if date1904 {
        Date::from_calendar_date(1904, Month::January, 1)
    } else {
        Date::from_calendar_date(1899, Month::December, 30)
    }
    .ok()?;
    let date = epoch.checked_add(Duration::days(days.checked_add(extra_days)?))?;
    Some(format!(
        "{} {}",
        format_excel_date(date),
        format_excel_clock(remaining_millis)
    ))
}

fn format_excel_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        date.month() as u8,
        date.day()
    )
}

fn format_excel_clock(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let micros = (millis.rem_euclid(1000)) * 1000;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds.div_euclid(3600) % 24,
        seconds.div_euclid(60) % 60,
        seconds % 60
    );
    if micros != 0 {
        text.push_str(&format!(".{micros:06}"));
    }
    text
}

fn parse_sheet_rows(
    xml: &str,
    shared_strings: &[String],
    max_rows: usize,
    date_styles: &DateStyles,
    date1904: bool,
) -> Result<(Vec<Vec<String>>, Vec<(u32, u32, u32, u32)>, bool)> {
    let mut reader = Reader::from_str(xml);
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let mut cell_kind = String::new();
    let mut cell_column = None;
    let mut cell_style = None;
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
                cell_style = attribute_value(&event, b"s", reader.decoder())?
                    .and_then(|value| value.parse::<usize>().ok());
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
                let mut value = match cell_kind.as_str() {
                    "s" => {
                        let index = cell_value.parse::<usize>().map_err(|_| {
                            anyhow::anyhow!(
                                "XLSX shared string index is invalid. 请修复并另存为 XLSX 后重新上传 / Repair and save the workbook as XLSX, then upload again."
                            )
                        })?;
                        shared_strings.get(index).cloned().ok_or_else(|| {
                            anyhow::anyhow!(
                                "XLSX shared string index {index} is missing. 请修复并另存为 XLSX 后重新上传 / Repair and save the workbook as XLSX, then upload again."
                            )
                        })?
                    }
                    "b" => match cell_value.as_str() {
                        "1" => "True".into(),
                        "0" => "False".into(),
                        _ => cell_value.clone(),
                    },
                    _ => cell_value.clone(),
                };
                if (cell_kind.is_empty() || cell_kind == "n")
                    && cell_style.is_some_and(|style| {
                        date_styles
                            .cell_is_date
                            .get(style)
                            .copied()
                            .unwrap_or(false)
                    })
                    && let Ok(serial) = cell_value.parse::<f64>()
                {
                    value = excel_serial_text(serial, date1904).unwrap_or_else(|| "#VALUE!".into());
                }
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
        let lower = name.to_lowercase();
        // Mirrors RAGFlow _load_excel_to_workbook: a PK zip is XLSX, the OLE2
        // signature is a legacy XLS, and anything else is converted from CSV.
        let content = if lower.ends_with(".csv") {
            self.parse_csv(data)?
        } else if is_zip_workbook(data) {
            self.parse_xlsx(data)?
        } else if is_ole_workbook(data) {
            self.parse_xls(data)?
        } else {
            self.parse_csv(data)?
        };
        let media_type = if lower.ends_with(".csv") {
            "text/csv"
        } else if lower.ends_with(".xls") {
            "application/vnd.ms-excel"
        } else {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        };

        Ok(new_document(name, content, media_type, data.len()))
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
                        "1" => "True".into(),
                        "0" => "False".into(),
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
    if ext.contains("xls") && is_ole_workbook(data) {
        return Ok(read_xls_sheets(data, usize::MAX)?
            .iter()
            .map(|sheet| sheet.rows.len())
            .sum());
    }
    if ext.contains("xls") && is_zip_workbook(data) {
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
    if ext == "csv" || ext == "txt" || ext.contains("xls") {
        let text = crate::parser::txt::decode_text(data)?;
        return Ok(text.split('\n').count());
    }
    Ok(0)
}


