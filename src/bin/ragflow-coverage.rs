//! Validate, refresh and baseline-migrate the pinned RAGFlow file coverage ledger.

use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Pinned RAGFlow baseline (tag `v0.27.2`, release commit a024bea).
const FIXED_COMMIT: &str = "a024bea0cd93f39e6652a42bf84dd20c55bc560b";
const BASELINE_TAG: &str = "v0.27.2";
const LEDGER_PATH: &str = "docs/ragflow-v0.27.2-coverage.md";
const DELTA_PATH: &str = "docs/ragflow-v0.27.2-delta.md";
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
    mapping: String,
    note: String,
}

#[derive(Debug, Clone)]
struct OldRow {
    blob_prefix: String,
    status: String,
    mapping: String,
    note: String,
}

#[derive(Debug)]
struct Options {
    write: bool,
    migrate: Option<PathBuf>,
    source: PathBuf,
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
    let options = parse_args()?;
    verify_commit(&options.source)?;
    let ledger_path = PathBuf::from(LEDGER_PATH);
    let entries = git_entries(&options.source)?;
    let mut write = options.write;

    if let Some(old_ledger) = &options.migrate {
        let old_markdown = std::fs::read_to_string(old_ledger)
            .with_context(|| format!("failed to read {}", old_ledger.display()))?;
        let (ledger, delta) = migrate_ledger(&old_markdown, &entries)?;
        crate_atomic_write(&ledger_path, ledger.as_bytes())?;
        crate_atomic_write(&PathBuf::from(DELTA_PATH), delta.as_bytes())?;
        println!(
            "coverage ledger migrated: {} files from {}",
            entries.len(),
            old_ledger.display()
        );
        write = true;
    }

