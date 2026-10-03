//! User manifest owns configuration. SQLite holds regenerable index/catalog/cache.
use crate::{
    analysis::{self, AnalysisReport, StreamOutput},
    app::{InputArgs, Unsupported},
    cache,
    dbc::Decoder,
    index,
    output::{self, AtomicOutput},
    playback::Cancellation,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MANIFEST: &str = "workspace.json";
const DATABASE: &str = "workspace.db";
const MAX_MANIFEST: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub channel: u16,
    pub path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    pub name: String,
    pub path: PathBuf,
    pub id_map: Option<PathBuf>,
    pub bindings: Vec<Binding>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub name: String,
    pub revision: u64,
    pub cache_limit_bytes: u64,
    pub logs: Vec<Log>,
}

pub fn open_database(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    ensure!(
        connection.pragma_query_value::<i64, _>(None, "application_id", |r| r.get(0))?
            == cache::APPLICATION_ID,
        "not a canlog workspace database"
    );
    ensure!(
        connection.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))?
            == cache::SCHEMA,
        "unsupported workspace schema; existing data preserved"
    );
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

pub fn create(root: &Path, name: Option<&str>, cancel: &Cancellation) -> Result<Manifest> {
    cancel.check()?;
    fs::create_dir(root).context("workspace must be a new directory")?;
    let result = (|| {
        let manifest = Manifest {
            schema_version: 1,
            name: name
                .unwrap_or_else(|| {
                    root.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("CAN workspace")
                })
                .into(),
            revision: 1,
            cache_limit_bytes: 512 * 1024 * 1024,
            logs: vec![],
        };
        let connection = Connection::open(root.join(DATABASE))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
            PRAGMA application_id=1128355667; PRAGMA user_version=1;
            CREATE TABLE source_revisions(log_name TEXT NOT NULL,source_sha256 TEXT NOT NULL,processing_key TEXT NOT NULL,
                index_path TEXT NOT NULL,metadata_json TEXT NOT NULL,PRIMARY KEY(log_name,source_sha256,processing_key));
            CREATE TABLE cache_generations(id INTEGER PRIMARY KEY,semantic_key TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('building','complete')),
                quality TEXT NOT NULL CHECK(quality IN ('unknown','clean','degraded')),bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes>=0),
                last_used INTEGER NOT NULL,report_json TEXT, CHECK(state='building' OR report_json IS NOT NULL));
            CREATE INDEX generations_key ON cache_generations(semantic_key,state);
            CREATE TABLE signal_entries(generation_id INTEGER NOT NULL REFERENCES cache_generations(id) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),typed_json TEXT NOT NULL,checksum TEXT NOT NULL CHECK(length(checksum)=64),PRIMARY KEY(generation_id,ordinal));")?;
        connection.close().map_err(|(_, e)| e)?;
        fs::create_dir(root.join("indexes"))?;
        output::write_report(&root.join(MANIFEST), &manifest, false)?;
        Ok(manifest)
    })();
    // Do not recursively remove a workspace after a partial initialization error.
    // Existing paths are never accepted by create, and the diagnostic retains evidence.
    result
}

