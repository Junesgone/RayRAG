//! Validate and refresh the pinned RAGFlow file coverage ledger.

use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const FIXED_COMMIT: &str = "cb93883f3f8c975eecb2fed81210effeb3bdb06f";
const LEDGER_PATH: &str = "docs/ragflow-v0.26.4-coverage.md";
const STATUSES: &[&str] = &[
    "aligned",
    "full",
    "partial",
    "replaced",
    "N/A",
    "reference",
    "unmapped",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct InventoryRow {
    path: String,
    source_lines: Option<usize>,
    bytes: usize,
    blob_prefix: String,
    status: String,
}

#[derive(Debug)]
struct GitEntry {
    path: String,
    blob: String,
    bytes: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StatusCount {
    files: usize,
    lines: usize,
}

fn main() -> Result<()> {
    let (write, source) = parse_args()?;
    verify_commit(&source)?;
    let ledger_path = PathBuf::from(LEDGER_PATH);
    let original = std::fs::read_to_string(&ledger_path)
        .with_context(|| format!("failed to read {}", ledger_path.display()))?;
    let mut rows = parse_inventory(&original)?;
    let entries = git_entries(&source)?;
    validate_inventory(&source, &mut rows, &entries, write)?;
    let counts = summarize(&rows)?;
    let inventory_refreshed = refresh_inventory(&original, &rows)?;
    let refreshed = refresh_summary(&inventory_refreshed, &counts, rows.len())?;

    if write {
        if refreshed != original {
            crate_atomic_write(&ledger_path, refreshed.as_bytes())?;
        }
        println!(
            "coverage ledger refreshed: {} files, unmapped={}",
            rows.len(),
            counts.get("unmapped").map_or(0, |count| count.files)
        );
    } else if refreshed != original {
        bail!(
            "coverage Summary is stale; run `cargo run --locked --bin ragflow-coverage -- --write {}`",
            source.display()
        );
    } else {
        println!(
            "coverage ledger verified: commit={}, files={}, unmapped={}",
            FIXED_COMMIT,
            rows.len(),
            counts.get("unmapped").map_or(0, |count| count.files)
        );
    }
    Ok(())
}

fn parse_args() -> Result<(bool, PathBuf)> {
    let mut write = false;
    let mut source = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--check" => {}
            "--write" => write = true,
            "-h" | "--help" => {
                println!(
                    "Usage: ragflow-coverage [--check|--write] [RAGFLOW_CHECKOUT]\n\
                     Defaults to $RAGFLOW_SOURCE or /tmp/ragflow-v0.26.4."
                );
                std::process::exit(0);
            }
            value if value.starts_with('-') => bail!("unknown option: {value}"),
            value if source.is_none() => source = Some(PathBuf::from(value)),
            value => bail!("unexpected argument: {value}"),
        }
    }
    let source = source
        .or_else(|| std::env::var_os("RAGFLOW_SOURCE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp/ragflow-v0.26.4"));
    Ok((write, source))
}

fn verify_commit(source: &Path) -> Result<()> {
    let output = git_output(source, &["rev-parse", "HEAD"])?;
    let head = String::from_utf8(output)?.trim().to_owned();
    if head != FIXED_COMMIT {
        bail!(
            "RAGFlow checkout {} is at {head}, expected {FIXED_COMMIT}",
            source.display()
        );
    }
    Ok(())
}

fn parse_inventory(markdown: &str) -> Result<Vec<InventoryRow>> {
    let mut rows = Vec::new();
    let mut paths = BTreeSet::new();
    for line in markdown.lines().filter(|line| line.starts_with("| `")) {
        let columns: Vec<_> = line.split('|').map(str::trim).collect();
        if columns.len() < 8 {
            bail!("malformed inventory row: {line}");
        }
        let path = strip_ticks(columns[1])?.to_owned();
        if !paths.insert(path.clone()) {
            bail!("duplicate inventory path: {path}");
        }
        let source_lines = match columns[2] {
            "-" => None,
            value => Some(
                value
                    .parse()
                    .with_context(|| format!("invalid source line count for {path}"))?,
            ),
        };
        let bytes = columns[3]
            .parse()
            .with_context(|| format!("invalid byte count for {path}"))?;
        let blob_prefix = strip_ticks(columns[4])?.to_owned();
        let status = columns[5].to_owned();
        if !STATUSES.contains(&status.as_str()) {
            bail!("unknown status {status:?} for {path}");
        }
        rows.push(InventoryRow {
            path,
            source_lines,
            bytes,
            blob_prefix,
            status,
        });
    }
    if rows.is_empty() {
        bail!("File Inventory is empty");
    }
    Ok(rows)
}

