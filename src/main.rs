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
// Record keeps the same input/filter flags, with --input instead of a positional source.
#[derive(clap::Args)]
struct RecordArgs {
    #[arg(long, value_enum)]
    input_format: Option<canlog::formats::Format>,
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
    let (args, operation, control) = match cli.command {
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