pub struct Workspace {
    pub root: PathBuf,
    pub manifest: Manifest,
}
impl Workspace {
    pub fn load(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let mut bytes = vec![];
        fs::File::open(root.join(MANIFEST))?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_MANIFEST,
            "workspace manifest exceeds 1 MiB"
        );
        let manifest: Manifest =
            serde_json::from_slice(&bytes).context("invalid workspace manifest")?;
        ensure!(
            manifest.schema_version == 1 && manifest.revision > 0,
            "unsupported workspace manifest version"
        );
        ensure!(
            manifest.cache_limit_bytes > 0 && manifest.cache_limit_bytes <= 16 * 1024 * 1024 * 1024,
            "invalid workspace cache quota"
        );
        ensure!(manifest.logs.len() <= 4096, "workspace log limit exceeded");
        let mut names = BTreeSet::new();
        for log in &manifest.logs {
            validate_name(&log.name)?;
            ensure!(names.insert(&log.name), "duplicate workspace log name");
            ensure!(
                log.bindings.len() <= 16 && log.bindings.iter().all(|b| b.channel > 0),
                "invalid workspace DBC bindings"
            );
        }
        open_database(&root.join(DATABASE))?;
        Ok(Self { root, manifest })
    }
    pub fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.into()
        } else {
            self.root.join(path)
        }
    }
    fn stored_path(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).into()
    }
    pub fn log(&self, name: &str) -> Result<&Log> {
        self.manifest
            .logs
            .iter()
            .find(|l| l.name == name)
            .with_context(|| format!("workspace log not found: {name}"))
    }
    fn save(&mut self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.manifest.revision = self
            .manifest
            .revision
            .checked_add(1)
            .context("manifest revision overflow")?;
        let bytes = serde_json::to_vec_pretty(&self.manifest)?;
        ensure!(
            (bytes.len() as u64) < MAX_MANIFEST,
            "workspace manifest exceeds 1 MiB"
        );
        let atomic = AtomicOutput::new(&self.root.join(MANIFEST), "-", true)?;
        let mut file = atomic.file()?;
        file.write_all(&bytes)?;
        writeln!(file)?;
        drop(file);
        atomic.publish(true)
    }
    pub fn protected_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.root.join(MANIFEST),
            self.root.join(DATABASE),
            self.root.join("workspace.db-wal"),
            self.root.join("workspace.db-shm"),
        ];
        for log in &self.manifest.logs {
            paths.push(self.resolve(&log.path));
            if let Some(map) = &log.id_map {
                paths.push(self.resolve(map));
            }
            paths.extend(log.bindings.iter().map(|b| self.resolve(&b.path)));
        }
        paths
    }
    pub fn protect_output(&self, destination: &Path) -> Result<()> {
        for path in self.protected_paths() {
            if path.exists() {
                index::protect(destination, &path)?;
            }
            output::ensure_distinct_paths(destination, &path)?;
        }
        // Managed indexes/cache remain derived artifacts, never stream destinations.
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        ensure!(
            !parent.starts_with(self.root.join("indexes").canonicalize()?),
            "output cannot replace a managed index"
        );
        Ok(())
    }
    pub fn input(&self, args: &InputArgs) -> Result<InputArgs> {
        let log = self.log(&args.input)?;
        let mut args = args.clone();
        args.input = self.resolve(&log.path).to_string_lossy().into_owned();
        if args.id_map.is_none() {
            args.id_map = log.id_map.as_ref().map(|p| self.resolve(p));
        }
        Ok(args)
    }
    pub fn bindings(&self, name: &str, overrides: &[String]) -> Result<Vec<String>> {
        let log = self.log(name)?;
        let mut by_channel: BTreeMap<u16, Vec<PathBuf>> = BTreeMap::new();
        for binding in &log.bindings {
            by_channel
                .entry(binding.channel)
                .or_default()
                .push(self.resolve(&binding.path));
        }
        let mut override_channels = BTreeSet::new();
        for binding in overrides {
            let (channel, path) = binding
                .split_once('=')
                .context("DBC binding must be CHANNEL=PATH")?;
            let channel: u16 = channel.parse()?;
            ensure!(channel > 0, "DBC channel must be positive");
            if override_channels.insert(channel) {
                by_channel.insert(channel, vec![]);
            }
            by_channel
                .entry(channel)
                .or_default()
                .push(PathBuf::from(path).canonicalize()?);
        }
        Ok(by_channel
            .into_iter()
            .flat_map(|(channel, paths)| {
                paths
                    .into_iter()
                    .map(move |p| format!("{channel}={}", p.display()))
            })
            .collect())
    }
    fn index_path(&self, args: &InputArgs, cancel: &Cancellation) -> Result<PathBuf> {
        let key = cache::digest(&(
            Path::new(&args.input).canonicalize()?,
            index::hash_file(Path::new(&args.input), cancel)?,
            index::Processing::of(args, cancel)?,
        ))?;
        Ok(self.root.join("indexes").join(format!("{key}.sqlite")))
    }
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')),
        "log name must contain 1..128 letters, digits, underscore, dash or dot"
    );
    Ok(())
}

