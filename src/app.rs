use crate::{
    core::*,
    formats::{self, Format},
    output::{self, AtomicOutput},
    playback::{Cancellation, Clock, Control, RealClock, Scheduler},
};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    path::PathBuf,
    sync::mpsc::Receiver,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Unsupported {
    Error,
    Skip,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum IdKind {
    Standard,
    Extended,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Regression {
    Error,
    Immediate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Sink {
    Jsonl,
    Console,
}

#[derive(Debug, Clone, clap::Args)]
pub struct InputArgs {
    pub input: String,
    #[arg(long, value_enum)]
    pub input_format: Option<Format>,
    /// Explicit channel/name assignments for symbolic ASC CAN IDs.
    #[arg(long)]
    pub id_map: Option<PathBuf>,
    /// Preserve bounded native records and CAN annotations in the same ASC/BLF format.
    #[arg(long)]
    pub preserve_records: bool,
    #[arg(long, value_enum, default_value = "error")]
    pub unsupported: Unsupported,
    #[arg(long)]
    pub recover: bool,
    #[arg(long)]
    pub channel: Vec<u16>,
    /// Decimal ID or 0x-prefixed hexadecimal ID/range, e.g. 0x100-0x1ff.
    #[arg(long)]
    pub id: Vec<String>,
    #[arg(long, value_enum)]
    pub id_kind: Option<IdKind>,
    #[arg(long, value_enum)]
    pub direction: Option<Direction>,
    #[arg(long, value_parser = seconds_ns)]
    pub start: Option<i64>,
    #[arg(long, value_parser = seconds_ns)]
    pub end: Option<i64>,
    #[arg(long)]
    pub limit: Option<u64>,
    #[arg(long)]
    pub report: Option<PathBuf>,
    #[arg(long, default_value_t = 65536)]
    pub max_line_bytes: usize,
    #[arg(long, default_value_t = 1048576)]
    pub max_object_bytes: usize,
    #[arg(long, default_value_t = 8388608)]
    pub max_container_bytes: usize,
}
#[derive(Debug, Clone, clap::Args)]
pub struct WriteArgs {
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long, value_enum)]
    pub format: Option<Format>,
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long)]
    pub sync: bool,
    /// Named losses only: unsupported-record, corrupted-region, field:format-metadata,
    /// field:source-metadata, field:direction.
    #[arg(long)]
    pub allow_loss: Vec<String>,
}

#[derive(Debug)]
pub enum Operation {
    Info {
        scan: bool,
    },
    Stats,
    View {
        unlimited: bool,
    },
    Write(WriteArgs),
    Replay {
        write: Option<WriteArgs>,
        speed: f64,
        no_wait: bool,
        repeat: u32,
        gap: i64,
        regression: Regression,
        sink: Sink,
    },
}
impl Operation {
    fn write_args(&self) -> Option<&WriteArgs> {
        match self {
            Self::Write(w) => Some(w),
            Self::Replay { write, .. } => write.as_ref(),
            _ => None,
        }
    }
}

