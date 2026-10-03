//! Sparse raw-file replay anchors. SQLite stores coordinates and issues, not payloads.
use crate::{
    app::{Filter, InputArgs, Unsupported},
    core::*,
    formats,
    output::AtomicOutput,
    playback::Cancellation,
};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const INDEX_SCHEMA: i64 = 1;
pub const PARSER_ID: &str = "canlog-asc-blf-v2-index-v1";
const APPLICATION_ID: i64 = 0x43414e4c;

pub fn hash_file(path: &Path, cancel: &Cancellation) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        cancel.check()?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Processing {
    pub parser: String,
    pub max_line: usize,
    pub max_object: usize,
    pub max_container: usize,
    pub id_map_sha256: Option<String>,
}
impl Processing {
    pub fn of(args: &InputArgs, cancel: &Cancellation) -> Result<Self> {
        Ok(Self {
            parser: PARSER_ID.into(),
            max_line: args.max_line_bytes,
            max_object: args.max_object_bytes,
            max_container: args.max_container_bytes,
            id_map_sha256: args
                .id_map
                .as_ref()
                .map(|p| hash_file(p, cancel))
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: i64,
    pub source: PathBuf,
    pub source_sha256: String,
    pub size_bytes: u64,
    pub processing: Processing,
    pub metadata: Metadata,
    pub records: u64,
    pub frames: u64,
    pub issues: u64,
    pub chunks: u64,
    pub complete_scan: bool,
}

pub fn configure_reader(args: &InputArgs, cancel: &Cancellation) -> Result<Box<dyn LogReader>> {
    let filter = Filter::new(args.clone())?;
    let map = args
        .id_map
        .as_ref()
        .map(|p| crate::id_map::IdMap::load(p).map(Arc::new))
        .transpose()?;
    let mut reader = formats::open_cancellable_reader(
        &args.input,
        args.input_format,
        filter.limits(),
        cancel.clone(),
    )?;
    reader.configure(false, map)?;
    Ok(reader)
}

pub fn check_issue(args: &InputArgs, issue: &Issue) -> Result<()> {
    if issue.kind == IssueKind::CorruptedRegion {
        ensure!(
            args.recover,
            "corrupted record at {:?}: {} (use --recover)",
            issue.location,
            issue.message
        );
    } else {
        ensure!(
            args.unsupported == Unsupported::Skip,
            "{:?} at {:?}: {} (use --unsupported skip)",
            issue.kind,
            issue.location,
            issue.message
        );
    }
    Ok(())
}

/// Whole generation publication is atomic. A failed/cancelled build never
/// replaces the previous index or publishes incomplete coverage.
pub fn build(
    args: &InputArgs,
    destination: &Path,
    overwrite: bool,
    stride: u64,
    cancel: &Cancellation,
) -> Result<Manifest> {
    let _filter = Filter::new(args.clone())?;
    ensure!(
        args.input != "-" && !args.preserve_records,
        "index requires a file and semantic parsing"
    );
    ensure!(
        args.start.is_none()
            && args.end.is_none()
            && args.channel.is_empty()
            && args.id.is_empty()
            && args.id_kind.is_none()
            && args.direction.is_none()
            && args.limit.is_none(),
        "index build requires a full scan without filters"
    );
    ensure!(
        (1..=1_000_000).contains(&stride),
        "stride must be 1..1000000"
    );
    let source = Path::new(&args.input).canonicalize()?;
    let source_hash = hash_file(&source, cancel)?;
    let processing = Processing::of(args, cancel)?;
    if let Some(map) = &args.id_map {
        protect(destination, map)?;
    }
    let atomic = AtomicOutput::new(destination, &args.input, overwrite)?;
    let mut conn = Connection::open(atomic.temporary_path())?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE;
        PRAGMA application_id=1128353356; PRAGMA user_version=1;
        CREATE TABLE manifest(id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
        CREATE TABLE chunks(sequence INTEGER PRIMARY KEY CHECK(sequence>=0), checkpoint TEXT NOT NULL,
            records INTEGER NOT NULL CHECK(records>0), min_ns INTEGER, max_ns INTEGER,
            unknown_time INTEGER NOT NULL CHECK(unknown_time IN (0,1)),
            CHECK((min_ns IS NULL AND max_ns IS NULL) OR (min_ns<=max_ns)));
        CREATE INDEX chunks_time ON chunks(min_ns,max_ns);
        CREATE TABLE issues(sequence INTEGER PRIMARY KEY CHECK(sequence>=0), timestamp_ns INTEGER,
            channel INTEGER, kind TEXT NOT NULL, json TEXT NOT NULL);
        CREATE INDEX issues_time ON issues(timestamp_ns);")?;
    let mut reader = configure_reader(args, cancel)?;
    // Fail before scanning unsupported index formats.
    reader.checkpoint()?;
    let mut manifest = Manifest {
        schema_version: INDEX_SCHEMA,
        source,
        source_sha256: source_hash,
        size_bytes: std::fs::metadata(&args.input)?.len(),
        processing,
        metadata: reader.metadata().clone(),
        records: 0,
        frames: 0,
        issues: 0,
        chunks: 0,
        complete_scan: false,
    };
    let mut eof = false;
    while !eof {
        cancel.check()?;
        let checkpoint = serde_json::to_string(&reader.checkpoint()?)?;
        let mut count = 0u64;
        let mut min: Option<i64> = None;
        let mut max: Option<i64> = None;
        let mut unknown = false;
        let tx = conn.transaction()?;
        for _ in 0..stride {
            cancel.check()?;
            let Some(item) = reader.next_item()? else {
                eof = true;
                break;
            };
            let time = match item {
                ReadItem::Frame(record) => {
                    manifest.frames += 1;
                    Some(record.frame.timestamp_ns())
                }
                ReadItem::Issue(issue) => {
                    check_issue(args, &issue)?;
                    tx.execute(
                        "INSERT INTO issues VALUES (?1,?2,?3,?4,?5)",
                        params![
                            i64::try_from(manifest.records)?,
                            issue.timestamp_ns,
                            issue.channel,
                            format!("{:?}", issue.kind),
                            serde_json::to_string(&issue)?
                        ],
                    )?;
                    manifest.issues += 1;
                    issue.timestamp_ns
                }
            };
            if let Some(t) = time {
                min = Some(min.map_or(t, |m| m.min(t)));
                max = Some(max.map_or(t, |m| m.max(t)));
            } else {
                unknown = true;
            }
            manifest.records += 1;
            count += 1;
        }
        if count > 0 {
            tx.execute(
                "INSERT INTO chunks VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    i64::try_from(manifest.chunks)?,
                    checkpoint,
                    i64::try_from(count)?,
                    min,
                    max,
                    unknown
                ],
            )?;
            manifest.chunks += 1;
        }
        tx.commit()?;
    }
    manifest.metadata = reader.metadata().clone();
    ensure!(
        manifest.source_sha256 == hash_file(&manifest.source, cancel)?,
        "source changed during index build"
    );
    ensure!(
        manifest.processing == Processing::of(args, cancel)?,
        "ID mapping changed during index build"
    );
    manifest.complete_scan = true;
    conn.execute(
        "INSERT INTO manifest VALUES (1,?1)",
        [serde_json::to_string(&manifest)?],
    )?;
    conn.close().map_err(|(_, error)| error)?;
    cancel.check()?;
    atomic.publish(true)?;
    Ok(manifest)
}

pub fn protect(destination: &Path, source: &Path) -> Result<()> {
    if destination.exists() {
        ensure!(
            !same_file::is_same_file(destination, source)?,
            "output would replace a source or database"
        );
    }
    Ok(())
}

fn open(path: &Path) -> Result<(Connection, Manifest)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    ensure!(
        conn.pragma_query_value::<i64, _>(None, "application_id", |r| r.get(0))? == APPLICATION_ID,
        "not a canlog index"
    );
    ensure!(
        conn.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))? == INDEX_SCHEMA,
        "unsupported index schema; rebuild with this binary (existing index is preserved)"
    );
    let json: String = conn.query_row("SELECT json FROM manifest WHERE id=1", [], |r| r.get(0))?;
    let manifest: Manifest = serde_json::from_str(&json)?;
    ensure!(
        manifest.schema_version == INDEX_SCHEMA && manifest.complete_scan,
        "incomplete index generation"
    );
    Ok((conn, manifest))
}
pub fn info(path: &Path) -> Result<Manifest> {
    Ok(open(path)?.1)
}