pub fn add(
    root: &Path,
    input: &Path,
    name: &str,
    id_map: Option<&Path>,
    cancel: &Cancellation,
) -> Result<Manifest> {
    validate_name(name)?;
    let connection = open_database(&root.join(DATABASE))?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let mut workspace = Workspace::load(root)?;
    ensure!(
        workspace.manifest.logs.len() < 4096,
        "workspace log limit exceeded"
    );
    ensure!(
        !workspace.manifest.logs.iter().any(|l| l.name == name),
        "workspace log name already exists"
    );
    let source = input.canonicalize()?;
    for path in workspace.protected_paths().iter().filter(|p| p.exists()) {
        ensure!(
            !same_file::is_same_file(&source, path)?,
            "source is already registered or is workspace data"
        );
    }
    let mut reader = crate::formats::open_reader(
        &source.to_string_lossy(),
        None,
        crate::core::Limits::default(),
    )?;
    reader.checkpoint()?;
    let map = id_map
        .map(|p| -> Result<PathBuf> {
            let p = p.canonicalize()?;
            crate::id_map::IdMap::load(&p)?;
            Ok(workspace.stored_path(&p))
        })
        .transpose()?;
    workspace.manifest.logs.push(Log {
        name: name.into(),
        path: workspace.stored_path(&source),
        id_map: map,
        bindings: vec![],
    });
    workspace.save(cancel)?;
    connection.execute_batch("COMMIT")?;
    Ok(workspace.manifest)
}

pub fn bind(
    root: &Path,
    name: &str,
    bindings: &[String],
    cancel: &Cancellation,
) -> Result<Manifest> {
    let connection = open_database(&root.join(DATABASE))?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let mut workspace = Workspace::load(root)?;
    workspace.log(name)?;
    let decoder = Decoder::load(bindings, cancel)?;
    let bindings = decoder
        .assignments
        .iter()
        .map(|a| Binding {
            channel: a.channel,
            path: workspace.stored_path(&a.path),
        })
        .collect();
    workspace
        .manifest
        .logs
        .iter_mut()
        .find(|l| l.name == name)
        .expect("log checked")
        .bindings = bindings;
    decoder.verify_databases(cancel)?;
    workspace.save(cancel)?;
    connection.execute_batch("COMMIT")?;
    Ok(workspace.manifest)
}

pub fn build_index(
    root: &Path,
    args: &InputArgs,
    stride: u64,
    cancel: &Cancellation,
) -> Result<index::Manifest> {
    crate::app::Filter::new(args.clone())?;
    ensure!(
        (1..=1_000_000).contains(&stride),
        "stride must be 1..1000000"
    );
    ensure!(
        !args.preserve_records
            && args.start.is_none()
            && args.end.is_none()
            && args.channel.is_empty()
            && args.id.is_empty()
            && args.id_kind.is_none()
            && args.direction.is_none()
            && args.limit.is_none(),
        "workspace index requires a full scan without filters"
    );
    let workspace = Workspace::load(root)?;
    let name = args.input.clone();
    let args = workspace.input(args)?;
    let path = workspace.index_path(&args, cancel)?;
    let manifest = if path.exists() {
        let manifest = index::info(&path)?;
        ensure!(
            manifest.source == Path::new(&args.input).canonicalize()?
                && manifest.source_sha256 == index::hash_file(Path::new(&args.input), cancel)?
                && manifest.processing == index::Processing::of(&args, cancel)?,
            "managed index identity differs; remove damaged derived index and rebuild"
        );
        // A strict build cannot reuse a previously degraded generation.
        if manifest.issues > 0 && (args.unsupported == Unsupported::Error || !args.recover) {
            let mut reader = index::configure_reader(&args, cancel)?;
            while let Some(item) = reader.next_item()? {
                cancel.check()?;
                if let crate::core::ReadItem::Issue(i) = item {
                    index::check_issue(&args, &i)?;
                }
            }
        }
        manifest
    } else {
        index::build(&args, &path, false, stride, cancel)?
    };
    ensure!(
        manifest.source_sha256 == index::hash_file(Path::new(&args.input), cancel)?
            && manifest.processing == index::Processing::of(&args, cancel)?,
        "source or parser configuration changed during index verification"
    );
    let connection = open_database(&workspace.root.join(DATABASE))?;
    connection.execute(
        "INSERT OR REPLACE INTO source_revisions VALUES (?1,?2,?3,?4,?5)",
        params![
            name,
            manifest.source_sha256,
            cache::digest(&manifest.processing)?,
            path.to_string_lossy(),
            serde_json::to_string(&manifest)?
        ],
    )?;
    Ok(manifest)
}

