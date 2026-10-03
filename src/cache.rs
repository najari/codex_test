//! Stateless DBC row cache. Building generations are never lookup candidates.
use crate::{
    app::InputArgs,
    dbc::{Assignment, CachedSignals, ADAPTER_ID, ENGINE_REVISION},
    index::Processing,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

pub const SCHEMA: i64 = 1;
pub const APPLICATION_ID: i64 = 0x43415753;
const MAX_ROW: usize = 2 * 1024 * 1024;
const BATCH_BYTES: usize = 2 * 1024 * 1024;
const BATCH_ROWS: usize = 128;

pub fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub fn semantic_key(
    args: &InputArgs,
    hash: &str,
    processing: &Processing,
    assignments: &[Assignment],
) -> Result<String> {
    let mut bindings: Vec<_> = assignments
        .iter()
        .map(|a| (a.channel, a.sha256.as_str()))
        .collect();
    bindings.sort_unstable();
    digest(
        &serde_json::json!({"schema":SCHEMA,"source":Path::new(&args.input).canonicalize()?,"source_sha256":hash,
        "processing":processing,"engine":ENGINE_REVISION,"adapter":ADAPTER_ID,"assignments":bindings,
        "unsupported":format!("{:?}",args.unsupported),"recover":args.recover,"timeline":"source-local-original-order"}),
    )
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct CacheStats {
    pub key: String,
    pub hits: u64,
    pub misses: u64,
    pub rows_written: u64,
    pub rows_evicted: u64,
    pub quota_skips: u64,
    pub generation_completed: bool,
}

pub struct Session {
    connection: Connection,
    run: Option<i64>,
    quota: u64,
    pending: Vec<(i64, String)>,
    pending_bytes: usize,
    touched: BTreeSet<i64>,
    pub stats: CacheStats,
}
impl Session {
    pub fn open(path: &Path, key: String, quota: u64) -> Result<Self> {
        let connection = crate::workspace::open_database(path)?;
        ensure!(
            quota > 0 && quota <= 16 * 1024 * 1024 * 1024,
            "cache quota must be 1 byte..16 GiB"
        );
        Ok(Self {
            connection,
            run: None,
            quota,
            pending: vec![],
            pending_bytes: 0,
            touched: BTreeSet::new(),
            stats: CacheStats {
                key,
                ..Default::default()
            },
        })
    }
    pub fn lookup(&mut self, ordinal: u64) -> Result<Option<CachedSignals>> {
        let row = self.connection.query_row("SELECT e.typed_json,g.id,e.checksum FROM signal_entries e JOIN cache_generations g ON g.id=e.generation_id
            WHERE g.semantic_key=?1 AND g.state='complete' AND e.ordinal=?2 ORDER BY g.id DESC LIMIT 1",
            params![self.stats.key,i64::try_from(ordinal)?], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?))).optional()?;
        let Some((json, generation, checksum)) = row else {
            self.stats.misses += 1;
            return Ok(None);
        };
        ensure!(
            json.len() <= MAX_ROW,
            "cache row exceeds resource limit; clear cache"
        );
        ensure!(
            format!("{:x}", Sha256::digest(json.as_bytes())) == checksum,
            "cache checksum mismatch; clear cache"
        );
        let payload =
            serde_json::from_str(&json).context("invalid cached signal row; clear cache")?;
        if self.touched.insert(generation) {
            self.connection.execute(
                "UPDATE cache_generations SET last_used=unixepoch() WHERE id=?1",
                [generation],
            )?;
        }
        self.stats.hits += 1;
        Ok(Some(payload))
    }
    pub fn store(&mut self, ordinal: u64, payload: &CachedSignals) -> Result<()> {
        let json = serde_json::to_string(payload)?;
        if json.len() > MAX_ROW || json.len() as u64 > self.quota {
            self.stats.quota_skips += 1;
            return Ok(());
        }
        if self.pending_bytes + json.len() > BATCH_BYTES {
            self.flush()?;
        }
        self.pending_bytes += json.len();
        self.pending.push((i64::try_from(ordinal)?, json));
        if self.pending.len() >= BATCH_ROWS {
            self.flush()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run = if let Some(run) = self.run {
            run
        } else {
            tx.execute("INSERT INTO cache_generations(semantic_key,state,quality,last_used) VALUES (?1,'building','unknown',unixepoch())",[&self.stats.key])?;
            let run = tx.last_insert_rowid();
            self.run = Some(run);
            run
        };
        let mut rows_written = 0;
        let mut rows_evicted = 0;
        let mut quota_skips = 0;
        for (ordinal, json) in &self.pending {
            let mut size: i64 = tx.query_row(
                "SELECT coalesce(sum(bytes),0) FROM cache_generations",
                [],
                |r| r.get(0),
            )?;
            while size as u64 + json.len() as u64 > self.quota {
                let oldest = tx.query_row("SELECT id,bytes FROM cache_generations WHERE state='complete' ORDER BY last_used,id LIMIT 1",[],
                    |r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).optional()?;
                let Some((id, bytes)) = oldest else {
                    break;
                };
                let rows: i64 = tx.query_row(
                    "SELECT count(*) FROM signal_entries WHERE generation_id=?1",
                    [id],
                    |r| r.get(0),
                )?;
                tx.execute("DELETE FROM cache_generations WHERE id=?1", [id])?;
                rows_evicted += u64::try_from(rows)?;
                size -= bytes;
            }
            if size as u64 + json.len() as u64 > self.quota {
                quota_skips += 1;
                continue;
            }
            ensure!(
                tx.execute(
                    "UPDATE cache_generations SET bytes=bytes+?1 WHERE id=?2 AND state='building'",
                    params![i64::try_from(json.len())?, run]
                )? == 1,
                "cache generation was removed during decoding"
            );
            tx.execute(
                "INSERT INTO signal_entries VALUES (?1,?2,?3,?4)",
                params![
                    run,
                    ordinal,
                    json,
                    format!("{:x}", Sha256::digest(json.as_bytes()))
                ],
            )?;
            rows_written += 1;
        }
        tx.commit()?;
        self.stats.rows_written += rows_written;
        self.stats.rows_evicted += rows_evicted;
        self.stats.quota_skips += quota_skips;
        self.pending.clear();
        self.pending_bytes = 0;
        Ok(())
    }
    /// Only called after both original file and DBC identities are reverified.
    pub fn complete(&mut self, report: &crate::analysis::AnalysisReport) -> Result<()> {
        self.flush()?;
        if let Some(run) = self.run {
            let degraded = report.issues_selected > 0
                || report
                    .decode_counts
                    .iter()
                    .any(|(k, v)| *v > 0 && !matches!(k.as_str(), "decoded" | "remote"));
            let quality = if !report.selection_complete {
                "unknown"
            } else if degraded {
                "degraded"
            } else {
                "clean"
            };
            let mut saved_report = serde_json::to_value(report)?;
            saved_report["status"] = if degraded { "partial" } else { "complete" }.into();
            ensure!(self.connection.execute("UPDATE cache_generations SET state='complete',quality=?1,report_json=?2 WHERE id=?3 AND state='building'",
                params![quality,serde_json::to_string(&saved_report)?,run])?==1,"cache generation no longer exists");
            self.run = None;
            self.stats.generation_completed = true;
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // Abrupt termination can leave building rows; lookup always excludes them.
        if let Some(run) = self.run {
            let _ = self.connection.execute(
                "DELETE FROM cache_generations WHERE id=?1 AND state='building'",
                [run],
            );
        }
    }
}