pub struct IndexedReader {
    conn: Connection,
    args: InputArgs,
    cancel: Cancellation,
    pub manifest: Manifest,
    current: Option<Box<dyn LogReader>>,
    remaining: u64,
    next_sequence: u64,
    pub chunks_read: u64,
}
impl IndexedReader {
    pub fn new(path: &Path, args: &InputArgs, cancel: &Cancellation) -> Result<Self> {
        Filter::new(args.clone())?;
        ensure!(
            !args.preserve_records && args.input != "-",
            "indexed queries require semantic file input"
        );
        let (conn, manifest) = open(path)?;
        ensure!(
            Path::new(&args.input).canonicalize()? == manifest.source,
            "index belongs to a different source"
        );
        ensure!(
            manifest.processing == Processing::of(args, cancel)?,
            "parser limits or ID mapping differ; rebuild index or use matching options"
        );
        ensure!(
            std::fs::metadata(&args.input)?.len() == manifest.size_bytes
                && hash_file(Path::new(&args.input), cancel)? == manifest.source_sha256,
            "stale index: source content changed; rebuild index"
        );
        Ok(Self {
            conn,
            args: args.clone(),
            cancel: cancel.clone(),
            manifest,
            current: None,
            remaining: 0,
            next_sequence: 0,
            chunks_read: 0,
        })
    }
    pub fn verify_source(&self) -> Result<()> {
        ensure!(
            hash_file(Path::new(&self.args.input), &self.cancel)? == self.manifest.source_sha256,
            "source changed during indexed query"
        );
        ensure!(
            Processing::of(&self.args, &self.cancel)? == self.manifest.processing,
            "ID mapping changed during indexed query"
        );
        Ok(())
    }
}
impl LogReader for IndexedReader {
    fn metadata(&self) -> &Metadata {
        &self.manifest.metadata
    }
    fn next_item(&mut self) -> Result<Option<ReadItem>> {
        self.cancel.check()?;
        if self.remaining == 0 {
            let row = self
                .conn
                .query_row(
                    "SELECT sequence,checkpoint,records FROM chunks WHERE sequence>=?1 AND
                (unknown_time=1 OR (max_ns>=?2 AND min_ns<?3)) ORDER BY sequence LIMIT 1",
                    params![
                        i64::try_from(self.next_sequence)?,
                        self.args.start.unwrap_or(0),
                        self.args.end.unwrap_or(i64::MAX)
                    ],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((sequence, checkpoint, count)) = row else {
                return Ok(None);
            };
            // Adjacent chunks share the running decoder, avoiding repeated
            // inflation and prefix replay of the same BLF container.
            if self.current.is_none() || u64::try_from(sequence)? != self.next_sequence {
                let checkpoint: ReaderCheckpoint =
                    serde_json::from_str(&checkpoint).context("reading replay anchor")?;
                let mut reader = configure_reader(&self.args, &self.cancel)?;
                reader.restore(&checkpoint, &self.cancel)?;
                self.current = Some(reader);
            }
            self.remaining = u64::try_from(count)?;
            ensure!(self.remaining > 0, "empty index chunk");
            self.next_sequence = u64::try_from(sequence)? + 1;
            self.chunks_read += 1;
        }
        self.remaining -= 1;
        let item = self
            .current
            .as_mut()
            .context("missing indexed reader")?
            .next_item()?;
        if item.is_none() {
            bail!("index chunk extends beyond source EOF");
        }
        Ok(item)
    }
}