fn strip_ticks(value: &str) -> Result<&str> {
    value
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .with_context(|| format!("expected backtick-delimited value, got {value:?}"))
}

fn git_entries(source: &Path) -> Result<Vec<GitEntry>> {
    let output = git_output(
        source,
        &["ls-tree", "-r", "-l", "-z", "--full-tree", "HEAD"],
    )?;
    let mut entries = Vec::new();
    for raw in output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let tab = raw
            .iter()
            .position(|byte| *byte == b'\t')
            .context("git ls-tree entry has no path separator")?;
        let metadata = std::str::from_utf8(&raw[..tab])?;
        let path = std::str::from_utf8(&raw[tab + 1..])?.to_owned();
        let fields: Vec<_> = metadata.split_whitespace().collect();
        if fields.len() != 4 || fields[1] != "blob" {
            bail!("unsupported git tree entry: {metadata} {path}");
        }
        let bytes = fields[3]
            .parse()
            .with_context(|| format!("invalid git blob size for {path}"))?;
        entries.push(GitEntry {
            path,
            blob: fields[2].to_owned(),
            bytes,
        });
    }
    Ok(entries)
}

fn validate_inventory(
    source: &Path,
    rows: &mut [InventoryRow],
    entries: &[GitEntry],
    repair: bool,
) -> Result<()> {
    let by_path: BTreeMap<_, _> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.path.clone(), index))
        .collect();
    if entries.len() != rows.len() {
        bail!(
            "inventory/tree file count mismatch: ledger={}, git={}",
            rows.len(),
            entries.len()
        );
    }

    let mut child = Command::new("git")
        .args(["-C", source.to_str().context("non-UTF-8 source path")?])
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start git cat-file --batch")?;
    let mut input = BufWriter::new(child.stdin.take().context("git cat-file stdin missing")?);
    let mut output = BufReader::new(child.stdout.take().context("git cat-file stdout missing")?);

    for entry in entries {
        let row_index = *by_path
            .get(&entry.path)
            .with_context(|| format!("git path missing from inventory: {}", entry.path))?;
        writeln!(input, "{}", entry.blob)?;
        input.flush()?;
        let mut header = String::new();
        output.read_line(&mut header)?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 || fields[1] != "blob" {
            bail!(
                "unexpected git cat-file header for {}: {header:?}",
                entry.path
            );
        }
        let size: usize = fields[2].parse()?;
        if size != entry.bytes {
            bail!("git cat-file size changed for {}", entry.path);
        }
        let mut bytes = vec![0; size];
        output.read_exact(&mut bytes)?;
        let mut newline = [0];
        output.read_exact(&mut newline)?;
        if newline[0] != b'\n' {
            bail!("git cat-file framing error for {}", entry.path);
        }
        let actual_lines = std::str::from_utf8(&bytes)
            .ok()
            .map(|text| text.lines().count());
        let row = &mut rows[row_index];
        if repair {
            row.source_lines = actual_lines;
            row.bytes = entry.bytes;
            row.blob_prefix = entry.blob.chars().take(16).collect();
        } else {
            if row.bytes != entry.bytes {
                bail!(
                    "byte count mismatch for {}: ledger={}, git={}",
                    entry.path,
                    row.bytes,
                    entry.bytes
                );
            }
            if !entry.blob.starts_with(&row.blob_prefix) {
                bail!(
                    "blob mismatch for {}: ledger={}, git={}",
                    entry.path,
                    row.blob_prefix,
                    entry.blob
                );
            }
            if actual_lines != row.source_lines {
                bail!(
                    "source line mismatch for {}: ledger={:?}, git={actual_lines:?}",
                    entry.path,
                    row.source_lines
                );
            }
        }
    }
    drop(input);
    let status = child.wait()?;
    if !status.success() {
        bail!("git cat-file --batch exited with {status}");
    }
    Ok(())
}

