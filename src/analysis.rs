//! Application boundary for sparse indexing and signal decoding.
use crate::{
    app::{Filter, InputArgs},
    core::*,
    dbc::{Assignment, Decoder},
    index::{self, IndexedReader, Manifest},
    output::{self, AtomicOutput},
    playback::Cancellation,
};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Debug, Clone, clap::Args)]
pub struct StreamOutput {
    /// JSONL/CSV destination; omitted means stdout.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Default JSONL; .csv destinations also select CSV. CSV requires DBC decoding.
    #[arg(long, value_enum)]
    pub format: Option<crate::signal_export::SignalFormat>,
    #[arg(long)]
    pub overwrite: bool,
}

pub struct CacheOptions {
    pub database: PathBuf,
    pub quota: u64,
}

#[derive(Debug, Serialize, Default)]
pub struct AnalysisReport {
    pub schema_version: u32,
    pub status: String,
    pub metadata: Metadata,
    pub source_sha256: Option<String>,
    pub processing: Option<index::Processing>,
    pub index: Option<Manifest>,
    pub frames_examined: u64,
    pub frames_selected: u64,
    pub output_rows: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_schema: Option<crate::signal_export::CsvSchema>,
    pub issues_selected: u64,
    pub issue_counts: BTreeMap<String, u64>,
    pub issue_examples: Vec<Issue>,
    pub omitted_issue_examples: u64,
    pub decode_counts: BTreeMap<String, u64>,
    pub engine_revision: Option<String>,
    pub assignments: Vec<Assignment>,
    pub cache: Option<crate::cache::CacheStats>,
    pub chunks_read: Option<u64>,
    pub selection_complete: bool,
    pub published: bool,
    pub error: Option<String>,
}

pub fn stream(
    args: &InputArgs,
    index_path: Option<&std::path::Path>,
    bindings: Option<&[String]>,
    output_args: &StreamOutput,
    cancel: &Cancellation,
) -> (AnalysisReport, Result<()>) {
    stream_cached(args, index_path, bindings, output_args, cancel, None)
}

pub fn stream_cached(
    args: &InputArgs,
    index_path: Option<&std::path::Path>,
    bindings: Option<&[String]>,
    output_args: &StreamOutput,
    cancel: &Cancellation,
    cache_options: Option<&CacheOptions>,
) -> (AnalysisReport, Result<()>) {
    let mut report = AnalysisReport {
        schema_version: 1,
        status: "running".into(),
        ..Default::default()
    };
    let result = execute(
        args,
        index_path,
        bindings,
        output_args,
        cancel,
        cache_options,
        &mut report,
    );
    report.status = match &result {
        Ok(())
            if report.issues_selected > 0
                || report
                    .decode_counts
                    .iter()
                    .any(|(k, v)| *v > 0 && !matches!(k.as_str(), "decoded" | "remote")) =>
        {
            "partial"
        }
        Ok(()) => "complete",
        Err(e) if e.is::<crate::playback::Cancelled>() => "cancelled",
        Err(_) => "failed",
    }
    .into();
    if let Err(e) = &result {
        report.error = Some(format!("{e:#}"));
    }
    if let Some(path) = &args.report {
        // A report path is never permitted to replace an input, index, DBC or output.
        let write = (|| {
            protect_paths(path, args, index_path, &report.assignments)?;
            if let Some(out) = &output_args.output {
                index::protect(path, out)?;
                output::ensure_distinct_paths(path, out)?;
            }
            output::write_report(path, &report, false)
        })();
        if let Err(e) = write {
            return (report, Err(e));
        }
    }
    (report, result)
}

fn protect_paths(
    path: &std::path::Path,
    args: &InputArgs,
    index: Option<&std::path::Path>,
    assignments: &[Assignment],
) -> Result<()> {
    if args.input != "-" {
        crate::index::protect(path, std::path::Path::new(&args.input))?;
    }
    if let Some(map) = &args.id_map {
        crate::index::protect(path, map)?;
    }
    if let Some(index) = index {
        crate::index::protect(path, index)?;
    }
    for dbc in assignments {
        crate::index::protect(path, &dbc.path)?;
    }
    Ok(())
}

