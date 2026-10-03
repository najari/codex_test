use canlog::{
    app::*,
    playback::{Cancellation, Cancelled, Control},
};
use clap::{Parser, Subcommand};
use std::{
    io::{self, BufRead},
    path::PathBuf,
    sync::mpsc,
};

#[derive(Parser)]
#[command(
    version,
    about = "Streaming ASC/BLF CAN log recorder and timed playback engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Manage independent logs, stored DBC bindings and signal caches.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Build or inspect sparse ASC/BLF search indexes.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Decode signals using explicit channel-to-DBC assignments.
    Decode {
        #[command(flatten)]
        args: InputArgs,
        /// Repeat for multiple channels/databases: --dbc 1=network.dbc
        #[arg(long, required = true)]
        dbc: Vec<String>,
        #[arg(long)]
        index: Option<PathBuf>,
        #[command(flatten)]
        output: canlog::analysis::StreamOutput,
    },
    Info {
        #[command(flatten)]
        args: InputArgs,
        #[arg(long)]
        scan: bool,
    },
    Stats {
        #[command(flatten)]
        args: InputArgs,
    },
    View {
        #[command(flatten)]
        args: InputArgs,
        #[arg(long, conflicts_with = "limit")]
        unlimited: bool,
    },
    Convert {
        #[command(flatten)]
        args: InputArgs,
        output: PathBuf,
        #[arg(long, value_enum)]
        format: Option<canlog::formats::Format>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        sync: bool,
        #[arg(long)]
        allow_loss: Vec<String>,
    },
    Filter {
        #[command(flatten)]
        args: InputArgs,
        #[command(flatten)]
        write: WriteArgs,
    },
    Export {
        #[command(flatten)]
        args: InputArgs,
        #[command(flatten)]
        write: WriteArgs,
    },
    Record {
        #[arg(long)]
        input: String,
        #[command(flatten)]
        args: RecordArgs,
        #[command(flatten)]
        write: WriteArgs,
    },
    Replay {
        #[command(flatten)]
        args: InputArgs,
        /// Decode signals during replay; repeat for each channel/database.
        #[arg(long, value_name = "CHANNEL=PATH")]
        dbc: Vec<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum)]
        format: Option<canlog::formats::Format>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        sync: bool,
        #[arg(long)]
        allow_loss: Vec<String>,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        #[arg(long)]
        no_wait: bool,
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        #[arg(long,value_parser=canlog::core::seconds_ns,default_value="0")]
        repeat_gap: i64,
        #[arg(long, value_enum, default_value = "error")]
        on_regression: Regression,
        #[arg(long, value_enum, default_value = "jsonl")]
        sink: Sink,
        #[arg(long)]
        control_stdin: bool,
    },
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    Create {
        workspace: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    Info {
        workspace: PathBuf,
    },
    Add {
        workspace: PathBuf,
        input: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        id_map: Option<PathBuf>,
    },
    /// Reconnect a registered log to a file with identical content.
    Relink {
        workspace: PathBuf,
        log: String,
        input: PathBuf,
        /// Required for legacy entries when the old source and identity are unavailable.
        #[arg(long)]
        expected_sha256: Option<String>,
    },
    /// Replace all stored DBC bindings for one registered log.
    Bind {
        workspace: PathBuf,
        log: String,
        #[arg(long, required = true)]
        dbc: Vec<String>,
    },
    Index {
        workspace: PathBuf,
        #[command(flatten)]
        args: InputArgs,
        #[arg(long, default_value_t = 1024)]
        stride: u64,
    },
    Query {
        workspace: PathBuf,
        #[command(flatten)]
        args: InputArgs,
        #[command(flatten)]
        output: canlog::analysis::StreamOutput,
    },
    Decode {
        workspace: PathBuf,
        #[command(flatten)]
        args: InputArgs,
        #[arg(long)]
        dbc: Vec<String>,
        #[arg(long)]
        no_cache: bool,
        #[command(flatten)]
        output: canlog::analysis::StreamOutput,
    },
    Cache {
        workspace: PathBuf,
        #[command(subcommand)]
        command: CacheCommand,
    },
}
#[derive(Subcommand)]
enum CacheCommand {
    Info,
    Clear,
    Limit { mib: u64 },
}

