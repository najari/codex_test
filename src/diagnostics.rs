//! File publication and source identity boundary for passive diagnostic analysis.
use crate::{
    app::InputArgs,
    core::*,
    index,
    isotp::{Config, Event, Reassembler},
    output::{self, AtomicOutput},
    playback::Cancellation,
};
use anyhow::{ensure, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, clap::Args)]
pub struct IsotpOutput {
    /// Diagnostic JSONL destination; omitted means stdout.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long)]
    pub overwrite: bool,
}
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub analyzer: String,
    pub status: String,
    pub source_sha256: Option<String>,
    pub routes_sha256: Option<String>,
    pub timeline_key: Option<String>,
    pub processing: Option<index::Processing>,
    pub metadata: Metadata,
    pub config: Option<Config>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uds_policy: Option<crate::uds::Config>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uds_policy_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uds_counts: Option<BTreeMap<String, u64>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cdd_assignments: Vec<crate::cdd::Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdd_counts: Option<BTreeMap<String, u64>>,
    #[serde(skip)]
    cdd_decoder: Option<crate::cdd::Decoder>,
    #[serde(skip)]
    watermark_ns: i64,
    pub frames_examined: u64,
    pub frames_matched: u64,
    pub output_rows: u64,
    pub counts: BTreeMap<String, u64>,
    pub input_issues: u64,
    pub issue_examples: Vec<Issue>,
    pub omitted_issue_examples: u64,
    pub scan_complete: bool,
    pub published: bool,
    pub error: Option<String>,
}
#[derive(Serialize)]
struct Row<'a> {
    schema_version: u32,
    analyzer: &'static str,
    timeline_key: &'a str,
    result_key: String,
    #[serde(flatten)]
    event: &'a Event,
}
const ANALYZER: &str = "canlog-isotp-classic-v1";
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn protect(path: &Path, args: &InputArgs, routes: &Path) -> Result<()> {
    output::ensure_distinct_paths(path, Path::new(&args.input))?;
    output::ensure_distinct_paths(path, routes)?;
    if let Some(map) = &args.id_map {
        output::ensure_distinct_paths(path, map)?;
    }
    Ok(())
}
fn protect_cdd(path: &Path, policy: Option<&crate::uds::Config>) -> Result<()> {
    if let Some(policy) = policy {
        for cdd in policy.routes.iter().filter_map(|route| route.cdd.as_ref()) {
            output::ensure_distinct_paths(path, &cdd.path)?;
        }
    }
    Ok(())
}
fn emit_uds(
    writer: &mut dyn Write,
    observations: Vec<crate::uds::Observation>,
    report: &mut Report,
) -> Result<()> {
    for mut observation in observations {
        observation.observed_at_timestamp_ns = observation
            .observed_at_timestamp_ns
            .max(report.watermark_ns);
        if let Some(decoder) = &report.cdd_decoder {
            decoder.decorate(&mut observation);
        }
        if let Some(cdd) = &observation.cdd {
            *report
                .cdd_counts
                .as_mut()
                .unwrap()
                .entry(cdd.status.clone())
                .or_default() += 1;
        }
        let status = serde_json::to_value(observation.status)?
            .as_str()
            .unwrap()
            .to_owned();
        *report
            .uds_counts
            .as_mut()
            .unwrap()
            .entry(status)
            .or_default() += 1;
        let timeline_key = report.timeline_key.as_deref().unwrap();
        let mut identity = timeline_key.as_bytes().to_vec();
        identity.extend_from_slice(&serde_json::to_vec(&observation)?);
        #[derive(Serialize)]
        struct UdsRow<'a> {
            schema_version: u32,
            analyzer: &'a str,
            timeline_key: &'a str,
            result_key: String,
            #[serde(flatten)]
            observation: &'a crate::uds::Observation,
        }
        serde_json::to_writer(
            &mut *writer,
            &UdsRow {
                schema_version: 1,
                analyzer: &report.analyzer,
                timeline_key,
                result_key: digest(&identity),
                observation: &observation,
            },
        )?;
        writeln!(writer)?;
        report.output_rows += 1;
    }
    Ok(())
}
fn emit(
    writer: &mut dyn Write,
    events: Vec<Event>,
    report: &mut Report,
    matcher: &mut Option<crate::uds::Matcher>,
) -> Result<()> {
    for event in events {
        let observations = matcher
            .as_mut()
            .map(|matcher| matcher.consume(&event))
            .transpose()?
            .unwrap_or_default();
        let key = format!("{}:{}", event.kind, event.status);
        *report.counts.entry(key).or_default() += 1;
        if let Some(reason) = &event.reason {
            *report.counts.entry(format!("reason:{reason}")).or_default() += 1;
        }
        if event.kind == "payload" && event.fc_observation == "missing" {
            *report
                .counts
                .entry("fc_missing_payloads".into())
                .or_default() += 1;
        }
        if event.kind == "payload" {
            for (name, count) in [
                ("observed_reserved_stmin", event.flow_control.reserved_stmin),
                (
                    "observed_shorter_than_stmin",
                    event.flow_control.shorter_than_stmin,
                ),
                (
                    "observed_block_size_exceeded",
                    event.flow_control.block_size_exceeded,
                ),
                ("observed_cf_after_wait", event.flow_control.cf_after_wait),
            ] {
                if count > 0 {
                    *report.counts.entry(name.into()).or_default() += count;
                }
            }
        }
        let timeline_key = report.timeline_key.as_deref().unwrap();
        let mut identity = timeline_key.as_bytes().to_vec();
        identity.extend_from_slice(&serde_json::to_vec(&event)?);
        let row = Row {
            schema_version: 1,
            analyzer: ANALYZER,
            timeline_key,
            result_key: digest(&identity),
            event: &event,
        };
        serde_json::to_writer(&mut *writer, &row)?;
        writeln!(writer)?;
        report.output_rows += 1;
        emit_uds(writer, observations, report)?;
    }
    Ok(())
}
pub fn analyze(
    args: &InputArgs,
    routes: &Path,
    out: &IsotpOutput,
    cancel: &Cancellation,
) -> (Report, Result<()>) {
    analyze_inner(args, routes, None, out, cancel)
}
pub fn analyze_uds(
    args: &InputArgs,
    routes: &Path,
    policy: &Path,
    out: &IsotpOutput,
    cancel: &Cancellation,
) -> (Report, Result<()>) {
    analyze_inner(args, routes, Some(policy), out, cancel)
}
fn analyze_inner(
    args: &InputArgs,
    routes: &Path,
    policy: Option<&Path>,
    out: &IsotpOutput,
    cancel: &Cancellation,
) -> (Report, Result<()>) {
    let mut report = Report {
        schema_version: 1,
        analyzer: if policy.is_some() {
            "canlog-uds2013-physical-v1"
        } else {
            ANALYZER
        }
        .into(),
        status: "running".into(),
        ..Default::default()
    };
    let result = execute(args, routes, policy, out, cancel, &mut report);
    report.status = match &result {
        Err(e) if e.is::<crate::playback::Cancelled>() => "cancelled",
        Err(_) => "failed",
        Ok(())
            if report
                .cdd_assignments
                .iter()
                .any(|a| a.load_error_count > 0)
                || report.cdd_counts.as_ref().is_some_and(|counts| {
                    counts.iter().any(|(status, count)| {
                        *count > 0 && !matches!(status.as_str(), "decoded" | "not_decoded")
                    })
                }) =>
        {
            "partial"
        }
        Ok(())
            if report.uds_counts.as_ref().is_some_and(|counts| {
                counts.iter().any(|(status, count)| {
                    *count > 0
                        && !matches!(
                            status.as_str(),
                            "positive" | "negative" | "pending" | "suppressed_expected"
                        )
                })
            }) =>
        {
            "partial"
        }
        Ok(())
            if report.input_issues > 0
                || report.counts.iter().any(|(key, count)| {
                    *count > 0
                        && (key == "protocol_issue:protocol_violation"
                            || key == "payload:incomplete"
                            || key == "payload:aborted"
                            || key == "reason:orphan_flow_control")
                }) =>
        {
            "partial"
        }
        Ok(()) => "complete",
    }
    .into();
    if let Err(e) = &result {
        report.error = Some(format!("{e:#}"));
    }
    if let Some(path) = &args.report {
        let write = (|| {
            protect(path, args, routes)?;
            protect_cdd(path, report.uds_policy.as_ref())?;
            if let Some(policy) = policy {
                output::ensure_distinct_paths(path, policy)?;
            }
            if let Some(dest) = &out.output {
                output::ensure_distinct_paths(path, dest)?;
            }
            output::write_report(path, &report, false)
        })();
        if let Err(error) = write {
            return (report, Err(error));
        }
    }
    (report, result)
}
fn execute(
    args: &InputArgs,
    routes: &Path,
    policy: Option<&Path>,
    out: &IsotpOutput,
    cancel: &Cancellation,
    report: &mut Report,
) -> Result<()> {
    ensure!(
        args.input != "-" && !args.preserve_records,
        "ISO-TP requires a file and semantic parsing"
    );
    ensure!(
        args.start.is_none()
            && args.end.is_none()
            && args.channel.is_empty()
            && args.id.is_empty()
            && args.id_kind.is_none()
            && args.direction.is_none()
            && args.limit.is_none(),
        "ISO-TP requires a full scan without filters; select channels and endpoint IDs in --routes"
    );
    ensure!(
        !out.overwrite || out.output.is_some(),
        "--overwrite requires --output"
    );
    if let Some(path) = &out.output {
        protect(path, args, routes)?;
        if let Some(policy) = policy {
            output::ensure_distinct_paths(path, policy)?;
        }
    }
    if let Some(path) = &args.report {
        protect(path, args, routes)?;
        if let Some(policy) = policy {
            output::ensure_distinct_paths(path, policy)?;
        }
        ensure!(!path.exists(), "report already exists");
        if let Some(dest) = &out.output {
            output::ensure_distinct_paths(path, dest)?;
        }
    }
    cancel.check()?;
    let mut bytes = vec![];
    File::open(routes)?.take(262_145).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 262_144,
        "route configuration exceeds 256 KiB"
    );
    let config: Config = serde_json::from_slice(&bytes)?;
    let mut reassembler = Reassembler::new(config.clone())?;
    if let Some(path) = policy {
        let mut bytes = vec![];
        File::open(path)?.take(262_145).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 262_144, "UDS policy exceeds 256 KiB");
        let mut uds: crate::uds::Config = serde_json::from_slice(&bytes)?;
        for binding in &mut uds.routes {
            if let Some(cdd) = &mut binding.cdd {
                if !cdd.path.is_absolute() {
                    cdd.path = path.parent().unwrap_or(Path::new(".")).join(&cdd.path);
                }
            }
        }
        report.uds_policy = Some(uds);
        report.uds_policy.as_ref().unwrap().validate(&config)?;
        for dest in [&out.output, &args.report].into_iter().flatten() {
            protect_cdd(dest, report.uds_policy.as_ref())?;
        }
        let decoder = crate::cdd::Decoder::load(report.uds_policy.as_ref().unwrap(), cancel)?;
        report.cdd_assignments = decoder.summaries.clone();
        if !report.cdd_assignments.is_empty() {
            report.cdd_counts = Some(BTreeMap::new());
        }
        report.cdd_decoder = Some(decoder);
        report.uds_policy_sha256 = Some(digest(&bytes));
        report.uds_counts = Some(BTreeMap::new());
    }
    let routes_hash = digest(&bytes);
    let source_hash = index::hash_file(Path::new(&args.input), cancel)?;
    let processing = index::Processing::of(args, cancel)?;
    let mut reader = index::configure_reader(args, cancel)?;
    report.metadata = reader.metadata().clone();
    report.processing = Some(processing.clone());
    report.source_sha256 = Some(source_hash.clone());
    report.routes_sha256 = Some(routes_hash.clone());
    report.config = Some(config);
    // No UTC inference or multi-source clock mapping. Equal timestamps retain source order.
    report.timeline_key = Some(digest(&serde_json::to_vec(&(
        ANALYZER,
        &source_hash,
        &routes_hash,
        &processing,
        &report.metadata,
        "single-source-file-order",
    ))?));
    if policy.is_some() {
        report.timeline_key = Some(digest(&serde_json::to_vec(&(
            &report.analyzer,
            &report.timeline_key,
            &report.uds_policy_sha256,
            format!("{:?}", args.unsupported),
            args.recover,
        ))?));
    }
    if !report.cdd_assignments.is_empty() {
        report.timeline_key = Some(digest(&serde_json::to_vec(&(
            &report.timeline_key,
            &report.cdd_assignments,
        ))?));
    }
    let mut matcher = report
        .uds_policy
        .clone()
        .map(|policy| {
            crate::uds::Matcher::new(
                policy,
                report.config.as_ref().unwrap(),
                report.timeline_key.as_deref().unwrap(),
            )
        })
        .transpose()?;
    let atomic = out
        .output
        .as_ref()
        .map(|path| AtomicOutput::new(path, &args.input, out.overwrite))
        .transpose()?;
    let mut writer: Box<dyn Write> = if let Some(file) = &atomic {
        Box::new(BufWriter::new(file.file()?))
    } else {
        Box::new(BufWriter::new(io::stdout()))
    };
    let mut watermark = 0;
    while let Some(item) = reader.next_item()? {
        cancel.check()?;
        match item {
            ReadItem::Frame(record) => {
                watermark = record.frame.timestamp_ns();
                report.watermark_ns = watermark;
                report.frames_examined += 1;
                let result = reassembler.feed(&record);
                report.frames_matched = reassembler.matched_frames;
                let events = result?;
                if !events.iter().any(|e| {
                    e.kind == "protocol_issue" || (e.kind == "payload" && e.status != "complete")
                }) {
                    if let Some(matcher) = &mut matcher {
                        let mut starts = reassembler.receiving_responses();
                        starts.extend(
                            events
                                .iter()
                                .filter(|e| {
                                    e.kind == "payload"
                                        && e.direction == Some(crate::isotp::Direction::Response)
                                })
                                .map(|e| crate::isotp::ResponseStart {
                                    route: e.route.clone().unwrap(),
                                    first_timestamp_ns: e.first_timestamp_ns.unwrap(),
                                    first_ordinal: e.first_location.ordinal,
                                }),
                        );
                        emit_uds(&mut *writer, matcher.advance(watermark, &starts), report)?;
                    }
                }
                emit(&mut *writer, events, report, &mut matcher)?;
                if let Some(matcher) = &mut matcher {
                    emit_uds(
                        &mut *writer,
                        matcher.advance(watermark, &reassembler.receiving_responses()),
                        report,
                    )?;
                }
            }
            ReadItem::Issue(issue) => {
                report.input_issues += 1;
                if report.issue_examples.len() < 100 {
                    report.issue_examples.push(issue.clone());
                } else {
                    report.omitted_issue_examples += 1;
                }
                index::check_issue(args, &issue)?;
                if let Some(time) = issue.timestamp_ns {
                    watermark = time;
                    report.watermark_ns = watermark;
                    emit(
                        &mut *writer,
                        reassembler.advance(time, &issue.location)?,
                        report,
                        &mut matcher,
                    )?;
                }
                emit(
                    &mut *writer,
                    reassembler.gap(issue.channel),
                    report,
                    &mut matcher,
                )?;
                if let Some(matcher) = &mut matcher {
                    emit_uds(&mut *writer, matcher.gap(issue.channel, watermark), report)?;
                }
                // The issue itself remains visible even without a report or active session.
                serde_json::to_writer(
                    &mut *writer,
                    &serde_json::json!({"schema_version":1,"analyzer":ANALYZER,"timeline_key":report.timeline_key,"result_key":digest(&serde_json::to_vec(&(&report.timeline_key, &issue))?),"kind":"capture_gap","issue":issue}),
                )?;
                writeln!(writer)?;
                report.output_rows += 1;
                *report.counts.entry("capture_gap".into()).or_default() += 1;
            }
        }
    }
    emit(&mut *writer, reassembler.eof(), report, &mut matcher)?;
    if let Some(matcher) = &mut matcher {
        emit_uds(&mut *writer, matcher.eof(watermark), report)?;
    }
    writer.flush()?;
    drop(writer);
    cancel.check()?;
    ensure!(
        index::hash_file(Path::new(&args.input), cancel)? == source_hash,
        "source changed during ISO-TP analysis"
    );
    if let Some(policy) = policy {
        ensure!(
            Some(index::hash_file(policy, cancel)?) == report.uds_policy_sha256,
            "UDS policy changed during analysis"
        );
    }
    ensure!(
        index::hash_file(routes, cancel)? == routes_hash
            && index::Processing::of(args, cancel)? == processing,
        "route configuration or ID map changed during ISO-TP analysis"
    );
    for assignment in &report.cdd_assignments {
        ensure!(
            index::hash_file(&assignment.path, cancel)? == assignment.sha256,
            "CDD changed during analysis"
        );
    }
    report.scan_complete = true;
    if let Some(atomic) = atomic {
        atomic.publish(true)?;
        report.published = true;
    }
    Ok(())
}
