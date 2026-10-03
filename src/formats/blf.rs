use crate::core::*;
use anyhow::{anyhow, bail, ensure, Context, Result};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
};

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(b[i..i + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}
fn put16(b: &mut [u8], i: usize, v: u16) {
    b[i..i + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], i: usize, v: u32) {
    b[i..i + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], i: usize, v: u64) {
    b[i..i + 8].copy_from_slice(&v.to_le_bytes());
}

pub struct BlfReader {
    input: BufReader<File>,
    metadata: Metadata,
    limits: Limits,
    buffer: Vec<u8>,
    pos: usize,
    eof: bool,
    container: u64,
    ordinal: u64,
    padding_seen: usize,
    preserve: bool,
}
impl BlfReader {
    pub fn new(mut input: BufReader<File>, source: &str, limits: Limits) -> Result<Self> {
        let mut header = [0; 80];
        input
            .read_exact(&mut header)
            .context("truncated BLF header")?;
        ensure!(&header[..4] == b"LOGG", "missing BLF LOGG header");
        let size = u32_at(&header, 4) as usize;
        ensure!(
            (80..=limits.max_object).contains(&size),
            "invalid BLF file header size"
        );
        let actual_size = input.get_ref().metadata()?.len();
        ensure!(
            u64_at(&header, 16) == actual_size,
            "BLF file size mismatch (truncated or unfinished file)"
        );
        input.seek(SeekFrom::Start(size as u64))?;
        let mut start = [0; 8];
        let mut stop = [0; 8];
        for i in 0..8 {
            start[i] = u16_at(&header, 40 + 2 * i);
            stop[i] = u16_at(&header, 56 + 2 * i);
        }
        let metadata = Metadata {
            format: "blf".into(),
            profile: "vector-blf-can-v1-v2".into(),
            source: source.into(),
            date_text: system_date(start)
                .map(|dt| dt.format("%a %b %e %H:%M:%S%.3f %Y").to_string()),
            declared_objects: Some(u32_at(&header, 32) as u64),
            blf_start: Some(start),
            blf_stop: Some(stop),
            time_mode: Some("relative-to-header-origin".into()),
            ..Default::default()
        };
        Ok(Self {
            input,
            metadata,
            limits,
            buffer: Vec::new(),
            pos: 0,
            eof: false,
            container: 0,
            ordinal: 0,
            padding_seen: 0,
            preserve: false,
        })
    }
    fn container(&mut self) -> Result<bool> {
        let mut head = [0; 16];
        if self.input.read(&mut head[..1])? == 0 {
            self.eof = true;
            return Ok(false);
        }
        self.input
            .read_exact(&mut head[1..])
            .context("truncated outer BLF object header")?;
        ensure!(&head[..4] == b"LOBJ", "invalid outer BLF object signature");
        let hs = u16_at(&head, 4) as usize;
        let size = u32_at(&head, 8) as usize;
        ensure!(
            hs >= 16 && size >= hs && size <= self.limits.max_container,
            "outer BLF object exceeds resource limit or has invalid size"
        );
        let mut body = vec![0; size - 16];
        self.input
            .read_exact(&mut body)
            .context("truncated BLF container")?;
        let mut padding = [0; 3];
        self.input.read_exact(&mut padding[..size % 4])?;
        ensure!(
            u32_at(&head, 12) == 10,
            "unsupported outer BLF object type {}",
            u32_at(&head, 12)
        );
        let extra = hs - 16;
        ensure!(body.len() >= extra + 16, "truncated container header");
        let body = &body[extra..];
        let declared = u32_at(body, 8) as usize;
        ensure!(
            declared <= self.limits.max_container,
            "declared decompressed container exceeds resource limit"
        );
        let data = match u16_at(body, 0) {
            0 => body[16..].to_vec(),
            2 => {
                let mut decoder = ZlibDecoder::new(&body[16..]);
                let mut decoded = Vec::new();
                (&mut decoder)
                    .take(self.limits.max_container as u64 + 1)
                    .read_to_end(&mut decoded)
                    .context("invalid zlib container")?;
                ensure!(
                    decoded.len() <= self.limits.max_container,
                    "actual decompressed container exceeds resource limit"
                );
                ensure!(
                    decoder.total_in() as usize == body.len() - 16,
                    "trailing bytes after BLF zlib stream"
                );
                decoded
            }
            method => bail!("unsupported BLF compression method {method}"),
        };
        ensure!(
            data.len() == declared,
            "BLF declared/actual decompressed size mismatch"
        );
        self.buffer.drain(..self.pos);
        self.pos = 0;
        ensure!(
            self.buffer.len() <= self.limits.max_object + 3,
            "BLF carry exceeds resource limit"
        );
        self.buffer.extend_from_slice(&data);
        self.container += 1;
        Ok(true)
    }
    fn decode(&mut self, object: &[u8], location: Location) -> Result<ReadItem> {
        let hs = u16_at(object, 4) as usize;
        let version = u16_at(object, 6);
        let typ = u32_at(object, 12);
        let min_header = match version {
            1 => 32,
            2 => 40,
            _ => 16,
        };
        ensure!(
            hs >= min_header && object.len() >= hs,
            "invalid object header size"
        );
        let timestamp = if matches!(version, 1 | 2) {
            let multiplier = match u32_at(object, 16) {
                1 => 10_000_u64,
                2 => 1,
                _ => 0,
            };
            if multiplier == 0 {
                None
            } else {
                let ns = u64_at(object, 24)
                    .checked_mul(multiplier)
                    .context("BLF timestamp overflow")?;
                Some(i64::try_from(ns).context("BLF timestamp outside i64 range")?)
            }
        } else {
            None
        };
        let issue = |kind: IssueKind, message: String, channel: Option<u16>| {
            ReadItem::Issue(Issue {
                kind,
                message,
                location: location.clone(),
                timestamp_ns: timestamp,
                channel,
                object_type: Some(typ),
                native: if self.preserve && kind != IssueKind::CorruptedRegion {
                    Some(NativeRecord::Blf {
                        timestamp_ns: timestamp,
                        object: object.to_vec(),
                    })
                } else {
                    None
                },
            })
        };
        if !matches!(version, 1 | 2) {
            return Ok(issue(
                IssueKind::UnsupportedRecord,
                format!("unsupported object header version {version}"),
                None,
            ));
        }
        if timestamp.is_none() && matches!(typ, 1 | 86 | 100 | 101 | 2 | 73 | 104) {
            return Ok(issue(
                IssueKind::UnsupportedRecord,
                "CAN object has unsupported timestamp unit; no time is inferred".into(),
                None,
            ));
        }
        let b = &object[hs..];
        let decoded = match typ {
            1 | 86 => {
                ensure!(b.len() >= 16, "truncated CAN_MESSAGE");
                let flags = b[2];
                let raw_id = u32_at(b, 4);
                ensure!(raw_id & 0x6000_0000 == 0, "CAN ID contains reserved bits");
                let remote = flags & 0x80 != 0;
                let len = if remote { 0 } else { b[3].min(8) as usize };
                if (typ == 86 || flags & !0x81 != 0)
                    && !self.metadata.notes.iter().any(|s| s == "frame annotations")
                {
                    self.metadata.notes.push("frame annotations".into());
                }
                Frame::new(
                    timestamp.unwrap(),
                    u16_at(b, 0),
                    raw_id & 0x1fff_ffff,
                    raw_id & 0x8000_0000 != 0,
                    if flags & 1 == 0 {
                        Direction::Rx
                    } else {
                        Direction::Tx
                    },
                    remote,
                    false,
                    b[3],
                    b[8..8 + len].to_vec(),
                    false,
                    false,
                )
            }
            100 => {
                ensure!(b.len() >= 84, "truncated CAN_FD_MESSAGE");
                let flags = b[2];
                let fdflags = b[13];
                let len = b[14] as usize;
                let raw_id = u32_at(b, 4);
                if (b[8..13].iter().any(|&v| v != 0) || flags & !0x81 != 0 || fdflags & !7 != 0)
                    && !self.metadata.notes.iter().any(|s| s == "frame annotations")
                {
                    self.metadata.notes.push("frame annotations".into());
                }
                ensure!(
                    len <= 64 && raw_id & 0x6000_0000 == 0,
                    "invalid CAN FD payload or ID"
                );
                Frame::new(
                    timestamp.unwrap(),
                    u16_at(b, 0),
                    raw_id & 0x1fff_ffff,
                    raw_id & 0x8000_0000 != 0,
                    if flags & 1 == 0 {
                        Direction::Rx
                    } else {
                        Direction::Tx
                    },
                    flags & 0x80 != 0,
                    fdflags & 1 != 0,
                    b[3],
                    b[20..20 + len].to_vec(),
                    fdflags & 2 != 0,
                    fdflags & 4 != 0,
                )
            }
            101 => {
                ensure!(b.len() >= 40, "truncated CAN_FD_MESSAGE_64");
                let len = b[2] as usize;
                let flags = u32_at(b, 12);
                let raw_id = u32_at(b, 4);
                let data_end = if b[35] == 0 {
                    b.len()
                } else {
                    (b[35] as usize)
                        .checked_sub(hs)
                        .context("invalid FD extDataOffset")?
                };
                if (b[3] != 0
                    || b[8..12].iter().any(|&v| v != 0)
                    || b[16..34].iter().any(|&v| v != 0)
                    || b[35..40].iter().any(|&v| v != 0)
                    || flags & !0x7010 != 0)
                    && !self.metadata.notes.iter().any(|s| s == "frame annotations")
                {
                    self.metadata.notes.push("frame annotations".into());
                }
                ensure!(
                    len <= 64 && data_end >= 40 + len && data_end <= b.len(),
                    "truncated FD64 data; padding is not inferred"
                );
                ensure!(
                    raw_id & 0x6000_0000 == 0 && b[34] <= 1,
                    "invalid FD64 ID/direction"
                );
                Frame::new(
                    timestamp.unwrap(),
                    b[0] as u16,
                    raw_id & 0x1fff_ffff,
                    raw_id & 0x8000_0000 != 0,
                    if b[34] == 0 {
                        Direction::Rx
                    } else {
                        Direction::Tx
                    },
                    flags & 0x10 != 0,
                    flags & 0x1000 != 0,
                    b[1],
                    b[40..40 + len].to_vec(),
                    flags & 0x2000 != 0,
                    flags & 0x4000 != 0,
                )
            }
            2 | 73 | 104 => {
                return Ok(issue(
                    IssueKind::CanError,
                    format!("CAN error object {typ}"),
                    if b.len() >= 2 {
                        Some(u16_at(b, 0))
                    } else {
                        None
                    },
                ))
            }
            _ => {
                return Ok(issue(
                    IssueKind::UnsupportedRecord,
                    format!("unsupported BLF object {typ}"),
                    None,
                ))
            }
        };
        match decoded {
            Ok(frame) => Ok(ReadItem::Frame(FrameRecord {
                frame,
                location,
                native: self.preserve.then(|| NativeRecord::Blf {
                    timestamp_ns: timestamp,
                    object: object.to_vec(),
                }),
            })),
            Err(e) => Ok(issue(IssueKind::CorruptedRegion, e.to_string(), None)),
        }
    }
}
impl LogReader for BlfReader {
    fn configure(
        &mut self,
        preserve: bool,
        id_map: Option<std::sync::Arc<crate::id_map::IdMap>>,
    ) -> Result<()> {
        ensure!(id_map.is_none(), "ID mapping requires ASC input");
        self.preserve = preserve;
        Ok(())
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn next_item(&mut self) -> Result<Option<ReadItem>> {
        loop {
            // Only zero padding at the validated object boundary is skipped.
            while self.pos < self.buffer.len() && self.buffer[self.pos] == 0 {
                let n = self.buffer[self.pos..]
                    .iter()
                    .take_while(|&&b| b == 0)
                    .count();
                ensure!(
                    self.ordinal > 0 && self.padding_seen + n <= 3,
                    "invalid BLF object padding"
                );
                self.padding_seen += n;
                self.pos += n;
            }
            if self.buffer.len() - self.pos < 16 {
                if !self.eof && self.container()? {
                    continue;
                }
                ensure!(self.buffer.len() == self.pos, "truncated object at EOF");
                return Ok(None);
            }
            let head = &self.buffer[self.pos..self.pos + 16];
            ensure!(
                &head[..4] == b"LOBJ",
                "invalid BLF inner object signature at container {} offset {}",
                self.container,
                self.pos
            );
            let hs = u16_at(head, 4) as usize;
            let size = u32_at(head, 8) as usize;
            ensure!(
                hs >= 16 && size >= hs && size <= self.limits.max_object,
                "invalid or oversized BLF inner object"
            );
            if self.buffer.len() - self.pos < size {
                ensure!(
                    !self.eof && self.container()?,
                    "truncated BLF object across containers"
                );
                continue;
            }
            let location = Location {
                source: self.metadata.source.clone(),
                ordinal: self.ordinal,
                container: Some(self.container),
                object_offset: Some(self.pos as u64),
                ..Default::default()
            };
            let object = self.buffer[self.pos..self.pos + size].to_vec();
            self.pos += size;
            self.ordinal += 1;
            self.padding_seen = 0;
            return self.decode(&object, location).map(Some);
        }
    }
}

pub struct BlfWriter {
    output: BufWriter<File>,
    buffer: Vec<u8>,
    count: u32,
    uncompressed: u64,
    start: [u16; 8],
    source_stop: Option<[u16; 8]>,
    last_ns: i64,
    finished: bool,
}
impl BlfWriter {
    pub fn new(file: File, metadata: &Metadata) -> Result<Self> {
        Self::with_preservation(file, metadata, false)
    }
    pub fn with_preservation(file: File, metadata: &Metadata, preserve: bool) -> Result<Self> {
        let mut output = BufWriter::new(file);
        output.write_all(&[0; 144])?;
        Ok(Self {
            output,
            buffer: Vec::with_capacity(128 * 1024),
            count: 0,
            uncompressed: 144,
            start: metadata.blf_start.unwrap_or([0; 8]),
            source_stop: if preserve { metadata.blf_stop } else { None },
            last_ns: 0,
            finished: false,
        })
    }
    fn flush_container(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&self.buffer)?;
        let data = encoder.finish()?;
        let size = 32 + data.len();
        let mut header = [0; 32];
        header[..4].copy_from_slice(b"LOBJ");
        put16(&mut header, 4, 16);
        put16(&mut header, 6, 1);
        put32(&mut header, 8, size as u32);
        put32(&mut header, 12, 10);
        put16(&mut header, 16, 2);
        put32(&mut header, 24, self.buffer.len() as u32);
        self.output.write_all(&header)?;
        self.output.write_all(&data)?;
        self.output.write_all(&[0; 3][..size % 4])?;
        self.uncompressed = self
            .uncompressed
            .checked_add(32 + self.buffer.len() as u64)
            .context("uncompressed size overflow")?;
        self.buffer.clear();
        Ok(())
    }
    fn append_object(&mut self, object: &[u8], timestamp: Option<i64>) -> Result<()> {
        for chunk in object.chunks(128 * 1024) {
            if self.buffer.len() + chunk.len() > 128 * 1024 {
                self.flush_container()?;
            }
            self.buffer.extend_from_slice(chunk);
        }
        let padding = object.len() % 4;
        if self.buffer.len() + padding > 128 * 1024 {
            self.flush_container()?;
        }
        self.buffer.extend_from_slice(&[0; 3][..padding]);
        if let Some(t) = timestamp {
            self.last_ns = self.last_ns.max(t);
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| anyhow!("BLF object count exceeds u32"))?;
        Ok(())
    }
}
impl LogWriter for BlfWriter {
    fn write_native(&mut self, record: &NativeRecord) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        let NativeRecord::Blf {
            timestamp_ns,
            object,
        } = record
        else {
            bail!("BLF writer requires BLF native record");
        };
        ensure!(
            object.len() >= 16 && object.len() <= 16777216 && &object[..4] == b"LOBJ",
            "invalid native BLF object"
        );
        let hs = u16_at(object, 4) as usize;
        ensure!(
            hs >= 16
                && hs <= object.len()
                && u32_at(object, 8) as usize == object.len()
                && u32_at(object, 12) != 10,
            "invalid native BLF object boundary or nested container"
        );
        ensure!(
            timestamp_ns.is_none_or(|t| t >= 0),
            "negative native BLF timestamp"
        );
        let version = u16_at(object, 6);
        let minimum = match version {
            1 => 32,
            2 => 40,
            _ => 16,
        };
        ensure!(hs >= minimum, "invalid native BLF header version/size");
        let decoded_time = if matches!(version, 1 | 2) {
            let unit = match u32_at(object, 16) {
                1 => 10_000_u64,
                2 => 1,
                _ => 0,
            };
            if unit == 0 {
                None
            } else {
                Some(
                    i64::try_from(
                        u64_at(object, 24)
                            .checked_mul(unit)
                            .context("native timestamp overflow")?,
                    )
                    .context("native timestamp outside i64")?,
                )
            }
        } else {
            None
        };
        ensure!(
            decoded_time == *timestamp_ns,
            "native timestamp metadata disagrees with BLF header"
        );
        self.append_object(object, *timestamp_ns)
    }
    fn write_frame(&mut self, f: &Frame) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        f.validate()?;
        ensure!(
            f.direction() != Direction::Unknown,
            "BLF cannot preserve unknown direction"
        );
        let len = if f.fd() { 84 } else { 16 };
        let mut object = vec![0; 32 + len];
        object[..4].copy_from_slice(b"LOBJ");
        put16(&mut object, 4, 32);
        put16(&mut object, 6, 1);
        put32(&mut object, 8, (32 + len) as u32);
        put32(&mut object, 12, if f.fd() { 100 } else { 1 });
        put32(&mut object, 16, 2);
        put64(&mut object, 24, f.timestamp_ns() as u64);
        let b = &mut object[32..];
        put16(b, 0, f.channel());
        b[2] = u8::from(f.direction() == Direction::Tx) | if f.remote() { 0x80 } else { 0 };
        b[3] = f.raw_dlc();
        put32(b, 4, f.id() | if f.extended() { 0x8000_0000 } else { 0 });
        let data_offset = if f.fd() {
            b[13] = 1 | if f.brs() { 2 } else { 0 } | if f.esi() { 4 } else { 0 };
            b[14] = f.data().len() as u8;
            20
        } else {
            8
        };
        b[data_offset..data_offset + f.data().len()].copy_from_slice(f.data());
        self.append_object(&object, Some(f.timestamp_ns()))
    }
    fn finish(&mut self) -> Result<()> {
        ensure!(!self.finished, "writer finalized");
        self.flush_container()?;
        self.output.flush()?;
        let size = self.output.stream_position()?;
        let mut header = [0; 144];
        header[..4].copy_from_slice(b"LOGG");
        put32(&mut header, 4, 144);
        header[8] = 5;
        header[12] = 2;
        header[13] = 6;
        header[14] = 8;
        header[15] = 1;
        put64(&mut header, 16, size);
        put64(&mut header, 24, self.uncompressed);
        put32(&mut header, 32, self.count);
        let stop = self.source_stop.unwrap_or_else(|| {
            system_date(self.start)
                .and_then(|dt| dt.checked_add_signed(chrono::TimeDelta::nanoseconds(self.last_ns)))
                .and_then(|dt| parse_date(&dt.format("%a %b %e %H:%M:%S%.3f %Y").to_string()))
                .unwrap_or([0; 8])
        });
        for (i, value) in stop.iter().enumerate() {
            put16(&mut header, 40 + 2 * i, self.start[i]);
            put16(&mut header, 56 + 2 * i, *value);
        }
        self.output.seek(SeekFrom::Start(0))?;
        self.output.write_all(&header)?;
        self.output.flush()?;
        self.finished = true;
        Ok(())
    }
}