fn workspace_command(command: WorkspaceCommand, cancel: &Cancellation) -> anyhow::Result<i32> {
    use canlog::workspace as ws;
    let value = match command {
        WorkspaceCommand::Create { workspace, name } => {
            serde_json::to_value(ws::create(&workspace, name.as_deref(), cancel)?)?
        }
        WorkspaceCommand::Info { workspace } => {
            serde_json::to_value(ws::Workspace::load(&workspace)?.manifest)?
        }
        WorkspaceCommand::Add {
            workspace,
            input,
            name,
            id_map,
        } => serde_json::to_value(ws::add(
            &workspace,
            &input,
            &name,
            id_map.as_deref(),
            cancel,
        )?)?,
        WorkspaceCommand::Relink {
            workspace,
            log,
            input,
            expected_sha256,
        } => serde_json::to_value(ws::relink(
            &workspace,
            &log,
            &input,
            expected_sha256.as_deref(),
            cancel,
        )?)?,
        WorkspaceCommand::Bind {
            workspace,
            log,
            dbc,
        } => serde_json::to_value(ws::bind(&workspace, &log, &dbc, cancel)?)?,
        WorkspaceCommand::Index {
            workspace,
            args,
            stride,
        } => {
            if let Some(path) = &args.report {
                let ws = ws::Workspace::load(&workspace)?;
                ws.protect_output(path)?;
                anyhow::ensure!(!path.exists(), "report exists");
            }
            let manifest = ws::build_index(&workspace, &args, stride, cancel)?;
            if let Some(path) = &args.report {
                canlog::output::write_report(path, &manifest, false)?;
            }
            serde_json::to_writer_pretty(std::io::stdout(), &manifest)?;
            println!();
            return Ok(if manifest.issues > 0 { 3 } else { 0 });
        }
        WorkspaceCommand::Query {
            workspace,
            args,
            output,
        } => {
            let (report, result) = ws::stream(&workspace, &args, None, &output, true, cancel);
            return finish_analysis(report, result);
        }
        WorkspaceCommand::Decode {
            workspace,
            args,
            dbc,
            no_cache,
            output,
        } => {
            let (report, result) =
                ws::stream(&workspace, &args, Some(&dbc), &output, no_cache, cancel);
            return finish_analysis(report, result);
        }
        WorkspaceCommand::Cache { workspace, command } => match command {
            CacheCommand::Info => ws::cache_info(&workspace)?,
            CacheCommand::Clear => {
                cancel.check()?;
                ws::clear_cache(&workspace)?;
                ws::cache_info(&workspace)?
            }
            CacheCommand::Limit { mib } => {
                serde_json::to_value(ws::set_quota(&workspace, mib, cancel)?)?
            }
        },
    };
    serde_json::to_writer_pretty(std::io::stdout(), &value)?;
    println!();
    Ok(0)
}

#[derive(Subcommand)]
enum IndexCommand {
    Build {
        #[command(flatten)]
        args: InputArgs,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, default_value_t = 1024)]
        stride: u64,
    },
    Info {
        index: PathBuf,
    },
    Query {
        #[command(flatten)]
        args: InputArgs,
        #[arg(long)]
        index: PathBuf,
        #[command(flatten)]
        output: canlog::analysis::StreamOutput,
    },
}