fn execute(
    args: &InputArgs,
    index_path: Option<&std::path::Path>,
    bindings: Option<&[String]>,
    output_args: &StreamOutput,
    cancel: &Cancellation,
    cache_options: Option<&CacheOptions>,
    report: &mut AnalysisReport,
) -> Result<()> {
    let filter = Filter::new(args.clone())?;
    let export_format = crate::signal_export::output_format(
        output_args.output.as_deref(),
        output_args.format.map(Into::into),
    )?;
    ensure!(
        export_format != crate::formats::Format::Csv || bindings.is_some(),
        "signal CSV requires DBC decoding; use export --format csv for raw frames"
    );
    report.export_schema = crate::signal_export::schema(export_format);
    let processing = index::Processing::of(args, cancel)?;
    report.processing = Some(processing.clone());
    ensure!(
        !args.preserve_records,
        "query/decode uses semantic records; --preserve-records is unsupported"
    );
    ensure!(
        !output_args.overwrite || output_args.output.is_some(),
        "--overwrite requires --output"
    );
    let mut decoder = bindings.map(|b| Decoder::load(b, cancel)).transpose()?;
    if let Some(d) = &decoder {
        report.assignments = d.assignments.clone();
        report.engine_revision = Some(crate::dbc::ENGINE_REVISION.into());
    }
    if let Some(path) = &args.report {
        if let Some(cache) = cache_options {
            crate::index::protect(path, &cache.database)?;
            output::ensure_distinct_paths(path, &cache.database)?;
        }
        protect_paths(path, args, index_path, &report.assignments)?;
        ensure!(!path.exists(), "report exists; select a new report path");
    }
    if let Some(path) = &output_args.output {
        if let Some(cache) = cache_options {
            crate::index::protect(path, &cache.database)?;
            output::ensure_distinct_paths(path, &cache.database)?;
        }
        protect_paths(path, args, index_path, &report.assignments)?;
        if let Some(report_path) = &args.report {
            output::ensure_distinct_paths(path, report_path)?;
        }
    }
    let atomic = output_args
        .output
        .as_ref()
        .map(|p| AtomicOutput::new(p, &args.input, output_args.overwrite))
        .transpose()?;
    let mut writer: Box<dyn Write> = if let Some(a) = &atomic {
        Box::new(std::io::BufWriter::new(a.file()?))
    } else {
        Box::new(io::stdout())
    };
    crate::signal_export::write_header(&mut writer, export_format)?;
    let mut indexed = index_path
        .map(|p| IndexedReader::new(p, args, cancel))
        .transpose()?;
    let mut scanned = if indexed.is_none() {
        Some(index::configure_reader(args, cancel)?)
    } else {
        None
    };
    let source_hash = if args.input != "-" {
        Some(index::hash_file(std::path::Path::new(&args.input), cancel)?)
    } else {
        None
    };
    report.source_sha256 = source_hash.clone();
    let mut cache = cache_options
        .map(|options| -> Result<_> {
            ensure!(decoder.is_some(), "signal cache requires DBC decoding");
            let key = crate::cache::semantic_key(
                args,
                source_hash
                    .as_deref()
                    .context("cache requires file input")?,
                &processing,
                &report.assignments,
            )?;
            crate::cache::Session::open(&options.database, key, options.quota)
        })
        .transpose()?;
    if let Some(i) = &indexed {
        report.index = Some(i.manifest.clone());
    }
    let mut limited = false;
    loop {
        cancel.check()?;
        let reader: &mut dyn LogReader = match &mut indexed {
            Some(i) => i,
            None => &mut **scanned.as_mut().expect("scan reader initialized"),
        };
        let item = reader.next_item()?;
        report.metadata = reader.metadata().clone();
        let Some(item) = item else {
            break;
        };
        match item {
            ReadItem::Frame(record) => {
                report.frames_examined += 1;
                if !filter.matches(&record.frame) {
                    continue;
                }
                if args.limit.is_some_and(|n| report.frames_selected >= n) {
                    limited = true;
                    break;
                }
                report.frames_selected += 1;
                if let Some(decoder) = &mut decoder {
                    let ordinal = record.location.ordinal;
                    let hit = cache
                        .as_mut()
                        .map(|c| c.lookup(ordinal))
                        .transpose()?
                        .flatten();
                    let decoded = if let Some(payload) = hit {
                        crate::dbc::DecodedFrame::from_cache(record, payload)
                    } else {
                        let decoded = decoder.decode(record)?;
                        if let Some(cache) = &mut cache {
                            cache.store(ordinal, &decoded.cached())?;
                        }
                        decoded
                    };
                    *report
                        .decode_counts
                        .entry(decoded.status.clone())
                        .or_default() += 1;
                    report.output_rows +=
                        crate::signal_export::write_frame(&mut writer, &decoded, export_format)?;
                } else {
                    crate::formats::jsonl::write_json_record(
                        &mut writer,
                        &record.frame,
                        &record.location,
                    )?;
                    report.output_rows += 1;
                }
            }
            ReadItem::Issue(issue) => {
                if !filter.time_channel(issue.timestamp_ns, issue.channel) {
                    continue;
                }
                report.issues_selected += 1;
                *report
                    .issue_counts
                    .entry(format!("{:?}", issue.kind))
                    .or_default() += 1;
                if report.issue_examples.len() < 100 {
                    report.issue_examples.push(issue.clone());
                } else {
                    report.omitted_issue_examples += 1;
                }
                index::check_issue(args, &issue)?;
            }
        }
    }
    report.selection_complete = !limited;
    if let Some(i) = &indexed {
        i.verify_source()?;
        report.chunks_read = Some(i.chunks_read);
    } else if let Some(hash) = source_hash {
        ensure!(
            index::hash_file(std::path::Path::new(&args.input), cancel)? == hash,
            "source changed during scan"
        );
    }
    if let Some(d) = &decoder {
        d.verify_databases(cancel)?;
    }
    ensure!(
        processing == index::Processing::of(args, cancel)?,
        "ID mapping changed during scan"
    );
    writer.flush()?;
    drop(writer);
    cancel.check()?;
    if let Some(cache) = &mut cache {
        cache.complete(report)?;
        report.cache = Some(cache.stats.clone());
    }
    if let Some(a) = atomic {
        a.publish(true)?;
        report.published = true;
    }
    Ok(())
}