pub struct Filter {
    args: InputArgs,
    ranges: Vec<(u32, u32)>,
}
impl Filter {
    pub fn new(args: InputArgs) -> Result<Self> {
        ensure!(
            args.channel.iter().all(|&c| c != 0),
            "channel must be positive"
        );
        ensure!(
            args.start.unwrap_or(0) <= args.end.unwrap_or(i64::MAX),
            "start must not exceed end"
        );
        ensure!(args.limit != Some(0), "limit must be positive");
        ensure!(
            (256..=1048576).contains(&args.max_line_bytes),
            "max-line-bytes must be 256..1048576"
        );
        ensure!(
            (128..=16777216).contains(&args.max_object_bytes),
            "max-object-bytes must be 128..16777216"
        );
        ensure!(
            (128..=67108864).contains(&args.max_container_bytes),
            "max-container-bytes must be 128..67108864"
        );
        let mut ranges = Vec::new();
        for range in &args.id {
            let (a, b) = range.split_once('-').unwrap_or((range, range));
            let (a, _) = parse_id(a, 10)?;
            let (b, _) = parse_id(b, 10)?;
            ensure!(a <= b && b <= 0x1fff_ffff, "invalid ID range");
            ranges.push((a, b));
        }
        Ok(Self { args, ranges })
    }
    pub fn matches(&self, f: &Frame) -> bool {
        self.time_channel(Some(f.timestamp_ns()), Some(f.channel()))
            && (self.ranges.is_empty()
                || self.ranges.iter().any(|&(a, b)| (a..=b).contains(&f.id())))
            && self
                .args
                .id_kind
                .is_none_or(|kind| f.extended() == (kind == IdKind::Extended))
            && self.args.direction.is_none_or(|d| f.direction() == d)
    }
    pub(crate) fn time_channel(&self, timestamp: Option<i64>, channel: Option<u16>) -> bool {
        // Unknown coordinates cannot prove that an issue is outside selection.
        timestamp.is_none_or(|t| {
            t >= self.args.start.unwrap_or(0) && t < self.args.end.unwrap_or(i64::MAX)
        }) && channel.is_none_or(|c| self.args.channel.is_empty() || self.args.channel.contains(&c))
    }
    pub(crate) fn limits(&self) -> Limits {
        Limits {
            max_line: self.args.max_line_bytes,
            max_object: self.args.max_object_bytes,
            max_container: self.args.max_container_bytes,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub status: String,
    pub metadata: Metadata,
    pub frames_read: u64,
    pub frames_selected: u64,
    pub frames_written: u64,
    pub issues: u64,
    pub issues_outside_selection: u64,
    pub issues_preserved: u64,
    pub native_records_written: u64,
    pub preserved_counts: BTreeMap<String, u64>,
    pub id_map: Option<crate::id_map::Document>,
    pub mapped_frames: u64,
    pub issue_counts: BTreeMap<String, u64>,
    pub issue_examples: Vec<Issue>,
    pub omitted_issue_examples: u64,
    pub losses: BTreeMap<String, u64>,
    pub statistics: BTreeMap<String, u64>,
    pub first_timestamp_ns: Option<i64>,
    pub last_timestamp_ns: Option<i64>,
    pub regressions: u64,
    pub output_first_timestamp_ns: Option<i64>,
    pub output_last_timestamp_ns: Option<i64>,
    pub replay_timeline: Option<String>,
    pub scan_complete: bool,
    pub finalized: bool,
    pub durable: bool,
    pub published: bool,
    pub elapsed_ms: u128,
    pub error: Option<String>,
}
fn increment(map: &mut BTreeMap<String, u64>, key: String) -> Result<()> {
    ensure!(
        map.contains_key(&key) || map.len() < 4096,
        "statistics key resource limit exceeded (4096)"
    );
    *map.entry(key).or_default() += 1;
    Ok(())
}
fn loss(report: &mut Report, allowed: &BTreeSet<String>, category: &str) -> Result<()> {
    increment(&mut report.losses, category.into())?;
    ensure!(
        allowed.contains(category),
        "loss rejected: {category}; explicitly use --allow-loss {category}"
    );
    Ok(())
}
pub fn validate(args: &InputArgs, op: &Operation) -> Result<()> {
    ensure!(args.input_format != Some(Format::Csv), "CSV is export-only");
    ensure!(
        args.input != "-" || args.input_format == Some(Format::Jsonl),
        "stdin requires --input-format jsonl"
    );
    Filter::new(args.clone())?;
    if let Some(path) = &args.id_map {
        ensure!(args.input != "-", "ID mapping requires ASC file input");
        crate::id_map::IdMap::load(path)?;
        if let Some(w) = op.write_args() {
            ensure!(
                !w.output.exists() || !same_file::is_same_file(path, &w.output)?,
                "output cannot replace ID map input"
            );
        }
    }
    if args.preserve_records {
        let writing = op
            .write_args()
            .context("preserve-records requires file output")?;
        ensure!(
            matches!(
                formats::output_format(&writing.output, writing.format)?,
                Format::Asc | Format::Blf
            ),
            "preserve-records requires ASC or BLF output"
        );
        ensure!(
            args.id_map.is_none(),
            "preserve-records and ID mapping cannot be combined"
        );
        ensure!(args.id.is_empty() && args.id_kind.is_none() && args.direction.is_none(), "preserve-records supports time/channel filters only; ID/direction cannot select opaque events");
        if let Operation::Replay { repeat, .. } = op {
            ensure!(
                *repeat == 1,
                "preserve-records cannot repeat opaque records with unknown timestamps"
            );
        }
    }
    if let Operation::Replay {
        speed, repeat, gap, ..
    } = op
    {
        ensure!(
            speed.is_finite() && *speed > 0.0 && *repeat > 0 && *gap >= 0,
            "replay requires positive finite speed/repeat and nonnegative gap"
        );
        ensure!(
            args.input != "-" || *repeat == 1,
            "stdin cannot be reopened for repeat"
        );
    }
    if let Some(w) = op.write_args() {
        formats::output_format(&w.output, w.format)?;
        for category in &w.allow_loss {
            ensure!(
                matches!(
                    category.as_str(),
                    "unsupported-record"
                        | "corrupted-region"
                        | "field:format-metadata"
                        | "field:source-metadata"
                        | "field:direction"
                ),
                "unknown loss category: {category}"
            );
        }
        if w.output.exists() && args.input != "-" {
            ensure!(
                !same_file::is_same_file(&args.input, &w.output)?,
                "input and output refer to the same file"
            );
        }
        ensure!(
            w.overwrite || !w.output.exists(),
            "output exists; use --overwrite"
        );
    }
    if let Some(path) = &args.report {
        ensure!(
            !path.exists(),
            "report already exists; choose a new report path"
        );
        let absolute = |p: &std::path::Path| -> Result<PathBuf> {
            let parent = p
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."));
            Ok(parent
                .canonicalize()?
                .join(p.file_name().context("file path has no filename")?))
        };
        if let Some(w) = op.write_args() {
            ensure!(
                absolute(path)? != absolute(&w.output)?,
                "report and output must be distinct paths"
            );
        }
        if args.input != "-" {
            ensure!(
                absolute(path)? != std::path::Path::new(&args.input).canonicalize()?,
                "report cannot replace input"
            );
        }
    }
    Ok(())
}

pub fn run(
    args: InputArgs,
    op: Operation,
    cancel: Cancellation,
    controls: Option<Receiver<Control>>,
) -> (Report, Result<()>) {
    let mut report = Report {
        schema_version: 1,
        status: "running".into(),
        ..Default::default()
    };
    let clock = RealClock::default();
    let result = execute(args.clone(), &op, &cancel, controls.as_ref(), &mut report);
    report.elapsed_ms = clock.now().as_millis();
    report.status = match &result {
        Ok(())
            if report.issues > report.issues_outside_selection + report.issues_preserved
                || !report.losses.is_empty() =>
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
    if result.is_ok() && matches!(op, Operation::Info { scan: true } | Operation::Stats) {
        if let Err(e) = (|| -> Result<()> {
            let mut out = io::stdout().lock();
            serde_json::to_writer_pretty(&mut out, &report)?;
            writeln!(out)?;
            Ok(())
        })() {
            return (report, Err(e));
        }
    }
    if let Some(path) = &args.report {
        if let Err(e) = output::write_report(path, &report, false) {
            return (report, Err(e.context("writing final report")));
        }
    }
    (report, result)
}

fn execute(
    args: InputArgs,
    op: &Operation,
    cancel: &Cancellation,
    controls: Option<&Receiver<Control>>,
    report: &mut Report,
) -> Result<()> {
    let filter = Filter::new(args.clone())?;
    let writing = op.write_args();
    let allowed: BTreeSet<_> = writing
        .map(|w| w.allow_loss.iter().cloned().collect())
        .unwrap_or_default();
    let output_format = writing
        .map(|w| formats::output_format(&w.output, w.format))
        .transpose()?;
    let id_map = args
        .id_map
        .as_ref()
        .map(|path| crate::id_map::IdMap::load(path).map(std::sync::Arc::new))
        .transpose()?;
    report.id_map = id_map.as_ref().map(|map| map.document.clone());
    let mut reader = formats::open_cancellable_reader(
        &args.input,
        args.input_format,
        filter.limits(),
        cancel.clone(),
    )?;
    reader.configure(args.preserve_records, id_map.clone())?;
    let mut pending = reader.next_item()?;
    report.metadata = reader.metadata().clone();
    if args.preserve_records {
        ensure!(
            matches!(
                (reader.metadata().format.as_str(), output_format),
                ("asc", Some(Format::Asc)) | ("blf", Some(Format::Blf))
            ),
            "native records require same-format output; cross-format preservation is unsupported"
        );
    }
    if matches!(op, Operation::Info { scan: false }) {
        report.status = "header-only".into();
        serde_json::to_writer_pretty(io::stdout().lock(), &report.metadata)?;
        println!();
        return Ok(());
    }
    let atomic = writing
        .map(|w| AtomicOutput::new(&w.output, &args.input, w.overwrite))
        .transpose()?;
    let mut writer = atomic
        .as_ref()
        .map(|o| {
            formats::make_writer_with_preservation(
                o.file()?,
                output_format.unwrap(),
                reader.metadata(),
                args.preserve_records,
            )
        })
        .transpose()?;
    let (repeat, gap) = if let Operation::Replay { repeat, gap, .. } = op {
        (*repeat, *gap)
    } else {
        (1, 0)
    };
    let mut scheduler = if let Operation::Replay {
        speed,
        no_wait,
        regression,
        ..
    } = op
    {
        Some(Scheduler::new(
            RealClock::default(),
            *speed,
            *no_wait,
            *regression == Regression::Immediate,
        )?)
    } else {
        None
    };
    let source_revision = if args.input != "-" {
        Some(std::fs::metadata(&args.input)?)
    } else {
        None
    };
    let mut cycle_offset = 0_i64;
    if matches!(op, Operation::Replay { .. }) {
        report.replay_timeline = Some(
            "speed changes waiting only; repeat offsets use selected span + repeat-gap".into(),
        );
    }
    let mut previous = None;
    let mut stdout = io::stdout().lock();
    let limit = if let Operation::View { unlimited: false } = op {
        args.limit.or(Some(100))
    } else {
        args.limit
    };
    let mut stopped_by_limit = false;
    for cycle in 0..repeat {
        let mut mapping_seen = 0;
        let mut first_cycle = None;
        let mut last_cycle = None;
        loop {
            cancel.check()?;
            let item = match pending.take() {
                Some(item) => Some(item),
                None => reader.next_item()?,
            };
            let Some(item) = item else {
                break;
            };
            let mapped = reader.metadata().mapped_frames;
            report.mapped_frames += mapped - mapping_seen;
            mapping_seen = mapped;
            match item {
                ReadItem::Issue(mut issue) => {
                    let native = issue.native.take();
                    report.issues += 1;
                    increment(&mut report.issue_counts, format!("{:?}", issue.kind))?;
                    if report.issue_examples.len() < 100 {
                        report.issue_examples.push(issue.clone());
                    } else {
                        report.omitted_issue_examples += 1;
                    }
                    if !filter.time_channel(issue.timestamp_ns, issue.channel) {
                        report.issues_outside_selection += 1;
                        continue;
                    }
                    if args.preserve_records {
                        if let Some(native) = &native {
                            if let Some(t) = issue.timestamp_ns {
                                if previous.is_some_and(|p| t < p) {
                                    report.regressions += 1;
                                }
                                previous = Some(t);
                            }
                            if let Some(scheduler) = &mut scheduler {
                                scheduler
                                    .wait_optional(issue.timestamp_ns, cancel, controls)
                                    .with_context(|| {
                                        format!("replay native record at {:?}", issue.location)
                                    })?;
                            }
                            writer
                                .as_mut()
                                .context("native records require writer")?
                                .write_native(native)?;
                            report.native_records_written += 1;
                            report.issues_preserved += 1;
                            increment(&mut report.preserved_counts, format!("{:?}", issue.kind))?;
                            if let Some(t) = issue.timestamp_ns {
                                report.output_first_timestamp_ns.get_or_insert(t);
                                report.output_last_timestamp_ns = Some(t);
                            }
                            continue;
                        }
                    }
                    let corrupted = issue.kind == IssueKind::CorruptedRegion;
                    if corrupted && !args.recover {
                        bail!("parse issue at {:?}: {}", issue.location, issue.message);
                    }
                    if !corrupted && args.unsupported == Unsupported::Error {
                        bail!(
                            "unsupported record at {:?}: {}",
                            issue.location,
                            issue.message
                        );
                    }
                    if writer.is_some() {
                        loss(
                            report,
                            &allowed,
                            if corrupted {
                                "corrupted-region"
                            } else {
                                "unsupported-record"
                            },
                        )?;
                    }
                }
                ReadItem::Frame(record) => {
                    let f = &record.frame;
                    report.frames_read += 1;
                    if !filter.matches(f) {
                        continue;
                    }
                    report.frames_selected += 1;
                    let output_time = f
                        .timestamp_ns()
                        .checked_add(cycle_offset)
                        .context("repeat timestamp overflow")?;
                    if previous.is_some_and(|p| output_time < p) {
                        report.regressions += 1;
                    }
                    previous = Some(output_time);
                    report.first_timestamp_ns = Some(
                        report
                            .first_timestamp_ns
                            .map_or(f.timestamp_ns(), |t| t.min(f.timestamp_ns())),
                    );
                    report.last_timestamp_ns = Some(
                        report
                            .last_timestamp_ns
                            .map_or(f.timestamp_ns(), |t| t.max(f.timestamp_ns())),
                    );
                    if matches!(op, Operation::Stats | Operation::Info { scan: true }) {
                        increment(
                            &mut report.statistics,
                            format!(
                                "ch{}:{}:{:X}:{:?}:{}:{}",
                                f.channel(),
                                if f.extended() { "extended" } else { "standard" },
                                f.id(),
                                f.direction(),
                                if f.fd() { "fd" } else { "classic" },
                                if f.remote() { "remote" } else { "data" }
                            ),
                        )?;
                    }
                    first_cycle.get_or_insert(f.timestamp_ns());
                    last_cycle =
                        Some(last_cycle.map_or(f.timestamp_ns(), |t: i64| t.max(f.timestamp_ns())));
                    let mut output_frame = f.with_timestamp(output_time)?;
                    report.output_first_timestamp_ns.get_or_insert(output_time);
                    report.output_last_timestamp_ns = Some(output_time);
                    if writer.is_some()
                        && !args.preserve_records
                        && matches!(output_format, Some(Format::Asc | Format::Blf))
                        && output_frame.direction() == Direction::Unknown
                    {
                        loss(report, &allowed, "field:direction")?;
                        output_frame = output_frame.with_direction(Direction::Rx)?;
                    }
                    if let Some(scheduler) = &mut scheduler {
                        scheduler
                            .wait(output_frame.timestamp_ns(), cancel, controls)
                            .with_context(|| format!("replay at {:?}", record.location))?;
                    }
                    if let Some(writer) = &mut writer {
                        if args.preserve_records {
                            writer.write_native(
                                record
                                    .native
                                    .as_ref()
                                    .context("frame has no native representation")?,
                            )?;
                            report.native_records_written += 1;
                        } else {
                            writer.write_frame(&output_frame)?;
                        }
                        report.frames_written += 1;
                    } else if matches!(op, Operation::Replay { .. } | Operation::View { .. }) {
                        if matches!(
                            op,
                            Operation::Replay {
                                sink: Sink::Console,
                                ..
                            }
                        ) {
                            writeln!(
                                stdout,
                                "{} ch{} {:X}{} {:?} {}",
                                time_text(output_frame.timestamp_ns()),
                                output_frame.channel(),
                                output_frame.id(),
                                if output_frame.extended() { "x" } else { "" },
                                output_frame.direction(),
                                output_frame
                                    .data()
                                    .iter()
                                    .map(|b| format!("{b:02X}"))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            )?;
                        } else {
                            formats::jsonl::write_json_record(
                                &mut stdout,
                                &output_frame,
                                &record.location,
                            )?;
                        }
                        stdout.flush()?;
                    }
                    if limit.is_some_and(|n| report.frames_selected >= n) {
                        stopped_by_limit = true;
                        break;
                    }
                }
            }
        }
        report.metadata = reader.metadata().clone();
        if stopped_by_limit || first_cycle.is_none() || cycle + 1 == repeat {
            break;
        }
        let span = last_cycle
            .unwrap()
            .checked_sub(first_cycle.unwrap())
            .context("repeat time overflow")?;
        ensure!(span >= 0, "repeat requires nonnegative cycle span");
        cycle_offset = cycle_offset
            .checked_add(span)
            .and_then(|n| n.checked_add(gap))
            .context("repeat offset overflow")?;
        if let Some(old) = &source_revision {
            let now = std::fs::metadata(&args.input)?;
            ensure!(
                now.len() == old.len() && now.modified()? == old.modified()?,
                "source changed during replay"
            );
        }
        reader = formats::open_reader(&args.input, args.input_format, filter.limits())?;
        reader.configure(args.preserve_records, id_map.clone())?;
        pending = reader.next_item()?;
    }
    report.scan_complete = !stopped_by_limit;
    if let Some(ref mut finalized_writer) = writer {
        if report
            .metadata
            .notes
            .iter()
            .any(|s| s == "input frame provenance")
        {
            loss(report, &allowed, "field:source-metadata")?;
        }
        if !args.preserve_records
            && report
                .metadata
                .notes
                .iter()
                .any(|s| s == "frame annotations")
        {
            loss(report, &allowed, "field:format-metadata")?;
        }
        if matches!(output_format, Some(Format::Jsonl | Format::Csv))
            && report.metadata.date_text.is_some()
        {
            loss(report, &allowed, "field:source-metadata")?;
        }
        if output_format == Some(Format::Blf)
            && report.metadata.date_text.is_some()
            && (report.metadata.blf_start.is_none()
                || report
                    .metadata
                    .notes
                    .iter()
                    .any(|s| s == "submillisecond date origin"))
        {
            loss(report, &allowed, "field:source-metadata")?;
        }
        cancel.check()?;
        finalized_writer.finish()?;
        report.finalized = true;
        drop(writer);
        let w = writing.unwrap();
        atomic.unwrap().publish(w.sync)?;
        report.published = true;
        report.durable = w.sync;
    }
    Ok(())
}