fn extended(command: Command) -> i32 {
    let cancel = Cancellation::default();
    let handler = cancel.clone();
    let result = (|| -> anyhow::Result<i32> {
        ctrlc::set_handler(move || handler.cancel())?;
        match command {
            Command::Workspace { command } => workspace_command(command, &cancel),
            Command::Index {
                command:
                    IndexCommand::Build {
                        args,
                        output,
                        overwrite,
                        stride,
                    },
            } => {
                if let Some(path) = &args.report {
                    canlog::index::protect(path, std::path::Path::new(&args.input))?;
                    canlog::output::ensure_distinct_paths(path, &output)?;
                    anyhow::ensure!(
                        !path.exists() && path != &output,
                        "report path exists or conflicts with output"
                    );
                }
                let manifest = canlog::index::build(&args, &output, overwrite, stride, &cancel)?;
                serde_json::to_writer_pretty(std::io::stdout(), &manifest)?;
                println!();
                if let Some(path) = &args.report {
                    canlog::output::write_report(path, &manifest, false)?;
                }
                eprintln!(
                    "index published: frames={}, issues={}, chunks={}",
                    manifest.frames, manifest.issues, manifest.chunks
                );
                Ok(if manifest.issues > 0 { 3 } else { 0 })
            }
            Command::Index {
                command: IndexCommand::Info { index },
            } => {
                serde_json::to_writer_pretty(std::io::stdout(), &canlog::index::info(&index)?)?;
                println!();
                Ok(0)
            }
            Command::Index {
                command:
                    IndexCommand::Query {
                        args,
                        index,
                        output,
                    },
            } => analysis_stream(args, Some(index), None, output, &cancel),
            Command::Decode {
                args,
                dbc,
                index,
                output,
            } => analysis_stream(args, index, Some(dbc), output, &cancel),
            _ => unreachable!("only analysis commands reach this dispatcher"),
        }
    })();
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            if error.is::<Cancelled>() {
                130
            } else {
                1
            }
        }
    }
}

fn analysis_stream(
    args: InputArgs,
    index: Option<PathBuf>,
    dbc: Option<Vec<String>>,
    output: canlog::analysis::StreamOutput,
    cancel: &Cancellation,
) -> anyhow::Result<i32> {
    let (report, result) =
        canlog::analysis::stream(&args, index.as_deref(), dbc.as_deref(), &output, cancel);
    finish_analysis(report, result)
}

