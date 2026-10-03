//! Adapter to the pinned candb-engine parser and compiled codec.
use crate::{core::*, index::hash_file, playback::Cancellation};
use anyhow::{anyhow, ensure, Context, Result};
use candb_engine::{
    analysis,
    codec::{CompiledMessage, Scratch},
    model::{Message, ValueType},
    parser::Document,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub const ENGINE_REVISION: &str = "da64ad9ccf10237fce0993d1b83f460416d3da53";
pub const ADAPTER_ID: &str = "canlog-dbc-signals-v2";
const MAX_DBC_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub channel: u16,
    pub path: PathBuf,
    pub sha256: String,
    pub messages: usize,
    pub warnings: Vec<Value>,
}
struct Definition {
    assignment: usize,
    message: Message,
    compiled: CompiledMessage,
    requires_fd: bool,
}
pub struct Decoder {
    pub assignments: Vec<Assignment>,
    definitions: BTreeMap<(u16, u32), Definition>,
    scratch: Scratch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RawValue {
    Signed(i64),
    Unsigned(u64),
    FloatBits(u64),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalSample {
    pub key: String,
    pub definition_ordinal: usize,
    pub name: String,
    pub status: String,
    pub raw: Option<RawValue>,
    pub raw_text: Option<String>,
    pub physical: Option<f64>,
    pub unit: String,
    pub description: String,
}
#[derive(Debug, Serialize)]
pub struct DecodedFrame {
    pub schema_version: u32,
    pub kind: &'static str,
    pub record: FrameRecord,
    pub status: String,
    pub database_sha256: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
    pub signals: Vec<SignalSample>,
}

/// Cache only derived signals and status, never the original frame payload.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedSignals {
    pub status: String,
    pub database_sha256: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
    pub signals: Vec<SignalSample>,
}
impl DecodedFrame {
    pub fn cached(&self) -> CachedSignals {
        CachedSignals {
            status: self.status.clone(),
            database_sha256: self.database_sha256.clone(),
            message: self.message.clone(),
            error: self.error.clone(),
            signals: self.signals.clone(),
        }
    }
    pub fn from_cache(record: FrameRecord, payload: CachedSignals) -> Self {
        Self {
            schema_version: 1,
            kind: "decoded_frame",
            record,
            status: payload.status,
            database_sha256: payload.database_sha256,
            message: payload.message,
            error: payload.error,
            signals: payload.signals,
        }
    }
}
impl Decoder {
    /// Multiple databases on one channel are allowed only for disjoint raw IDs.
    pub fn load(bindings: &[String], cancel: &Cancellation) -> Result<Self> {
        ensure!(
            !bindings.is_empty(),
            "at least one --dbc CHANNEL=PATH assignment is required"
        );
        ensure!(
            bindings.len() <= 16,
            "at most 16 DBC assignments are allowed"
        );
        let mut decoder = Self {
            assignments: vec![],
            definitions: BTreeMap::new(),
            scratch: Scratch::default(),
        };
        for binding in bindings {
            cancel.check()?;
            let (channel, path) = binding
                .split_once('=')
                .context("DBC binding must be CHANNEL=PATH")?;
            let channel: u16 = channel
                .parse()
                .context("DBC channel must be a positive integer")?;
            ensure!(channel > 0, "DBC channel must be positive");
            let path = PathBuf::from(path).canonicalize()?;
            ensure!(
                std::fs::metadata(&path)?.len() <= MAX_DBC_BYTES,
                "DBC exceeds 32 MiB limit"
            );
            // Bounded read even if the file grows after metadata inspection.
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(&path)?
                .take(MAX_DBC_BYTES + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_DBC_BYTES,
                "DBC exceeds 32 MiB limit"
            );
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let document = Document::from_bytes(&bytes, None)
                .map_err(|e| anyhow!(e))
                .with_context(|| format!("loading DBC {}", path.display()))?;
            ensure!(
                document.db.is_can_bus(),
                "DBC describes a non-CAN bus: {}",
                path.display()
            );
            ensure!(
                document.warnings.is_empty(),
                "DBC parse findings in {}: {}",
                path.display(),
                serde_json::to_string(&document.warnings)?
            );
            let findings = analysis::check(&document);
            let errors: Vec<_> = findings.iter().filter(|f| f.severity == "error").collect();
            ensure!(
                errors.is_empty(),
                "invalid DBC {}: {}",
                path.display(),
                serde_json::to_string(&errors)?
            );
            let assignment = decoder.assignments.len();
            let mut count = 0;
            for message in document
                .db
                .messages
                .iter()
                .filter(|m| m.raw_id != 0xc000_0000)
            {
                ensure!(
                    message.raw_id & 0x6000_0000 == 0
                        && (message.extended() || message.raw_id <= 0x7ff),
                    "DBC ID {} has reserved bits or lacks an Extended flag",
                    message.raw_id
                );
                ensure!(
                    message.dlc <= 64,
                    "DBC message {} exceeds a single CAN frame; transport decoding is required",
                    message.name
                );
                let mut names = BTreeSet::new();
                ensure!(
                    message.signals.iter().all(|s| names.insert(&s.name)),
                    "duplicate signal name in {}",
                    message.name
                );
                let key = (channel, message.raw_id);
                ensure!(!decoder.definitions.contains_key(&key), "ambiguous DBC assignment: channel {channel}, raw ID {} is defined more than once", message.raw_id);
                decoder.definitions.insert(
                    key,
                    Definition {
                        assignment,
                        message: message.clone(),
                        compiled: CompiledMessage::compile(message),
                        requires_fd: document.db.frame_format(message).contains("CAN FD")
                            || message.dlc > 8,
                    },
                );
                count += 1;
            }
            ensure!(count > 0, "DBC has no CAN message definitions");
            decoder.assignments.push(Assignment {
                channel,
                path,
                sha256: hash,
                messages: count,
                warnings: findings
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<_, _>>()?,
            });
        }
        Ok(decoder)
    }