fn refresh_inventory(markdown: &str, rows: &[InventoryRow]) -> Result<String> {
    let by_path: BTreeMap<_, _> = rows.iter().map(|row| (row.path.as_str(), row)).collect();
    let mut output = String::new();
    for line in markdown.lines() {
        if line.starts_with("| `") {
            let mut fields = line.splitn(7, '|');
            let _empty = fields.next();
            let path_cell = fields.next().context("inventory path missing")?.trim();
            let path = strip_ticks(path_cell)?;
            let _lines = fields.next().context("inventory line count missing")?;
            let _bytes = fields.next().context("inventory byte count missing")?;
            let _blob = fields.next().context("inventory blob missing")?;
            let _status = fields.next().context("inventory status missing")?;
            let rest = fields.next().context("inventory mapping/note missing")?;
            let row = by_path
                .get(path)
                .with_context(|| format!("inventory row disappeared: {path}"))?;
            let source_lines = row
                .source_lines
                .map_or_else(|| "-".into(), |lines| lines.to_string());
            output.push_str(&format!(
                "| `{}` | {} | {} | `{}` | {} | {}",
                row.path,
                source_lines,
                row.bytes,
                row.blob_prefix,
                row.status,
                rest.trim_start()
            ));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    Ok(output)
}

fn summarize(rows: &[InventoryRow]) -> Result<BTreeMap<String, StatusCount>> {
    let mut counts = BTreeMap::new();
    for status in STATUSES {
        counts.insert((*status).to_owned(), StatusCount::default());
    }
    for row in rows {
        let count = counts
            .get_mut(&row.status)
            .with_context(|| format!("unknown status {}", row.status))?;
        count.files += 1;
        count.lines += row.source_lines.unwrap_or(0);
    }
    Ok(counts)
}

fn refresh_summary(
    markdown: &str,
    counts: &BTreeMap<String, StatusCount>,
    total_files: usize,
) -> Result<String> {
    let lines: Vec<_> = markdown.lines().collect();
    let summary = lines
        .iter()
        .position(|line| *line == "## Summary")
        .context("missing Summary section")?;
    let inventory = lines
        .iter()
        .position(|line| *line == "## File Inventory")
        .context("missing File Inventory section")?;
    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        let replacement = if line.starts_with("- RAGFlow files:") {
            Some(format!("- RAGFlow files: `{total_files}`"))
        } else if index > summary && index < inventory {
            STATUSES.iter().find_map(|status| {
                line.starts_with(&format!("| {status} |")).then(|| {
                    let count = counts.get(*status).expect("status count was initialized");
                    format!("| {status} | {} | {} |", count.files, count.lines)
                })
            })
        } else {
            None
        };
        output.push_str(replacement.as_deref().unwrap_or(line));
        output.push('\n');
    }
    Ok(output)
}

fn git_output(source: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git in {}", source.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn crate_atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inventory_rows_and_rejects_duplicates() {
        let line = "| `conf/system_settings.json` | 88 | 1843 | `f546aa1436b0d262` | partial | `src/settings.rs` | note |";
        let rows = parse_inventory(line).unwrap();
        assert_eq!(rows[0].path, "conf/system_settings.json");
        assert_eq!(rows[0].source_lines, Some(88));
        assert_eq!(rows[0].status, "partial");
        assert!(parse_inventory(&format!("{line}\n{line}")).is_err());
    }

    #[test]
    fn refreshes_summary_from_inventory_counts() {
        let markdown = "- RAGFlow files: `9`\n\n## Summary\n\n| Status | Files | Text source lines |\n|---|---:|---:|\n| aligned | 0 | 0 |\n| full | 0 | 0 |\n| partial | 0 | 0 |\n| replaced | 0 | 0 |\n| N/A | 0 | 0 |\n| reference | 0 | 0 |\n| unmapped | 0 | 0 |\n\n## File Inventory\n";
        let rows = vec![InventoryRow {
            path: "x".into(),
            source_lines: Some(3),
            bytes: 3,
            blob_prefix: "abc".into(),
            status: "aligned".into(),
        }];
        let refreshed = refresh_summary(markdown, &summarize(&rows).unwrap(), 1).unwrap();
        assert!(refreshed.contains("- RAGFlow files: `1`"));
        assert!(refreshed.contains("| aligned | 1 | 3 |"));
    }
}