fn finish_analysis(
    report: canlog::analysis::AnalysisReport,
    result: anyhow::Result<()>,
) -> anyhow::Result<i32> {
    eprintln!("{}: examined={}, selected={}, issues={}, chunks={:?}, decode={:?}, selection_complete={}, published={}",
        report.status, report.frames_examined, report.frames_selected, report.issues_selected, report.chunks_read, report.decode_counts,
        report.selection_complete, report.published);
    if let Some(cache) = &report.cache {
        eprintln!(
            "cache: hits={}, misses={}, written={}, evicted={}, quota_skips={}",
            cache.hits, cache.misses, cache.rows_written, cache.rows_evicted, cache.quota_skips
        );
    }
    result?;
    Ok(if report.status == "partial" { 3 } else { 0 })
}
// Record keeps the same input/filter flags, with --input instead of a positional source.
#[derive(clap::Args)]
struct RecordArgs {
    #[arg(long, value_enum)]
    input_format: Option<canlog::formats::Format>,
    #[arg(long)]
    id_map: Option<PathBuf>,
    #[arg(long)]
    preserve_records: bool,
    #[arg(long, value_enum, default_value = "error")]
    unsupported: Unsupported,
    #[arg(long)]
    recover: bool,
    #[arg(long)]
    channel: Vec<u16>,
    #[arg(long)]
    id: Vec<String>,
    #[arg(long, value_enum)]
    id_kind: Option<IdKind>,
    #[arg(long, value_enum)]
    direction: Option<canlog::core::Direction>,
    #[arg(long,value_parser=canlog::core::seconds_ns)]
    start: Option<i64>,
    #[arg(long,value_parser=canlog::core::seconds_ns)]
    end: Option<i64>,
    #[arg(long)]
    limit: Option<u64>,
    #[arg(long)]
    report: Option<PathBuf>,
    #[arg(long, default_value_t = 65536)]
    max_line_bytes: usize,
    #[arg(long, default_value_t = 1048576)]
    max_object_bytes: usize,
    #[arg(long, default_value_t = 8388608)]
    max_container_bytes: usize,
}
impl RecordArgs {
    fn input(self, input: String) -> InputArgs {
        InputArgs {
            input,
            input_format: self.input_format,
            id_map: self.id_map,
            preserve_records: self.preserve_records,
            unsupported: self.unsupported,
            recover: self.recover,
            channel: self.channel,
            id: self.id,
            id_kind: self.id_kind,
            direction: self.direction,
            start: self.start,
            end: self.end,
            limit: self.limit,
            report: self.report,
            max_line_bytes: self.max_line_bytes,
            max_object_bytes: self.max_object_bytes,
            max_container_bytes: self.max_container_bytes,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if matches!(
        &cli.command,
        Command::Index { .. } | Command::Decode { .. } | Command::Workspace { .. }
    ) {
        std::process::exit(extended(cli.command));
    }
    let (args, operation, control) = match cli.command {
        Command::Index { .. } | Command::Decode { .. } | Command::Workspace { .. } => {
            unreachable!("handled by analysis dispatcher")
        }
        Command::Info { args, scan } => (args, Operation::Info { scan }, false),
        Command::Stats { args } => (args, Operation::Stats, false),
        Command::View { args, unlimited } => (args, Operation::View { unlimited }, false),
        Command::Convert {
            args,
            output,
            format,
            overwrite,
            sync,
            allow_loss,
        } => (
            args,
            Operation::Write(WriteArgs {
                output,
                format,
                overwrite,
                sync,
                allow_loss,
            }),
            false,
        ),
        Command::Filter { args, write } | Command::Export { args, write } => {
            (args, Operation::Write(write), false)
        }
        Command::Record { input, args, write } => {
            (args.input(input), Operation::Write(write), false)
        }
        Command::Replay {
            args,
            dbc,
            output,
            format,
            overwrite,
            sync,
            allow_loss,
            speed,
            no_wait,
            repeat,
            repeat_gap,
            on_regression,
            sink,
            control_stdin,
        } => {
            if control_stdin && args.input == "-" {
                eprintln!("control stdin conflicts with input stdin");
                std::process::exit(2);
            }
            if output.is_none() && (format.is_some() || overwrite || sync || !allow_loss.is_empty())
            {
                eprintln!("file options require --output");
                std::process::exit(2);
            }
            let write = output.map(|output| WriteArgs {
                output,
                format,
                overwrite,
                sync,
                allow_loss,
            });
            (
                args,
                Operation::Replay {
                    write,
                    dbc,
                    speed,
                    no_wait,
                    repeat,
                    gap: repeat_gap,
                    regression: on_regression,
                    sink,
                },
                control_stdin,
            )
        }
    };
    if let Err(e) = validate(&args, &operation) {
        eprintln!("configuration error: {e:#}");
        std::process::exit(2);
    }
    let cancel = Cancellation::default();
    let handler = cancel.clone();
    if let Err(e) = ctrlc::set_handler(move || handler.cancel()) {
        eprintln!("cannot install cancel handler: {e}");
        std::process::exit(1);
    }
    let controls = if control {
        let (tx, rx) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else {
                    break;
                };
                let command = match line.trim() {
                    "pause" => Control::Pause,
                    "resume" => Control::Resume,
                    "stop" => Control::Stop,
                    _ => {
                        eprintln!("control expects pause, resume, or stop");
                        continue;
                    }
                };
                if tx.send(command).is_err() {
                    break;
                }
            }
        });
        Some(rx)
    } else {
        None
    };
    let (report, result) = run(args, operation, cancel, controls);
    eprintln!(
        "{}: read={}, selected={}, written={}, issues={}, finalized={}, published={}, elapsed={}ms",
        report.status,
        report.frames_read,
        report.frames_selected,
        report.frames_written,
        report.issues,
        report.finalized,
        report.published,
        report.elapsed_ms
    );
    if !report.losses.is_empty() {
        eprintln!("losses: {:?}", report.losses);
    }
    if !report.decode_counts.is_empty() {
        eprintln!("DBC frame statuses: {:?}", report.decode_counts);
    }
    if report.native_records_written > 0 {
        eprintln!(
            "preserved: native={}, non-frame={}, categories={:?}",
            report.native_records_written, report.issues_preserved, report.preserved_counts
        );
    }
    if report.issues > 0 {
        eprintln!(
            "issue counts: {:?} (use --report for locations)",
            report.issue_counts
        );
    }
    let code = match result {
        Ok(()) => {
            if report.status == "partial" {
                3
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("{e:#}");
            if e.is::<Cancelled>() {
                130
            } else {
                1
            }
        }
    };
    std::process::exit(code);
}