    pub fn verify_databases(&self, cancel: &Cancellation) -> Result<()> {
        for assignment in &self.assignments {
            ensure!(
                hash_file(&assignment.path, cancel)? == assignment.sha256,
                "DBC changed during decoding: {}",
                assignment.path.display()
            );
        }
        Ok(())
    }

    pub fn decode(&mut self, record: FrameRecord) -> Result<DecodedFrame> {
        let mut result = DecodedFrame {
            schema_version: 1,
            kind: "decoded_frame",
            record,
            status: "decoded".into(),
            database_sha256: None,
            message: None,
            error: None,
            signals: vec![],
        };
        let frame = &result.record.frame;
        if frame.remote() {
            result.status = "remote".into();
            return Ok(result);
        }
        let raw_id = frame.id() | if frame.extended() { 0x8000_0000 } else { 0 };
        let Some(definition) = self.definitions.get(&(frame.channel(), raw_id)) else {
            result.status = if self
                .assignments
                .iter()
                .any(|a| a.channel == frame.channel())
            {
                "no_message"
            } else {
                "no_database"
            }
            .into();
            return Ok(result);
        };
        let assignment = &self.assignments[definition.assignment];
        result.database_sha256 = Some(assignment.sha256.clone());
        result.message = Some(definition.message.name.clone());
        if definition.requires_fd && !frame.fd() {
            result.status = "format_mismatch".into();
            result.error = Some("DBC requires a CAN FD frame".into());
            return Ok(result);
        }
        let rows = match definition.compiled.decode(frame.data(), &mut self.scratch) {
            Ok(rows) => rows,
            Err(error) => {
                result.status = "length_mismatch".into();
                result.error = Some(error);
                return Ok(result);
            }
        };
        let by_name: BTreeMap<_, _> = rows
            .iter()
            .map(|r| (r["name"].as_str().unwrap_or(""), r))
            .collect();
        ensure!(
            by_name.len() == rows.len()
                && rows.iter().all(|r| definition
                    .message
                    .signals
                    .iter()
                    .any(|s| Some(s.name.as_str()) == r["name"].as_str())),
            "DBC engine result does not match signal definitions"
        );
        for (ordinal, signal) in definition.message.signals.iter().enumerate() {
            let mut sample = SignalSample {
                key: format!(
                    "{}:{}:{raw_id}:{ordinal}",
                    assignment.sha256,
                    frame.channel()
                ),
                definition_ordinal: ordinal,
                name: signal.name.clone(),
                status: "inactive".into(),
                raw: None,
                raw_text: None,
                physical: None,
                unit: signal.unit.clone(),
                description: String::new(),
            };
            if let Some(row) = by_name.get(signal.name.as_str()) {
                let raw = row["raw"]
                    .as_str()
                    .context("DBC engine raw value must be text")?;
                sample.description = row["description"].as_str().unwrap_or("").into();
                if raw.is_empty() {
                    sample.status = "decode_error".into();
                    result.status = "signal_error".into();
                } else {
                    sample.raw_text = Some(raw.into());
                    sample.raw = Some(match signal.value_type {
                        ValueType::Integer if signal.signed => RawValue::Signed(raw.parse()?),
                        ValueType::Integer => RawValue::Unsigned(raw.parse()?),
                        ValueType::Float | ValueType::Double => {
                            let bits = if signal.signed {
                                raw.parse::<i64>()? as u64
                            } else {
                                raw.parse::<u64>()?
                            };
                            RawValue::FloatBits(if signal.length == 32 {
                                bits & 0xffff_ffff
                            } else {
                                bits
                            })
                        }
                        ValueType::Invalid(_) => {
                            anyhow::bail!("invalid signal type escaped DBC validation")
                        }
                    });
                    sample.physical = row["physical"].as_f64().filter(|v| v.is_finite());
                    sample.status = if sample.physical.is_some() {
                        "valid"
                    } else {
                        "non_finite"
                    }
                    .into();
                    if sample.physical.is_none() {
                        result.status = "signal_error".into();
                    }
                }
            }
            result.signals.push(sample);
        }
        Ok(result)
    }
}
