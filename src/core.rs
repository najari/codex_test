use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::io::BufRead;

pub const FD_LENGTHS: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Rx,
    Tx,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FrameWire")]
pub struct Frame {
    timestamp_ns: i64,
    channel: u16,
    id: u32,
    extended: bool,
    direction: Direction,
    remote: bool,
    fd: bool,
    raw_dlc: u8,
    data: Vec<u8>,
    brs: bool,
    esi: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameWire {
    timestamp_ns: i64,
    channel: u16,
    id: u32,
    extended: bool,
    direction: Direction,
    remote: bool,
    fd: bool,
    raw_dlc: u8,
    data: Vec<u8>,
    brs: bool,
    esi: bool,
}
impl TryFrom<FrameWire> for Frame {
    type Error = anyhow::Error;
    fn try_from(f: FrameWire) -> Result<Self> {
        Self::new(
            f.timestamp_ns,
            f.channel,
            f.id,
            f.extended,
            f.direction,
            f.remote,
            f.fd,
            f.raw_dlc,
            f.data,
            f.brs,
            f.esi,
        )
    }
}

impl Frame {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timestamp_ns: i64,
        channel: u16,
        id: u32,
        extended: bool,
        direction: Direction,
        remote: bool,
        fd: bool,
        raw_dlc: u8,
        data: Vec<u8>,
        brs: bool,
        esi: bool,
    ) -> Result<Self> {
        let f = Self {
            timestamp_ns,
            channel,
            id,
            extended,
            direction,
            remote,
            fd,
            raw_dlc,
            data,
            brs,
            esi,
        };
        f.validate()?;
        Ok(f)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.timestamp_ns >= 0, "negative timestamp");
        ensure!(self.channel > 0, "channel must be 1-based and nonzero");
        ensure!(
            self.id <= if self.extended { 0x1fff_ffff } else { 0x7ff },
            "CAN ID outside its declared range"
        );
        ensure!(self.raw_dlc <= 15, "DLC outside 0..15");
        ensure!(!(self.fd && self.remote), "CAN FD remote frame is invalid");
        ensure!(
            self.fd || (!self.brs && !self.esi),
            "Classic CAN cannot have FD flags"
        );
        let expected = if self.remote {
            0
        } else if self.fd {
            FD_LENGTHS[self.raw_dlc as usize]
        } else {
            self.raw_dlc.min(8) as usize
        };
        ensure!(
            self.data.len() == expected,
            "DLC/payload mismatch: DLC {} requires {} bytes, got {}",
            self.raw_dlc,
            expected,
            self.data.len()
        );
        Ok(())
    }
    pub fn timestamp_ns(&self) -> i64 {
        self.timestamp_ns
    }
    pub fn channel(&self) -> u16 {
        self.channel
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn extended(&self) -> bool {
        self.extended
    }
    pub fn direction(&self) -> Direction {
        self.direction
    }
    pub fn remote(&self) -> bool {
        self.remote
    }
    pub fn fd(&self) -> bool {
        self.fd
    }
    pub fn raw_dlc(&self) -> u8 {
        self.raw_dlc
    }
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn brs(&self) -> bool {
        self.brs
    }
    pub fn esi(&self) -> bool {
        self.esi
    }
    pub fn with_timestamp(&self, timestamp_ns: i64) -> Result<Self> {
        let mut f = self.clone();
        f.timestamp_ns = timestamp_ns;
        f.validate()?;
        Ok(f)
    }
    pub fn with_direction(&self, direction: Direction) -> Result<Self> {
        let mut f = self.clone();
        f.direction = direction;
        f.validate()?;
        Ok(f)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Location {
    pub source: String,
    pub ordinal: u64,
    pub line: Option<u64>,
    pub container: Option<u64>,
    pub object_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameRecord {
    pub frame: Frame,
    pub location: Location,
    #[serde(skip)]
    pub native: Option<NativeRecord>,
}

/// Bounded native record content for same-format rewriting. Not a cross-format codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeRecord {
    Asc {
        timestamp_ns: i64,
        radix: u32,
        body: String,
    },
    Blf {
        timestamp_ns: Option<i64>,
        object: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    UnsupportedRecord,
    UnresolvedId,
    ConflictingId,
    CorruptedRegion,
    CanError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub kind: IssueKind,
    pub message: String,
    pub location: Location,
    pub timestamp_ns: Option<i64>,
    pub channel: Option<u16>,
    pub object_type: Option<u32>,
    #[serde(skip)]
    pub native: Option<NativeRecord>,
}

#[derive(Debug, Clone)]
pub enum ReadItem {
    Frame(FrameRecord),
    Issue(Issue),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub format: String,
    pub profile: String,
    pub source: String,
    pub date_text: Option<String>,
    pub time_mode: Option<String>,
    pub declared_objects: Option<u64>,
    pub blf_start: Option<[u16; 8]>,
    pub blf_stop: Option<[u16; 8]>,
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asc_radix: Option<u32>,
    #[serde(default)]
    pub mapped_frames: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_line: usize,
    pub max_object: usize,
    pub max_container: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_line: 64 * 1024,
            max_object: 1024 * 1024,
            max_container: 8 * 1024 * 1024,
        }
    }
}

pub trait LogReader {
    fn metadata(&self) -> &Metadata;
    fn next_item(&mut self) -> Result<Option<ReadItem>>;
    /// A replay position, including parser state. No frame payload is stored.
    fn checkpoint(&mut self) -> Result<ReaderCheckpoint> {
        bail!("this format does not support indexing")
    }
    fn restore(
        &mut self,
        _checkpoint: &ReaderCheckpoint,
        _cancel: &crate::playback::Cancellation,
    ) -> Result<()> {
        bail!("this format does not support indexed seeking")
    }
    fn configure(
        &mut self,
        preserve: bool,
        id_map: Option<std::sync::Arc<crate::id_map::IdMap>>,
    ) -> Result<()> {
        ensure!(
            !preserve && id_map.is_none(),
            "native preservation and ID mapping require ASC/BLF input"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum ReaderCheckpoint {
    Asc {
        offset: u64,
        line: u64,
        ordinal: u64,
        radix: u32,
        relative: bool,
        time: i64,
        triggers: u32,
        metadata: Box<Metadata>,
    },
    Blf {
        offset: u64,
        container: u64,
        ordinal: u64,
        padding_seen: usize,
        skip: u64,
    },
}
pub trait LogWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<()>;
    fn write_native(&mut self, _record: &NativeRecord) -> Result<()> {
        bail!("writer cannot preserve this native record")
    }
    fn finish(&mut self) -> Result<()>;
}

// read_until without an unbounded allocation before checking line length.
pub fn bounded_line(reader: &mut dyn BufRead, line: &mut Vec<u8>, max: usize) -> Result<bool> {
    line.clear();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(!line.is_empty());
        }
        let n = buf
            .iter()
            .position(|&c| c == b'\n')
            .map_or(buf.len(), |i| i + 1);
        ensure!(
            line.len().checked_add(n).is_some_and(|size| size <= max),
            "line exceeds resource limit ({max} bytes)"
        );
        let done = buf[n - 1] == b'\n';
        line.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if done {
            return Ok(true);
        }
    }
}

pub fn seconds_ns(text: &str) -> Result<i64> {
    let text = text.strip_suffix('s').unwrap_or(text);
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    ensure!(
        !whole.is_empty() && whole.bytes().all(|c| c.is_ascii_digit()),
        "invalid nonnegative seconds: {text}"
    );
    ensure!(
        frac.len() <= 9 && frac.bytes().all(|c| c.is_ascii_digit()),
        "timestamp needs at most nine decimal places: {text}"
    );
    let whole: i64 = whole.parse()?;
    let frac: i64 = if frac.is_empty() {
        0
    } else {
        frac.parse::<i64>()? * 10_i64.pow(9 - frac.len() as u32)
    };
    whole
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(frac))
        .ok_or_else(|| anyhow::anyhow!("timestamp overflow"))
}

