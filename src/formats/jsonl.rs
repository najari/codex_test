use crate::core::*;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufWriter, Write},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub schema_version: u32,
    pub frame: Frame,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_location: Option<Location>,
}
pub fn write_json(output: &mut dyn Write, f: &Frame) -> Result<()> {
    serde_json::to_writer(
        &mut *output,
        &Envelope {
            schema_version: 1,
            frame: f.clone(),
            source_location: None,
        },
    )?;
    writeln!(output)?;
    Ok(())
}
pub fn write_json_record(output: &mut dyn Write, f: &Frame, location: &Location) -> Result<()> {
    serde_json::to_writer(
        &mut *output,
        &Envelope {
            schema_version: 1,
            frame: f.clone(),
            source_location: Some(location.clone()),
        },
    )?;
    writeln!(output)?;
    Ok(())
}
pub struct JsonReader {
    input: Box<dyn BufRead>,
    metadata: Metadata,
    limits: Limits,
    line: Vec<u8>,
    line_no: u64,
    ordinal: u64,
}
impl JsonReader {
    pub fn new(input: Box<dyn BufRead>, source: &str, limits: Limits) -> Self {
        Self {
            input,
            metadata: Metadata {
                format: "jsonl".into(),
                profile: "canlog-frame-v1".into(),
                source: source.into(),
                ..Default::default()
            },
            limits,
            line: Vec::new(),
            line_no: 0,
            ordinal: 0,
        }
    }
}
impl LogReader for JsonReader {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn next_item(&mut self) -> Result<Option<ReadItem>> {
        while bounded_line(&mut *self.input, &mut self.line, self.limits.max_line)? {
            self.line_no += 1;
            if self.line.iter().all(|c| c.is_ascii_whitespace()) {
                continue;
            }
            let parsed = (|| -> Result<Frame> {
                let envelope: Envelope = serde_json::from_slice(&self.line)?;
                ensure!(
                    envelope.schema_version == 1,
                    "unsupported JSONL schema version"
                );
                envelope.frame.validate()?;
                if envelope.source_location.is_some()
                    && !self
                        .metadata
                        .notes
                        .iter()
                        .any(|s| s == "input frame provenance")
                {
                    self.metadata.notes.push("input frame provenance".into());
                }
                Ok(envelope.frame)
            })();
            let location = Location {
                source: self.metadata.source.clone(),
                ordinal: self.ordinal,
                line: Some(self.line_no),
                ..Default::default()
            };
            self.ordinal += 1;
            return Ok(Some(match parsed {
                Ok(frame) => ReadItem::Frame(FrameRecord { frame, location }),
                Err(e) => ReadItem::Issue(Issue {
                    kind: IssueKind::CorruptedRegion,
                    message: e.to_string(),
                    location,
                    timestamp_ns: None,
                    channel: None,
                    object_type: None,
                }),
            }));
        }
        Ok(None)
    }
}
pub struct JsonWriter {
    output: BufWriter<File>,
    finished: bool,
}
impl JsonWriter {
    pub fn new(file: File) -> Self {
        Self {
            output: BufWriter::new(file),
            finished: false,
        }
    }
}
impl LogWriter for JsonWriter {
    fn write_frame(&mut self, f: &Frame) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        f.validate()?;
        write_json(&mut self.output, f)
    }
    fn finish(&mut self) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        self.output.flush()?;
        self.finished = true;
        Ok(())
    }
}
pub struct CsvWriter {
    output: BufWriter<File>,
    finished: bool,
}
impl CsvWriter {
    pub fn new(file: File) -> Result<Self> {
        let mut output = BufWriter::new(file);
        writeln!(
            output,
            "timestamp_ns,channel,id,extended,direction,remote,fd,raw_dlc,data_hex,brs,esi"
        )?;
        Ok(Self {
            output,
            finished: false,
        })
    }
}
impl LogWriter for CsvWriter {
    fn write_frame(&mut self, f: &Frame) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        f.validate()?;
        let data: String = f.data().iter().map(|b| format!("{b:02X}")).collect();
        writeln!(
            self.output,
            "{},{},{},{},{:?},{},{},{},{},{},{}",
            f.timestamp_ns(),
            f.channel(),
            f.id(),
            f.extended(),
            f.direction(),
            f.remote(),
            f.fd(),
            f.raw_dlc(),
            data,
            f.brs(),
            f.esi()
        )?;
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        self.output.flush()?;
        self.finished = true;
        Ok(())
    }
}