pub fn stream(
    root: &Path,
    args: &InputArgs,
    overrides: Option<&[String]>,
    output: &StreamOutput,
    no_cache: bool,
    cancel: &Cancellation,
) -> (AnalysisReport, Result<()>) {
    let setup = (|| -> Result<_> {
        let workspace = Workspace::load(root)?;
        if let Some(out) = &output.output {
            workspace.protect_output(out)?;
        }
        if let Some(report) = &args.report {
            workspace.protect_output(report)?;
        }
        let bindings = overrides
            .map(|b| workspace.bindings(&args.input, b))
            .transpose()?;
        let args = workspace.input(args)?;
        let index = workspace.index_path(&args, cancel)?;
        let index = index.exists().then_some(index);
        let cache = (!no_cache && bindings.is_some()).then(|| analysis::CacheOptions {
            database: workspace.root.join(DATABASE),
            quota: workspace.manifest.cache_limit_bytes,
        });
        Ok((args, index, bindings, cache))
    })();
    match setup {
        Ok((args, index, bindings, cache)) => analysis::stream_cached(
            &args,
            index.as_deref(),
            bindings.as_deref(),
            output,
            cancel,
            cache.as_ref(),
        ),
        Err(error) => (
            AnalysisReport {
                schema_version: 1,
                status: if error.is::<crate::playback::Cancelled>() {
                    "cancelled"
                } else {
                    "failed"
                }
                .into(),
                error: Some(format!("{error:#}")),
                ..Default::default()
            },
            Err(error),
        ),
    }
}

pub fn cache_info(root: &Path) -> Result<serde_json::Value> {
    let workspace = Workspace::load(root)?;
    let connection = open_database(&workspace.root.join(DATABASE))?;
    let generations: i64 = connection.query_row(
        "SELECT count(*) FROM cache_generations WHERE state='complete'",
        [],
        |r| r.get(0),
    )?;
    let building: i64 = connection.query_row(
        "SELECT count(*) FROM cache_generations WHERE state='building'",
        [],
        |r| r.get(0),
    )?;
    let rows:i64=connection.query_row("SELECT count(*) FROM signal_entries e JOIN cache_generations g ON g.id=e.generation_id WHERE g.state='complete'",[],|r|r.get(0))?;
    let bytes: i64 = connection.query_row(
        "SELECT coalesce(sum(bytes),0) FROM cache_generations",
        [],
        |r| r.get(0),
    )?;
    Ok(
        serde_json::json!({"generations":generations,"building_generations":building,"rows":rows,"payload_bytes":bytes,"quota_bytes":workspace.manifest.cache_limit_bytes}),
    )
}
pub fn clear_cache(root: &Path) -> Result<()> {
    let workspace = Workspace::load(root)?;
    let connection = open_database(&workspace.root.join(DATABASE))?;
    connection.execute("DELETE FROM cache_generations", [])?;
    Ok(())
}
pub fn set_quota(root: &Path, mib: u64, cancel: &Cancellation) -> Result<Manifest> {
    ensure!(
        (1..=16384).contains(&mib),
        "cache quota must be 1..16384 MiB"
    );
    let connection = open_database(&root.join(DATABASE))?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let mut workspace = Workspace::load(root)?;
    workspace.manifest.cache_limit_bytes = mib * 1024 * 1024;
    // Active builders are never removed by quota changes. Completed generations
    // are evicted atomically, before subsequent commands can reuse them.
    while connection.query_row(
        "SELECT coalesce(sum(bytes),0) FROM cache_generations",
        [],
        |r| r.get::<_, i64>(0),
    )? > i64::try_from(workspace.manifest.cache_limit_bytes)?
    {
        let removed = connection.execute("DELETE FROM cache_generations WHERE id=(SELECT id FROM cache_generations WHERE state='complete' ORDER BY last_used,id LIMIT 1)", [])?;
        if removed == 0 {
            break;
        }
    }
    workspace.save(cancel)?;
    connection.execute_batch("COMMIT")?;
    Ok(workspace.manifest)
}