    let original = std::fs::read_to_string(&ledger_path)
        .with_context(|| format!("failed to read {}", ledger_path.display()))?;
    let mut rows = parse_inventory(&original)?;
    validate_inventory(&options.source, &mut rows, &entries, write)?;
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
            options.source.display()
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

fn parse_args() -> Result<Options> {
    let mut write = false;
    let mut migrate = None;
    let mut source = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => {}
            "--write" => write = true,
            "--migrate" => {
                let value = args
                    .next()
                    .context("--migrate requires the previous ledger path")?;
                migrate = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                println!(
                    "Usage: ragflow-coverage [--check|--write] [--migrate OLD_LEDGER] [RAGFLOW_CHECKOUT]\n\
                     Defaults to $RAGFLOW_SOURCE or $HOME/.cache/ragflow-{BASELINE_TAG}.\n\
                     --migrate carries statuses from the previous ledger into the pinned tree."
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
        .unwrap_or_else(|| {
            std::env::var_os("HOME").map_or_else(
                || PathBuf::from(format!("ragflow-{BASELINE_TAG}")),
                |home| {
                    PathBuf::from(home)
                        .join(".cache")
                        .join(format!("ragflow-{BASELINE_TAG}"))
                },
            )
        });
    Ok(Options {
        write,
        migrate,
        source,
    })
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
        let row = parse_inventory_row(line)?;
        if !paths.insert(row.path.clone()) {
            bail!("duplicate inventory path: {}", row.path);
        }
        rows.push(row);
    }
    if rows.is_empty() {
        bail!("File Inventory is empty");
    }
    Ok(rows)
}

fn parse_inventory_row(line: &str) -> Result<InventoryRow> {
    let mut fields = line.splitn(7, '|');
    let _leading = fields
        .next()
        .context("inventory row is missing its leading pipe")?;
    let path = strip_ticks(fields.next().context("inventory path missing")?.trim())?.to_owned();
    let lines_cell = fields.next().context("inventory line count missing")?;
    let source_lines = match lines_cell.trim() {
        "-" => None,
        value => Some(
            value
                .parse()
                .with_context(|| format!("invalid source line count for {path}"))?,
        ),
    };
    let bytes = fields
        .next()
        .context("inventory byte count missing")?
        .trim()
        .parse()
        .with_context(|| format!("invalid byte count for {path}"))?;
    let blob_prefix =
        strip_ticks(fields.next().context("inventory blob missing")?.trim())?.to_owned();
    let status = fields
        .next()
        .context("inventory status missing")?
        .trim()
        .to_owned();
    if !STATUSES.contains(&status.as_str()) {
        bail!("unknown status {status:?} for {path}");
    }
    let rest = fields.next().context("inventory mapping/note missing")?;
    let rest = rest.trim();
    let rest = rest.strip_suffix('|').unwrap_or(rest).trim_end();
    let (mapping, note) = match rest.split_once('|') {
        Some((mapping, note)) => (mapping.trim().to_owned(), note.trim().to_owned()),
        None => (rest.to_owned(), String::new()),
    };
    Ok(InventoryRow {
        path,
        source_lines,
        bytes,
        blob_prefix,
        status,
        mapping,
        note,
    })
}

fn strip_ticks(value: &str) -> Result<&str> {
    value
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .with_context(|| format!("expected backtick-delimited value, got {value:?}"))
}

fn render_row(row: &InventoryRow) -> String {
    let source_lines = row
        .source_lines
        .map_or_else(|| "-".to_owned(), |lines| lines.to_string());
    format!(
        "| `{}` | {} | {} | `{}` | {} | {} | {} |",
        row.path, source_lines, row.bytes, row.blob_prefix, row.status, row.mapping, row.note
    )
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
            let parsed = parse_inventory_row(line)?;
            let row = by_path
                .get(parsed.path.as_str())
                .with_context(|| format!("inventory row disappeared: {}", parsed.path))?;
            output.push_str(&render_row(row));
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

/// Previous baseline tag extracted from the old ledger title (for example `v0.26.4`).
fn previous_baseline(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# RAGFlow "))
        .and_then(|rest| rest.strip_suffix(" Coverage Matrix"))
        .map(str::to_owned)
}

fn is_test_path(path: &str) -> bool {
    if path
        .split('/')
        .any(|segment| segment == "test" || segment == "tests")
    {
        return true;
    }
    path.contains("_test.") || path.contains("test_")
}

/// Mirror the classification rules already recorded in the v0.26.4 ledger so a
/// baseline migration does not invent new references: internal Go code, upstream
/// tests, upstream documentation and repository automation stay `reference`;
/// anything else starts `unmapped` and waits for a real audit.
fn classify_added(path: &str) -> (String, String, String) {
    if path.starts_with("internal/") {
        return (
            "reference".to_owned(),
            "`src/; Dockerfile; docker-compose.yml`".to_owned(),
            "Go infrastructure (non-Python parity scope); functionality covered by RayRAG Rust modules"
                .to_owned(),
        );
    }
    if is_test_path(path) {
        return (
            "reference".to_owned(),
            "`Rust unit and HTTP tests`".to_owned(),
            "Upstream tests, benchmarks and fixtures guide Rust compatibility verification and are not production runtime"
                .to_owned(),
        );
    }
    if path.starts_with("docs/") || path.ends_with(".md") || path.ends_with(".mdx") {
        return (
            "reference".to_owned(),
            "`README.md; MAINTENANCE.md; docs/`".to_owned(),
            "Upstream documentation is retained as audit reference; RayRAG maintains its own Rust deployment and compatibility records"
                .to_owned(),
        );
    }
    if path.starts_with(".github/") {
        return (
            "reference".to_owned(),
            "`-`".to_owned(),
            "Upstream repository automation is reference material, not RayRAG runtime".to_owned(),
        );
    }
    (
        "unmapped".to_owned(),
        "`-`".to_owned(),
        format!("Added in RAGFlow {BASELINE_TAG}; pending audit"),
    )
}

fn migrate_ledger(old_markdown: &str, entries: &[GitEntry]) -> Result<(String, String)> {
    let previous =
        previous_baseline(old_markdown).unwrap_or_else(|| "previous baseline".to_owned());
    let old_rows: BTreeMap<String, OldRow> = parse_inventory(old_markdown)?
        .into_iter()
        .map(|row| {
            (
                row.path,
                OldRow {
                    blob_prefix: row.blob_prefix,
                    status: row.status,
                    mapping: row.mapping,
                    note: row.note,
                },
            )
        })
        .collect();
    let new_paths: BTreeSet<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    let removed: Vec<&String> = old_rows
        .keys()
        .filter(|path| !new_paths.contains(path.as_str()))
        .collect();

    let mut rows = Vec::with_capacity(entries.len());
    let mut carried = 0usize;
    let mut changed = 0usize;
    let mut added = 0usize;
    let mut auto_reference = 0usize;
    let mut re_audit: Vec<(String, String)> = Vec::new();
    let mut added_unmapped: Vec<String> = Vec::new();
    for entry in entries {
        let blob_prefix: String = entry.blob.chars().take(16).collect();
        if let Some(old_row) = old_rows.get(&entry.path) {
            carried += 1;
            let content_changed = old_row.blob_prefix != blob_prefix;
            let audited = !matches!(old_row.status.as_str(), "reference" | "N/A" | "unmapped");
            let mut note = old_row.note.clone();
            if content_changed {
                changed += 1;
                if audited {
                    if !note.is_empty() {
                        note.push(' ');
                    }
                    note.push_str("[v0.27.2 content changed: re-audit]");
                    re_audit.push((entry.path.clone(), old_row.status.clone()));
                }
            }
            rows.push(InventoryRow {
                path: entry.path.clone(),
                source_lines: None,
                bytes: entry.bytes,
                blob_prefix,
                status: old_row.status.clone(),
                mapping: old_row.mapping.clone(),
                note,
            });
        } else {
            added += 1;
            let (status, mapping, note) = classify_added(&entry.path);
            if status == "unmapped" {
                added_unmapped.push(entry.path.clone());
            } else {
                auto_reference += 1;
            }
            rows.push(InventoryRow {
                path: entry.path.clone(),
                source_lines: None,
                bytes: entry.bytes,
                blob_prefix,
                status,
                mapping,
                note,
            });
        }
    }
    let counts = summarize(&rows)?;
    let old_count = old_rows.len();
    let removed_len = removed.len();
    let re_audit_len = re_audit.len();
    let unmapped_len = added_unmapped.len();
    let mut ledger = String::new();
    ledger.push_str(&format!("# RAGFlow {BASELINE_TAG} Coverage Matrix\n\n"));
    ledger.push_str(&format!("- Fixed commit: `{FIXED_COMMIT}`\n"));
    ledger.push_str(&format!("- RAGFlow files: `{}`\n", rows.len()));
    ledger.push_str(&format!(
        "- Baseline migration: carried {carried} rows from the RAGFlow {previous} ledger; {changed} rows changed upstream and the {re_audit_len} audited ones are marked `[v0.27.2 content changed: re-audit]`; {added} paths are new ({auto_reference} matched the upstream reference rules — internal Go / tests / docs / repository automation; {unmapped_len} start as `unmapped`); {removed_len} paths were removed upstream.\n"
    ));
    ledger.push_str(&format!(
        "- Detailed delta: `{DELTA_PATH}` lists the re-audit queue, the added paths and the removed paths.\n"
    ));
    ledger.push_str(&format!(
        "- Inventory validation: `cargo run --locked --bin ragflow-coverage -- --check\n  /path/to/ragflow-{BASELINE_TAG}` validates the fixed commit plus every Git path/blob/byte/text-line\n  and the summary; use `--write` after an intentional status edit to refresh metadata and totals.\n"
    ));
    ledger.push_str("- Policy: `aligned` means audited behavior, not a literal Python-to-Rust transliteration. `replaced` records an intentional Rust/PostgreSQL/zvec substitute. `Source lines` are counted from the pinned Git blobs; binary blobs are shown as `-`.\n\n");
    ledger.push_str("- Classification guard: unknown paths become `unmapped`; only explicit mappings or test/fixture rules may produce `reference`.\n");
    ledger.push_str("- `internal/` (Go production code): classified `reference` — RAGFlow's Go sub-project is outside the Python parity scope; its functionality (admin API, binding, cache, cli, dao) is covered by RayRAG's Rust modules (`src/server.rs`, `src/store/`, `src/api/`).\n");
    ledger.push_str("- `N/A-by-design`: non-Rust targets and non-code artifacts (Node/Go entrypoints, documentation, packaging/lockfile metadata, empty package markers) that are intentionally outside the Rust port scope; the audit note records where the equivalent semantics live, if any.\n\n");
    ledger.push_str("## Summary\n\n| Status | Files | Text source lines |\n|---|---:|---:|\n");
    for status in STATUSES {
        let count = counts.get(*status).context("status count missing")?;
        ledger.push_str(&format!(
            "| {status} | {} | {} |\n",
            count.files, count.lines
        ));
    }
    ledger.push('\n');
    if !removed.is_empty() {
        ledger.push_str("## Removed upstream paths\n\n");
        ledger.push_str(&format!(
            "These {removed_len} path(s) existed in the RAGFlow {previous} ledger and no longer exist in {BASELINE_TAG}:\n\n"
        ));
        for path in &removed {
            ledger.push_str(&format!("- `{path}`\n"));
        }
        ledger.push('\n');
    }
    ledger.push_str("## File Inventory\n\n");
    for row in &rows {
        ledger.push_str(&render_row(row));
        ledger.push('\n');
    }

    let mut delta = String::new();
    delta.push_str(&format!(
        "# RAGFlow {BASELINE_TAG} Baseline Delta (from RAGFlow {previous})\n\n"
    ));
    delta.push_str("Generated once by `ragflow-coverage --migrate`; internal engineering material, excluded from the published snapshot.\n\n");
    delta.push_str("## Counts\n\n");
    delta.push_str(&format!(
        "- Files in the RAGFlow {previous} ledger: {old_count}\n"
    ));
    delta.push_str(&format!(
        "- Files in the {BASELINE_TAG} tree: {}\n",
        entries.len()
    ));
    delta.push_str(&format!("- Added: {added}\n"));
    delta.push_str(&format!("- Removed: {removed_len}\n"));
    delta.push_str(&format!("- Content changed: {changed}\n"));
    delta.push_str(&format!(
        "- Re-audit queue (audited rows changed upstream): {re_audit_len}\n\n"
    ));
    delta.push_str("## Re-audit queue\n\n");
    for (path, status) in &re_audit {
        delta.push_str(&format!("- `{path}` — previous status: `{status}`\n"));
    }
    delta.push('\n');
    delta.push_str(&format!("## Added paths — unmapped ({unmapped_len})\n\n"));
    for path in &added_unmapped {
        delta.push_str(&format!("- `{path}`\n"));
    }
    delta.push('\n');
    delta.push_str("## Removed paths\n\n");
    for path in &removed {
        delta.push_str(&format!("- `{path}`\n"));
    }
    delta.push('\n');
    Ok((ledger, delta))
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

    const SAMPLE_ROW: &str = "| `conf/system_settings.json` | 88 | 1843 | `f546aa1436b0d262` | partial | `src/settings.rs` | note |";

    #[test]
    fn parses_inventory_rows_and_rejects_duplicates() {
        let rows = parse_inventory(SAMPLE_ROW).unwrap();
        assert_eq!(rows[0].path, "conf/system_settings.json");
        assert_eq!(rows[0].source_lines, Some(88));
        assert_eq!(rows[0].status, "partial");
        assert_eq!(rows[0].mapping, "`src/settings.rs`");
        assert_eq!(rows[0].note, "note");
        assert!(parse_inventory(&format!("{SAMPLE_ROW}\n{SAMPLE_ROW}")).is_err());
    }

    #[test]
    fn inventory_rows_round_trip_through_parser_and_renderer() {
        let row = parse_inventory_row(SAMPLE_ROW).unwrap();
        assert_eq!(render_row(&row), SAMPLE_ROW);
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
            mapping: "`src/x.rs`".into(),
            note: "audited".into(),
        }];
        let refreshed = refresh_summary(markdown, &summarize(&rows).unwrap(), 1).unwrap();
        assert!(refreshed.contains("- RAGFlow files: `1`"));
        assert!(refreshed.contains("| aligned | 1 | 3 |"));
    }

    #[test]
    fn classifies_added_paths_by_upstream_rules() {
        assert_eq!(
            classify_added("internal/admin/service_test.go").0,
            "reference"
        );
        assert_eq!(classify_added("rag/svr/task_executor.py").0, "unmapped");
        assert_eq!(classify_added("web/src/pages/home/index.tsx").0, "unmapped");
        assert_eq!(classify_added("test/unit/foo_test.py").0, "reference");
        assert_eq!(classify_added("docs/guides/foo.mdx").0, "reference");
        assert_eq!(classify_added(".github/workflows/x.yml").0, "reference");
        assert_eq!(classify_added("README_ru.md").0, "reference");
    }

    #[test]
    fn migrates_statuses_and_marks_changed_audited_rows() {
        let summary = "## Summary\n\n| Status | Files | Text source lines |\n|---|---:|---:|\n| aligned | 0 | 0 |\n| full | 0 | 0 |\n| partial | 0 | 0 |\n| replaced | 0 | 0 |\n| N/A | 0 | 0 |\n| reference | 0 | 0 |\n| unmapped | 0 | 0 |\n";
        let old = format!(
            "# RAGFlow v0.26.4 Coverage Matrix\n\n{summary}\n## File Inventory\n\n| `a.py` | 3 | 3 | `aaaaaaaaaaaaaaaa` | aligned | `src/a.rs` | audited |\n| `internal/x.go` | 3 | 3 | `bbbbbbbbbbbbbbbb` | reference | `src/` | go |\n| `gone.py` | 3 | 3 | `cccccccccccccccc` | reference | `-` | old |\n"
        );
        let entries = vec![
            GitEntry {
                path: "a.py".into(),
                blob: "aaaaaaaaaaaaaaaa0000".into(),
                bytes: 4,
            },
            GitEntry {
                path: "internal/x.go".into(),
                blob: "dddddddddddddddd0000".into(),
                bytes: 5,
            },
            GitEntry {
                path: "rag/new.py".into(),
                blob: "eeeeeeeeeeeeeeee0000".into(),
                bytes: 6,
            },
            GitEntry {
                path: "docs/new.mdx".into(),
                blob: "ffffffffffffffff0000".into(),
                bytes: 7,
            },
        ];
        let (ledger, delta) = migrate_ledger(&old, &entries).unwrap();
        assert!(
            ledger.contains(
                "| `a.py` | - | 4 | `aaaaaaaaaaaaaaaa` | aligned | `src/a.rs` | audited |"
            )
        );
        assert!(ledger.contains(
            "| `internal/x.go` | - | 5 | `dddddddddddddddd` | reference | `src/` | go |"
        ));
        assert!(ledger.contains("| `rag/new.py` | - | 6 | `eeeeeeeeeeeeeeee` | unmapped | `-` | Added in RAGFlow v0.27.2; pending audit |"));
        assert!(ledger.contains("| `docs/new.mdx` | - | 7 | `ffffffffffffffff` | reference |"));
        assert!(ledger.contains("- `gone.py`"));
        assert!(delta.contains("## Re-audit queue"));
        assert!(delta.contains("## Added paths — unmapped (1)"));
        assert!(!delta.contains("`a.py` — previous status"));
        assert!(delta.contains("- `rag/new.py`"));
    }

    #[test]
    fn marks_changed_audited_rows_for_re_audit() {
        let summary = "## Summary\n\n| Status | Files | Text source lines |\n|---|---:|---:|\n| aligned | 0 | 0 |\n| full | 0 | 0 |\n| partial | 0 | 0 |\n| replaced | 0 | 0 |\n| N/A | 0 | 0 |\n| reference | 0 | 0 |\n| unmapped | 0 | 0 |\n";
        let old = format!(
            "# RAGFlow v0.26.4 Coverage Matrix\n\n{summary}\n## File Inventory\n\n| `web/src/x.tsx` | 3 | 3 | `1111111111111111` | aligned | `src/web.rs` | audited |\n| `rag/y.py` | 3 | 3 | `2222222222222222` | reference | `src/` | ref |\n"
        );
        let entries = vec![
            GitEntry {
                path: "web/src/x.tsx".into(),
                blob: "33333333333333330000".into(),
                bytes: 4,
            },
            GitEntry {
                path: "rag/y.py".into(),
                blob: "44444444444444440000".into(),
                bytes: 4,
            },
        ];
        let (ledger, delta) = migrate_ledger(&old, &entries).unwrap();
        assert!(ledger.contains("| `web/src/x.tsx` | - | 4 | `3333333333333333` | aligned | `src/web.rs` | audited [v0.27.2 content changed: re-audit] |"));
        assert!(
            ledger
                .contains("| `rag/y.py` | - | 4 | `4444444444444444` | reference | `src/` | ref |")
        );
        assert!(delta.contains("- `web/src/x.tsx` — previous status: `aligned`"));
    }
}
