//! Import irssi autologs into the archive's `irc_*` tables.
//!
//! Idempotent, and dry-run by default; pass `--apply` to write.
//!
//! ```text
//! rsync -a irc:irclogs/ /some/staging/irclogs/
//! import_irclogs --root /some/staging/irclogs \
//!     --network mynet --network mynet2 --map mynet2=mynet \
//!     --self-nick mynick --self-nick mynick_ [--apply] [--all]
//! ```
//!
//! Under `--apply`, each file's `(mtime, size)` goes into `irc_import_state` and
//! unchanged files are skipped, so a run costs what arrived. `--all` reads
//! everything; use it after changing the parser.
//!
//! `--self-nick` marks your own lines. It is an argument because this
//! repository is public.
//!
//! `--map` merges the second tag irssi invents for a second simultaneous
//! connection (`mynet2`) into the first. The original tag stays in `source_tag`;
//! see migration v8.
//!
//! Config via env, as the ingester: `DB_HOST`, `DB_PORT` (3306), `DB_NAME`,
//! `DB_USER`, `DB_PASSWORD`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use irclog::{IrcLine, Kind, parse_log, parse_map_entry, parse_path, stored_network};
use signal_archiver::db::{Db, IrcConversations};

/// Import an irclogs tree into the archive.
#[derive(Parser)]
struct Args {
    /// A local copy of the irclogs tree.
    #[arg(long)]
    root: PathBuf,
    /// A network to import; repeatable. None means every network found under
    /// the root.
    #[arg(long = "network", value_name = "NAME")]
    networks: Vec<String>,
    /// Source tag to stored network, as `from=to`; repeatable.
    #[arg(long, value_name = "FROM=TO", value_parser = map_entry)]
    map: Vec<(String, String)>,
    /// A nick that is the user's own; repeatable.
    #[arg(long = "self-nick", value_name = "NICK")]
    self_nicks: Vec<String>,
    /// Write to the archive; without it, only report.
    #[arg(long)]
    apply: bool,
    /// Read every file, whatever `irc_import_state` says. The end-of-run report
    /// describes only the files read, so this is the whole-corpus audit.
    #[arg(long)]
    all: bool,
}

fn map_entry(pair: &str) -> Result<(String, String), String> {
    parse_map_entry(pair).ok_or_else(|| format!("wants from=to, got {pair}"))
}

/// What the run saw, printed at the end.
#[derive(Default)]
struct Report {
    files: u64,
    /// Unchanged since the last `--apply` run, so not opened at all.
    skipped: u64,
    /// Paths with no network component.
    legacy_paths: u64,
    lossy_files: Vec<String>,
    by_kind: BTreeMap<&'static str, u64>,
    inserted: u64,
    duplicates: u64,
    unparsed: u64,
    /// A few examples of unrecognised lines.
    unparsed_examples: Vec<String>,
}

/// Whether a log file has changed: mtime in nanoseconds, and size in bytes.
/// The size catches what a coarse `rsync` mtime might not.
fn file_state(path: &Path) -> Result<(i64, i64)> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64);
    Ok((mtime, meta.len() as i64))
}

/// The part of a snapshot that is safe to parse: up to and including the last
/// newline.
///
/// A line without its newline may still be being written. Imported, it would
/// keep its truncated text forever, since the complete line has the same
/// `line_no`. Left out, it is read whole once the file grows.
fn complete_lines(text: &str) -> &str {
    match text.rfind('\n') {
        Some(i) => &text[..=i],
        None => "",
    }
}