pub fn time_text(ns: i64) -> String {
    format!("{}.{:09}", ns / 1_000_000_000, ns % 1_000_000_000)
}

pub fn parse_id(text: &str, radix: u32) -> Result<(u32, bool)> {
    let extended = text.ends_with(['x', 'X']);
    let digits = if extended {
        &text[..text.len() - 1]
    } else {
        text
    };
    let (digits, radix) = if let Some(d) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (d, 16)
    } else {
        (digits, radix)
    };
    if digits.is_empty() {
        bail!("empty CAN ID");
    }
    Ok((u32::from_str_radix(digits, radix)?, extended))
}

pub fn parse_date(text: &str) -> Option<[u16; 8]> {
    use chrono::{Datelike, NaiveDateTime, Timelike};
    let mut tokens: Vec<_> = text.split_whitespace().collect();
    if tokens.len() < 5 {
        return None;
    }
    tokens[0] = match tokens[0] {
        "Mon" | "Mo" => "Mon",
        "Die" | "Di" => "Tue",
        "Mit" | "Mi" => "Wed",
        "Don" | "Do" => "Thu",
        "Fre" | "Fr" => "Fri",
        "Sam" | "Sa" => "Sat",
        "Son" | "So" => "Sun",
        other => other,
    };
    tokens[1] = match tokens[1] {
        "Mär" | "Mrz" => "Mar",
        "Mai" => "May",
        "Okt" => "Oct",
        "Dez" => "Dec",
        other => other,
    };
    let normalized = tokens.join(" ");
    let date = ["%a %b %e %I:%M:%S%.f %p %Y", "%a %b %e %H:%M:%S%.f %Y"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&normalized, format).ok())?;
    Some([
        date.year().try_into().ok()?,
        date.month() as u16,
        date.weekday().num_days_from_sunday() as u16,
        date.day() as u16,
        date.hour() as u16,
        date.minute() as u16,
        date.second() as u16,
        (date.nanosecond() / 1_000_000) as u16,
    ])
}
pub fn system_date(time: [u16; 8]) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDate::from_ymd_opt(time[0] as i32, time[1] as u32, time[3] as u32)?
        .and_hms_milli_opt(
            time[4] as u32,
            time[5] as u32,
            time[6] as u32,
            time[7] as u32,
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_time() {
        assert_eq!(seconds_ns("123.000000001").unwrap(), 123_000_000_001);
        assert!(seconds_ns("9223372037").is_err());
        for bad in ["NaN", "-1", "1e2", "1.0000000001"] {
            assert!(seconds_ns(bad).is_err());
        }
    }
    #[test]
    fn invariants() {
        for dlc in 0..16 {
            assert!(Frame::new(
                0,
                1,
                0x7ff,
                false,
                Direction::Rx,
                false,
                true,
                dlc,
                vec![0; FD_LENGTHS[dlc as usize]],
                true,
                true
            )
            .is_ok());
            assert!(Frame::new(
                0,
                1,
                1,
                false,
                Direction::Rx,
                false,
                false,
                dlc,
                vec![0; dlc.min(8) as usize],
                false,
                false
            )
            .is_ok());
        }
        assert!(Frame::new(
            0,
            1,
            0x800,
            false,
            Direction::Rx,
            false,
            false,
            0,
            vec![],
            false,
            false
        )
        .is_err());
        assert!(Frame::new(
            0,
            1,
            1,
            false,
            Direction::Rx,
            true,
            true,
            0,
            vec![],
            false,
            false
        )
        .is_err());
        assert!(Frame::new(
            0,
            1,
            1,
            false,
            Direction::Rx,
            false,
            false,
            1,
            vec![],
            false,
            false
        )
        .is_err());
    }
}
