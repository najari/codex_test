use crate::core::*;
use anyhow::{anyhow, bail, ensure, Context, Result};
use std::{
    fs::File,
    io::{BufRead, BufWriter, Write},
};

pub struct AscReader {
    input: Box<dyn BufRead>,
    metadata: Metadata,
    limits: Limits,
    line: Vec<u8>,
    line_no: u64,
    ordinal: u64,
    radix: u32,
    relative: bool,
    time: i64,
    triggers: u32,
    preserve: bool,
    id_map: Option<std::sync::Arc<crate::id_map::IdMap>>,
    mapped_current: bool,
}
impl AscReader {
    pub fn new(input: Box<dyn BufRead>, source: &str, limits: Limits) -> Self {
        Self {
            input,
            metadata: Metadata {
                format: "asc".into(),
                profile: "vector-text-can-v1".into(),
                source: source.into(),
                ..Default::default()
            },
            limits,
            line: Vec::new(),
            line_no: 0,
            ordinal: 0,
            radix: 16,
            relative: false,
            time: 0,
            triggers: 0,
            preserve: false,
            id_map: None,
            mapped_current: false,
        }
    }
    fn location(&self) -> Location {
        Location {
            source: self.metadata.source.clone(),
            line: Some(self.line_no),
            ordinal: self.ordinal,
            ..Default::default()
        }
    }
    fn issue(
        &self,
        kind: IssueKind,
        message: String,
        timestamp_ns: Option<i64>,
        channel: Option<u16>,
    ) -> ReadItem {
        ReadItem::Issue(Issue {
            kind,
            message,
            location: self.location(),
            timestamp_ns,
            channel,
            object_type: None,
            native: None,
        })
    }
    fn resolve_id(&mut self, token: &str, channel: u16) -> Result<(u32, bool)> {
        if let Ok(id) = parse_id(token, self.radix) {
            return Ok(id);
        }
        let id = self
            .id_map
            .as_ref()
            .and_then(|m| m.resolve(channel, token))
            .with_context(|| format!("numeric ID missing for {token} on channel {channel}"))?;
        self.mapped_current = true;
        Ok(id)
    }
    fn parse_frame(&mut self, t: &[&str], timestamp: i64) -> Result<Frame> {
        if t[1] == "CANFD" {
            ensure!(t.len() >= 9, "truncated CAN FD row");
            let channel = t[2].parse()?;
            let direction = direction(t[3])?;
            let (id, extended) = self.resolve_id(t[4], channel)?;
            let i = if matches!(t[5], "0" | "1") { 5 } else { 6 };
            ensure!(t.len() >= i + 4, "truncated CAN FD fields");
            let brs = flag(t[i])?;
            let esi = flag(t[i + 1])?;
            let dlc = u8::from_str_radix(t[i + 2], self.radix)?;
            let len: usize = t[i + 3].parse()?;
            ensure!(
                len <= 64 && t.len() >= i + 4 + len,
                "truncated CAN FD payload"
            );
            let data = self.bytes(&t[i + 4..i + 4 + len])?;
            let extra = &t[i + 4 + len..];
            ensure!(
                extra.is_empty() || extra.len() == 8,
                "unexpected CAN FD trailing fields"
            );
            if !extra.is_empty() {
                for field in extra {
                    u64::from_str_radix(field, 16).context("invalid CAN FD ancillary field")?;
                }
                let expected = 0x1000 | if brs { 0x2000 } else { 0 } | if esi { 0x4000 } else { 0 };
                if extra.iter().enumerate().any(|(j, s)| {
                    u64::from_str_radix(s, 16).unwrap() != if j == 2 { expected } else { 0 }
                }) {
                    self.note_annotations();
                }
            }
            Frame::new(
                timestamp, channel, id, extended, direction, false, true, dlc, data, brs, esi,
            )
        } else {
            ensure!(t.len() >= 6, "truncated Classic CAN row");
            let channel = t[1].parse()?;
            // CANoe symbolic exports use decimal ID= trailers independently of base.
            let trailer_id = t
                .windows(3)
                .find(|w| w[0] == "ID" && w[1] == "=")
                .map(|w| w[2]);
            let (id, extended) = if let Some(raw) = trailer_id {
                parse_id(raw, 10)?
            } else {
                self.resolve_id(t[2], channel)?
            };
            let direction = direction(t[3])?;
            ensure!(
                matches!(t[4], "d" | "D" | "r" | "R"),
                "unknown CAN frame type"
            );
            let remote = t[4].eq_ignore_ascii_case("r");
            let dlc = u8::from_str_radix(t[5], self.radix)?;
            ensure!(dlc <= 15, "invalid DLC");
            let len = if remote { 0 } else { dlc.min(8) as usize };
            ensure!(t.len() >= 6 + len, "truncated Classic CAN payload");
            let data = self.bytes(&t[6..6 + len])?;
            let extra = &t[6 + len..];
            ensure!(
                extra.len().is_multiple_of(3),
                "unexpected Classic CAN trailing data"
            );
            for field in extra.chunks(3) {
                ensure!(
                    matches!(field[0], "Length" | "BitCount" | "ID") && field[1] == "=",
                    "unsupported Classic CAN ancillary field"
                );
            }
            if !extra.is_empty() {
                self.note_annotations();
            }
            Frame::new(
                timestamp, channel, id, extended, direction, remote, false, dlc, data, false, false,
            )
        }
    }
    fn bytes(&self, t: &[&str]) -> Result<Vec<u8>> {
        t.iter()
            .map(|b| Ok(u8::from_str_radix(b, self.radix)?))
            .collect()
    }
    fn note_annotations(&mut self) {
        if !self.metadata.notes.iter().any(|s| s == "frame annotations") {
            self.metadata.notes.push("frame annotations".into());
        }
    }
}
fn direction(t: &str) -> Result<Direction> {
    match t {
        "Rx" => Ok(Direction::Rx),
        "Tx" => Ok(Direction::Tx),
        _ => bail!("unknown direction {t}"),
    }
}
fn flag(t: &str) -> Result<bool> {
    match t {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => bail!("invalid boolean flag"),
    }
}

