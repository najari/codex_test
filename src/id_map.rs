//! Explicit ASC symbolic ID assignments. No ID or ID kind is inferred.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub channel: u16,
    pub name: String,
    pub id: u32,
    pub extended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub schema_version: u32,
    pub mappings: Vec<Assignment>,
}

#[derive(Debug)]
pub struct IdMap {
    pub document: Document,
    entries: BTreeMap<(u16, String), (u32, bool)>,
}
impl IdMap {
    pub fn load(path: &Path) -> Result<Self> {
        let mut data = Vec::new();
        File::open(path)
            .with_context(|| format!("opening ID map {}", path.display()))?
            .take(262145)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() <= 262144,
            "ID map exceeds 256 KiB resource limit"
        );
        let document: Document = serde_json::from_slice(&data).context("invalid ID map JSON")?;
        ensure!(
            document.schema_version == 1,
            "unsupported ID map schema version"
        );
        ensure!(
            !document.mappings.is_empty() && document.mappings.len() <= 4096,
            "ID map requires 1..4096 assignments"
        );
        let mut entries = BTreeMap::new();
        for a in &document.mappings {
            ensure!(a.channel > 0, "ID map channel must be 1-based and nonzero");
            ensure!(
                !a.name.is_empty()
                    && a.name.len() <= 128
                    && !a.name.chars().any(char::is_whitespace),
                "ID map name must be one token of 1..128 UTF-8 bytes"
            );
            ensure!(
                a.id <= if a.extended { 0x1fff_ffff } else { 0x7ff },
                "ID map ID outside its declared range"
            );
            // A token that is numeric under either supported base cannot be overridden.
            ensure!(
                crate::core::parse_id(&a.name, 10).is_err()
                    && crate::core::parse_id(&a.name, 16).is_err(),
                "ID map name is ambiguous with a numeric CAN ID: {}",
                a.name
            );
            ensure!(
                entries
                    .insert((a.channel, a.name.clone()), (a.id, a.extended))
                    .is_none(),
                "duplicate ID map channel/name: {} {}",
                a.channel,
                a.name
            );
        }
        Ok(Self { document, entries })
    }
    pub fn resolve(&self, channel: u16, name: &str) -> Option<(u32, bool)> {
        self.entries.get(&(channel, name.to_owned())).copied()
    }
}
