pub mod asc;
pub mod blf;
pub mod jsonl;

use crate::core::{Limits, LogReader, LogWriter, Metadata};
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Asc,
    Blf,
    Jsonl,
    Csv,
}

pub fn output_format(path: &Path, explicit: Option<Format>) -> Result<Format> {
    if let Some(f) = explicit {
        return Ok(f);
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "asc" => Ok(Format::Asc),
        "blf" => Ok(Format::Blf),
        "jsonl" => Ok(Format::Jsonl),
        "csv" => Ok(Format::Csv),
        _ => bail!("cannot infer format; use --format"),
    }
}

pub fn open_reader(
    path: &str,
    explicit: Option<Format>,
    limits: Limits,
) -> Result<Box<dyn LogReader>> {
    if path == "-" {
        ensure!(
            explicit == Some(Format::Jsonl),
            "stdin requires --input-format jsonl"
        );
        return Ok(Box::new(jsonl::JsonReader::new(
            Box::new(BufReader::new(std::io::stdin())),
            path,
            limits,
        )));
    }
    let file = File::open(path).with_context(|| format!("opening {path}"))?;
    let mut input = BufReader::new(file);
    let probe = input.fill_buf()?;
    let probe = probe.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(probe);
    let detected = if probe.starts_with(b"LOGG") {
        Format::Blf
    } else if probe.starts_with(b"date ") || probe.starts_with(b"base ") || probe.starts_with(b"//")
    {
        Format::Asc
    } else if probe.iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'{') {
        Format::Jsonl
    } else {
        explicit.context("unrecognized header; specify --input-format")?
    };
    if let Some(f) = explicit {
        ensure!(
            f == detected,
            "explicit input format disagrees with file header"
        );
    }
    input.seek(SeekFrom::Start(0))?;
    match detected {
        Format::Asc => Ok(Box::new(asc::AscReader::new(Box::new(input), path, limits))),
        Format::Blf => Ok(Box::new(blf::BlfReader::new(input, path, limits)?)),
        Format::Jsonl => Ok(Box::new(jsonl::JsonReader::new(
            Box::new(input),
            path,
            limits,
        ))),
        Format::Csv => bail!("CSV is export-only"),
    }
}

/// Keeps CLI stdin backpressure bounded while allowing cancellation during a blocked read.
pub fn open_cancellable_reader(
    path: &str,
    explicit: Option<Format>,
    limits: Limits,
    cancel: crate::playback::Cancellation,
) -> Result<Box<dyn LogReader>> {
    if path != "-" {
        return open_reader(path, explicit, limits);
    }
    ensure!(
        explicit == Some(Format::Jsonl),
        "stdin requires --input-format jsonl"
    );
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut reader =
            jsonl::JsonReader::new(Box::new(BufReader::new(std::io::stdin())), "-", limits);
        loop {
            let item = reader.next_item();
            let done = !matches!(&item, Ok(Some(_)));
            if tx.send((item, reader.metadata().clone())).is_err() || done {
                break;
            }
        }
    });
    Ok(Box::new(StdinReader {
        receiver: rx,
        cancel,
        metadata: Metadata {
            format: "jsonl".into(),
            profile: "canlog-frame-v1".into(),
            source: "-".into(),
            ..Default::default()
        },
    }))
}
struct StdinReader {
    receiver: std::sync::mpsc::Receiver<(Result<Option<crate::core::ReadItem>>, Metadata)>,
    cancel: crate::playback::Cancellation,
    metadata: Metadata,
}

impl LogReader for StdinReader {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn next_item(&mut self) -> Result<Option<crate::core::ReadItem>> {
        loop {
            self.cancel.check()?;
            match self
                .receiver
                .recv_timeout(std::time::Duration::from_millis(10))
            {
                Ok((result, metadata)) => {
                    self.metadata = metadata;
                    return result;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

pub fn make_writer(file: File, format: Format, metadata: &Metadata) -> Result<Box<dyn LogWriter>> {
    match format {
        Format::Asc => Ok(Box::new(asc::AscWriter::new(file, metadata)?)),
        Format::Blf => Ok(Box::new(blf::BlfWriter::new(file, metadata)?)),
        Format::Jsonl => Ok(Box::new(jsonl::JsonWriter::new(file))),
        Format::Csv => Ok(Box::new(jsonl::CsvWriter::new(file)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_pending_stdin_channel() {
        let cancel = crate::playback::Cancellation::default();
        let signal = cancel.clone();
        let (_tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut reader = StdinReader {
            receiver: rx,
            cancel,
            metadata: Metadata::default(),
        };
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            signal.cancel();
        });
        let start = std::time::Instant::now();
        assert!(reader
            .next_item()
            .unwrap_err()
            .is::<crate::playback::Cancelled>());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        worker.join().unwrap();
    }
}