impl LogReader for AscReader {
    fn configure(
        &mut self,
        preserve: bool,
        id_map: Option<std::sync::Arc<crate::id_map::IdMap>>,
    ) -> Result<()> {
        ensure!(
            !preserve || id_map.is_none(),
            "preserve-records and ID mapping cannot be combined"
        );
        self.preserve = preserve;
        self.id_map = id_map;
        Ok(())
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn next_item(&mut self) -> Result<Option<ReadItem>> {
        while bounded_line(&mut *self.input, &mut self.line, self.limits.max_line)? {
            self.line_no += 1;
            let text = match std::str::from_utf8(&self.line) {
                Ok(s) => s.trim().trim_start_matches('\u{feff}').to_owned(),
                Err(_) => {
                    let (decoded, _, invalid) = encoding_rs::WINDOWS_1252.decode(&self.line);
                    ensure!(
                        !invalid,
                        "invalid ASC Windows-1252 text at line {}",
                        self.line_no
                    );
                    if !self
                        .metadata
                        .notes
                        .iter()
                        .any(|s| s == "Windows-1252 input")
                    {
                        self.metadata.notes.push("Windows-1252 input".into());
                    }
                    decoded.trim().to_owned()
                }
            };
            if text == "// canlog-date-origin unknown" {
                self.metadata.date_text = None;
                self.metadata.blf_start = None;
                if !self
                    .metadata
                    .notes
                    .iter()
                    .any(|s| s == "synthetic date placeholder")
                {
                    self.metadata
                        .notes
                        .push("synthetic date placeholder".into());
                }
                continue;
            }
            if text.is_empty() || text.starts_with("//") {
                continue;
            }
            if let Some(date) = text.strip_prefix("date ") {
                self.metadata.date_text = Some(date.into());
                self.metadata.blf_start = parse_date(date);
                if let Some((_, rest)) = date.split_once('.') {
                    let fraction: String =
                        rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if fraction.len() > 3
                        && fraction[3..].bytes().any(|b| b != b'0')
                        && !self
                            .metadata
                            .notes
                            .iter()
                            .any(|s| s == "submillisecond date origin")
                    {
                        self.metadata
                            .notes
                            .push("submillisecond date origin".into());
                    }
                }
                continue;
            }
            if text.starts_with("base ") {
                let tokens: Vec<_> = text.split_whitespace().collect();
                self.radix = match tokens.get(1) {
                    Some(&"hex") => 16,
                    Some(&"dec") => 10,
                    _ => bail!("invalid ASC base at line {}", self.line_no),
                };
                let mode = tokens.get(3).copied().unwrap_or("absolute");
                ensure!(
                    matches!(mode, "absolute" | "relative"),
                    "invalid ASC time mode"
                );
                self.relative = mode == "relative";
                self.metadata.time_mode = Some(mode.into());
                self.metadata.asc_radix = Some(self.radix);
                continue;
            }
            if text.ends_with("internal events logged") {
                continue;
            }
            if text.starts_with("Begin Triggerblock") {
                self.triggers += 1;
                ensure!(self.triggers==1,"multiple ASC trigger blocks require separate clock handling; this profile does not merge them");
                self.time = 0;
                if !self
                    .metadata
                    .notes
                    .iter()
                    .any(|s| s == "trigger block boundary")
                {
                    self.metadata.notes.push("trigger block boundary".into());
                }
                continue;
            }
            if text == "End TriggerBlock" || text == "End Triggerblock" {
                continue;
            }
            let t: Vec<_> = text.split_whitespace().collect();
            let timestamp = seconds_ns(t[0]);
            if timestamp.is_err() {
                ensure!(
                    !self.relative,
                    "invalid relative record timestamp: {}; subsequent timestamps cannot be recovered",
                    t[0]
                );
                let item = self.issue(
                    IssueKind::CorruptedRegion,
                    format!("invalid record timestamp: {}", t[0]),
                    None,
                    None,
                );
                self.ordinal += 1;
                return Ok(Some(item));
            }
            let delta = timestamp?;
            self.time = if self.relative {
                self.time
                    .checked_add(delta)
                    .ok_or_else(|| anyhow!("relative timestamp overflow"))?
            } else {
                delta
            };
            let timestamp = self.time;
            if text.contains("Start of measurement") && !self.preserve {
                continue;
            }
            let channel = t.get(1).and_then(|c| c.parse::<u16>().ok());
            let can = channel.is_some() || t.get(1) == Some(&"CANFD");
            let symbolic = if t.get(1) == Some(&"CANFD") {
                t.get(2)
                    .and_then(|c| c.parse::<u16>().ok())
                    .zip(t.get(4).copied())
            } else if t.get(3).is_some_and(|d| matches!(*d, "Rx" | "Tx")) {
                channel.zip(t.get(2).copied())
            } else {
                None
            };
            self.mapped_current = false;
            let mut item = if text.contains("ErrorFrame") && can {
                let error_channel = channel.or_else(|| t.get(2).and_then(|c| c.parse().ok()));
                self.issue(
                    IssueKind::CanError,
                    text.clone(),
                    Some(timestamp),
                    error_channel,
                )
            } else if !can
                || (channel.is_some()
                    && (t.get(2).is_some_and(|v| v.starts_with("Statistic:"))
                        || t.get(3) == Some(&"=")))
            {
                self.issue(
                    IssueKind::UnsupportedRecord,
                    text.clone(),
                    Some(timestamp),
                    channel,
                )
            } else if symbolic.is_some_and(|(c, token)| {
                parse_id(token, self.radix).is_err()
                    && self
                        .id_map
                        .as_ref()
                        .and_then(|m| m.resolve(c, token))
                        .is_none()
            }) && !t.windows(3).any(|w| w[0] == "ID" && w[1] == "=")
            {
                self.issue(
                    IssueKind::UnresolvedId,
                    format!("numeric ID missing for {}", symbolic.unwrap().1),
                    Some(timestamp),
                    channel,
                )
            } else {
                match self.parse_frame(&t, timestamp) {
                    Ok(frame) => {
                        if self.mapped_current {
                            self.metadata.mapped_frames += 1;
                            self.note_annotations();
                        }
                        if text.contains("Length =")
                            && !self.metadata.notes.iter().any(|s| s == "frame annotations")
                        {
                            self.metadata.notes.push("frame annotations".into());
                        }
                        ReadItem::Frame(FrameRecord {
                            frame,
                            location: self.location(),
                            native: None,
                        })
                    }
                    Err(e) => self.issue(
                        IssueKind::CorruptedRegion,
                        e.to_string(),
                        Some(timestamp),
                        channel,
                    ),
                }
            };
            if self.preserve {
                let native = NativeRecord::Asc {
                    timestamp_ns: timestamp,
                    radix: self.radix,
                    body: text
                        .split_once(char::is_whitespace)
                        .map_or("", |(_, body)| body.trim_start())
                        .to_owned(),
                };
                match &mut item {
                    ReadItem::Frame(r) => r.native = Some(native),
                    ReadItem::Issue(i) if i.kind != IssueKind::CorruptedRegion => {
                        i.native = Some(native)
                    }
                    _ => {}
                }
            }
            self.ordinal += 1;
            return Ok(Some(item));
        }
        Ok(None)
    }
}

pub struct AscWriter {
    output: BufWriter<File>,
    finished: bool,
    radix: u32,
}
impl AscWriter {
    pub fn new(file: File, metadata: &Metadata) -> Result<Self> {
        Self::with_preservation(file, metadata, false)
    }
    pub fn with_preservation(file: File, metadata: &Metadata, preserve: bool) -> Result<Self> {
        let radix = if preserve {
            metadata.asc_radix.unwrap_or(16)
        } else {
            16
        };
        let mut output = BufWriter::new(file);
        writeln!(
            output,
            "date {}",
            metadata
                .date_text
                .as_deref()
                .unwrap_or("Thu Jan 1 00:00:00.000 1970")
        )?;
        writeln!(output, "base {} timestamps absolute\n{}internal events logged\n// canlog; offsets preserved; date has no inferred UTC timezone", if radix == 10 { "dec" } else { "hex" }, if preserve { "" } else { "no " })?;
        if metadata.date_text.is_none() {
            writeln!(output, "// canlog-date-origin unknown")?;
        }
        Ok(Self {
            output,
            finished: false,
            radix,
        })
    }
}
impl LogWriter for AscWriter {
    fn write_native(&mut self, record: &NativeRecord) -> Result<()> {
        ensure!(!self.finished, "writer already finalized");
        let NativeRecord::Asc {
            timestamp_ns,
            radix,
            body,
        } = record
        else {
            bail!("ASC writer requires ASC native record");
        };
        ensure!(
            *timestamp_ns >= 0 && matches!(*radix, 10 | 16),
            "invalid ASC native record coordinates"
        );
        ensure!(
            *radix == self.radix,
            "ASC native base changed; mixed-base preservation is unsupported"
        );
        ensure!(
            !body.is_empty() && body.len() <= 1048576 && !body.contains(['\n', '\r']),
            "invalid ASC native record body"
        );
        writeln!(self.output, "{} {}", time_text(*timestamp_ns), body)?;
        Ok(())
    }
    fn write_frame(&mut self, f: &Frame) -> Result<()> {
        ensure!(!self.finished, "writer already finalized");
        f.validate()?;
        ensure!(
            f.direction() != Direction::Unknown,
            "ASC cannot preserve unknown direction"
        );
        let dir = if f.direction() == Direction::Rx {
            "Rx"
        } else {
            "Tx"
        };
        let number = |v: u32| {
            if self.radix == 10 {
                v.to_string()
            } else {
                format!("{v:X}")
            }
        };
        let id = format!("{}{}", number(f.id()), if f.extended() { "x" } else { "" });
        write!(self.output, "{} ", time_text(f.timestamp_ns()))?;
        if f.fd() {
            write!(
                self.output,
                "CANFD {} {} {} {} {} {} {}",
                f.channel(),
                dir,
                id,
                u8::from(f.brs()),
                u8::from(f.esi()),
                number(f.raw_dlc() as u32),
                f.data().len()
            )?;
        } else {
            write!(
                self.output,
                "{} {} {} {} {}",
                f.channel(),
                id,
                dir,
                if f.remote() { "r" } else { "d" },
                number(f.raw_dlc() as u32)
            )?;
        }
        for b in f.data() {
            if self.radix == 10 {
                write!(self.output, " {b}")?;
            } else {
                write!(self.output, " {b:02X}")?;
            }
        }
        if f.fd() {
            write!(
                self.output,
                " 0 0 {:X} 0 0 0 0 0",
                0x1000 | if f.brs() { 0x2000 } else { 0 } | if f.esi() { 0x4000 } else { 0 }
            )?;
        }
        writeln!(self.output)?;
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        ensure!(!self.finished, "writer already finalized");
        self.output.flush()?;
        self.finished = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn decimal_trailer_and_relative_events() {
        let data =
            b"base hex timestamps relative\n0.100 SV: unknown\n0.200 1 Name Tx d 1 FF ID = 272\n";
        let mut r = AscReader::new(Box::new(Cursor::new(data)), "test", Limits::default());
        assert!(matches!(r.next_item().unwrap(), Some(ReadItem::Issue(_))));
        let Some(ReadItem::Frame(item)) = r.next_item().unwrap() else {
            panic!()
        };
        assert_eq!(item.frame.id(), 272);
        assert_eq!(item.frame.timestamp_ns(), 300_000_000);
    }
    #[test]
    fn malformed_is_not_padded() {
        let mut r = AscReader::new(
            Box::new(Cursor::new(b"0.0 1 100 Rx d 2 FF\n")),
            "test",
            Limits::default(),
        );
        assert!(matches!(
            r.next_item().unwrap(),
            Some(ReadItem::Issue(Issue {
                kind: IssueKind::CorruptedRegion,
                ..
            }))
        ));
    }
}