/// Every `*.log` under `root`, as paths relative to it, in sorted order.
///
/// Sorted, because `id` orders lines within a minute and `<net>/<Y>/<M>/<D>`
/// sorts chronologically.
fn collect_logs(root: &Path) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "log") {
                let rel = path
                    .strip_prefix(root)
                    .context("path escaped the root")?
                    .to_string_lossy()
                    .into_owned();
                out.push(rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let db = if args.apply {
        Some(Db::connect(&signal_archiver::db::url_from_env()?).await?)
    } else {
        None
    };

    let logs = collect_logs(&args.root)?;
    let mut report = Report::default();
    let mut conversations = IrcConversations::default();

    // Empty under `--all`, and on a dry run, which has no database connection.
    let already_read = match (&db, args.all) {
        (Some(db), false) => db.irc_import_state().await?,
        _ => HashMap::new(),
    };
    // Files read but not yet recorded as read, awaiting the next flush.
    let mut pending_state: Vec<(String, i64, i64)> = vec![];

    for rel in &logs {
        let Some(path) = parse_path(rel) else {
            report.legacy_paths += 1;
            continue;
        };
        if !args.networks.is_empty() && !args.networks.contains(&path.network) {
            continue;
        }
        let stored_network = stored_network(&args.map, &path.network);

        let full = args.root.join(rel);
        let state = file_state(&full)
            .with_context(|| format!("reading the state of {}", full.display()))?;
        if already_read.get(rel) == Some(&state) {
            report.skipped += 1;
            continue;
        }

        // Lossily: some old logs are not valid UTF-8.
        let bytes = std::fs::read(&full)?;
        let text = match String::from_utf8_lossy(&bytes) {
            std::borrow::Cow::Borrowed(s) => s.to_string(),
            std::borrow::Cow::Owned(s) => {
                report.lossy_files.push(rel.clone());
                s
            }
        };

        let parsed = parse_log(path.date, complete_lines(&text));
        report.files += 1;
        report.unparsed += parsed.unparsed.len() as u64;
        for line_no in &parsed.unparsed {
            if report.unparsed_examples.len() < 20 {
                report.unparsed_examples.push(format!("{rel}:{line_no}"));
            }
        }

        let is_status = args.self_nicks.contains(&path.target);
        let file_date = format!(
            "{:04}-{:02}-{:02}",
            path.date.year, path.date.month, path.date.day
        );

        for entry in &parsed.entries {
            *report.by_kind.entry(entry.kind.as_str()).or_default() += 1;
        }

        let Some(db) = &db else { continue };

        if !parsed.entries.is_empty() {
            let conversation_id = conversations
                .id(db, stored_network, &path.target, is_status)
                .await?;

            let lines: Vec<IrcLine> = parsed
                .entries
                .iter()
                .map(|entry| IrcLine::from_entry(entry, entry.line_no, &args.self_nicks))
                .collect();

            // The source tag, not the mapped network; see migration v8.
            let written = db
                .insert_irc_lines(conversation_id, &path.network, &file_date, &lines)
                .await?;
            report.inserted += written;
            report.duplicates += lines.len() as u64 - written;
        }

        // After its lines are in, empty files included. A file that failed never
        // gets here, so the next run retries it. Flushed with the progress line.
        pending_state.push((rel.clone(), state.0, state.1));

        if report.files.is_multiple_of(500) {
            db.record_irc_imports(&pending_state).await?;
            pending_state.clear();
            println!(
                "  {} files, {} rows written…",
                report.files, report.inserted
            );
        }
    }

    // The final partial batch.
    if let Some(db) = &db
        && !pending_state.is_empty()
    {
        db.record_irc_imports(&pending_state).await?;
    }

    print_report(&args, &report);
    Ok(())
}

fn print_report(args: &Args, report: &Report) {
    println!("{} log files read", report.files);
    // Everything below describes only the files this run read.
    if report.skipped > 0 {
        println!(
            "{} unchanged since the last import and not opened — the counts below \
             describe the {} file(s) actually read, not the archive. `--all` \
             re-reads everything.",
            report.skipped, report.files
        );
    }
    if report.legacy_paths > 0 {
        println!(
            "{} paths skipped: no network component, so the server is unknown \
             (these predate irssi's $tag in autolog_path)",
            report.legacy_paths
        );
    }
    let total: u64 = report.by_kind.values().sum();
    println!("{total} lines recognised:");
    for (kind, n) in &report.by_kind {
        println!("  {kind:<8} {n}");
    }
    if report.unparsed > 0 {
        println!(
            "⚠ {} lines matched no known shape — a class nobody has written down yet:",
            report.unparsed
        );
        for example in &report.unparsed_examples {
            println!("    {example}");
        }
    }
    if !report.lossy_files.is_empty() {
        println!(
            "⚠ {} file(s) were not valid UTF-8 and were read lossily:",
            report.lossy_files.len()
        );
        for file in report.lossy_files.iter().take(10) {
            println!("    {file}");
        }
    }
    if args.apply {
        println!(
            "wrote {} rows, {} already present",
            report.inserted, report.duplicates
        );
    } else {
        println!("DRY RUN — nothing written. Pass --apply.");
    }
    if args.self_nicks.is_empty() {
        println!(
            "⚠ no --self-nick given: every line is attributed to somebody else, \
             and no conversation is marked as the status log."
        );
    }
}

/// Fails to compile when a `Kind` is added: extend the `irc_messages.kind` ENUM
/// with it, or the import fails at run time.
const _: fn(Kind) = |k| match k {
    Kind::Message | Kind::Action | Kind::Event | Kind::Notice => {}
};
