//! Versioned, streaming exports of the existing typed DBC decoder results.
use crate::{
    dbc::{DecodedFrame, RawValue, ENGINE_REVISION},
    formats::Format,
};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{io::Write, path::Path};

pub const CSV_COLUMNS: &[&str] = &[
    "schema_version",
    "row_kind",
    "timestamp_ns",
    "channel",
    "id",
    "extended",
    "direction",
    "remote",
    "fd",
    "raw_dlc",
    "data_hex",
    "brs",
    "esi",
    "source",
    "ordinal",
    "line",
    "container",
    "object_offset",
    "engine_revision",
    "frame_status",
    "database_sha256",
    "message",
    "error",
    "signal_key",
    "definition_ordinal",
    "signal_name",
    "signal_status",
    "raw_type",
    "raw_value",
    "raw_text",
    "physical",
    "unit",
    "description",
];

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SignalFormat {
    Jsonl,
    Csv,
}
impl From<SignalFormat> for Format {
    fn from(value: SignalFormat) -> Self {
        match value {
            SignalFormat::Jsonl => Self::Jsonl,
            SignalFormat::Csv => Self::Csv,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CsvSchema {
    pub id: &'static str,
    pub version: u32,
    pub encoding: &'static str,
    pub line_ending: &'static str,
    pub delimiter: &'static str,
    pub null_token: &'static str,
    pub string_escape: &'static str,
    pub integer_representation: &'static str,
    pub physical_representation: &'static str,
    pub timestamp_unit: &'static str,
    pub timestamp_origin: &'static str,
    pub columns: &'static [&'static str],
}

pub fn schema(format: Format) -> Option<CsvSchema> {
    (format == Format::Csv).then_some(CsvSchema {
        id: "canlog-signal-csv-v1",
        version: 1,
        encoding: "UTF-8",
        line_ending: "CRLF",
        delimiter: ",",
        null_token: "\\N",
        string_escape: "double leading backslash; double CSV quotes",
        integer_representation: "exact decimal text; raw_type declares signed/unsigned/float_bits",
        physical_representation: "finite f64 round-trip decimal text",
        timestamp_unit: "ns",
        timestamp_origin: "input offset; replay repeat offset is applied",
        columns: CSV_COLUMNS,
    })
}

/// Preserve the existing arbitrary destination names used by JSONL analysis.
/// CSV is inferred only from .csv or explicitly selected with --format csv.
pub fn output_format(path: Option<&Path>, explicit: Option<Format>) -> Result<Format> {
    let inferred = path
        .and_then(|p| p.extension())
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("csv"));
    let format = explicit.unwrap_or(if inferred { Format::Csv } else { Format::Jsonl });
    ensure!(
        matches!(format, Format::Jsonl | Format::Csv),
        "signal export supports JSONL or CSV only"
    );
    Ok(format)
}

pub fn write_header(out: &mut (impl Write + ?Sized), format: Format) -> Result<()> {
    ensure!(
        matches!(format, Format::Jsonl | Format::Csv),
        "signal export supports JSONL or CSV only"
    );
    if format == Format::Csv {
        writeln_crlf(out, &CSV_COLUMNS.join(","))?;
    }
    Ok(())
}

/// Count data rows, excluding the CSV header. Frames with no signal samples
/// produce a frame_status row so remote/unassigned/malformed payloads remain visible.
pub fn write_frame(
    out: &mut (impl Write + ?Sized),
    decoded: &DecodedFrame,
    format: Format,
) -> Result<u64> {
    match format {
        Format::Jsonl => {
            serde_json::to_writer(&mut *out, decoded)?;
            out.write_all(b"\n")?;
            Ok(1)
        }
        Format::Csv => write_csv_frame(out, decoded),
        _ => anyhow::bail!("signal export supports JSONL or CSV only"),
    }
}

fn writeln_crlf(out: &mut (impl Write + ?Sized), text: &str) -> Result<()> {
    out.write_all(text.as_bytes())?;
    out.write_all(b"\r\n")?;
    Ok(())
}

fn write_cell(out: &mut (impl Write + ?Sized), text: Option<&str>) -> Result<()> {
    let Some(text) = text else {
        out.write_all(b"\\N")?;
        return Ok(());
    };
    out.write_all(b"\"")?;
    // \N denotes null even after a standard CSV reader has removed quoting.
    // Escape every non-null leading backslash to make literal \N unambiguous.
    if text.starts_with('\\') {
        out.write_all(b"\\")?;
    }
    out.write_all(text.replace('"', "\"\"").as_bytes())?;
    out.write_all(b"\"")?;
    Ok(())
}

fn write_row(
    out: &mut (impl Write + ?Sized),
    prefix: &[Option<String>],
    signal: &[Option<String>],
) -> Result<()> {
    for (n, field) in prefix.iter().chain(signal).enumerate() {
        if n != 0 {
            out.write_all(b",")?;
        }
        write_cell(out, field.as_deref())?;
    }
    out.write_all(b"\r\n")?;
    Ok(())
}

fn write_csv_frame(out: &mut (impl Write + ?Sized), decoded: &DecodedFrame) -> Result<u64> {
    let f = &decoded.record.frame;
    let location = &decoded.record.location;
    let row_kind = if decoded.signals.is_empty() {
        "frame_status"
    } else {
        "signal"
    };
    let prefix = vec![
        Some("1".into()),
        Some(row_kind.into()),
        Some(f.timestamp_ns().to_string()),
        Some(f.channel().to_string()),
        Some(f.id().to_string()),
        Some(f.extended().to_string()),
        Some(
            match f.direction() {
                crate::core::Direction::Rx => "rx",
                crate::core::Direction::Tx => "tx",
                crate::core::Direction::Unknown => "unknown",
            }
            .into(),
        ),
        Some(f.remote().to_string()),
        Some(f.fd().to_string()),
        Some(f.raw_dlc().to_string()),
        Some(f.data().iter().map(|b| format!("{b:02X}")).collect()),
        Some(f.brs().to_string()),
        Some(f.esi().to_string()),
        Some(location.source.clone()),
        Some(location.ordinal.to_string()),
        location.line.map(|v| v.to_string()),
        location.container.map(|v| v.to_string()),
        location.object_offset.map(|v| v.to_string()),
        Some(ENGINE_REVISION.into()),
        Some(decoded.status.clone()),
        decoded.database_sha256.clone(),
        decoded.message.clone(),
        decoded.error.clone(),
    ];
    if decoded.signals.is_empty() {
        write_row(out, &prefix, &vec![None; CSV_COLUMNS.len() - prefix.len()])?;
        return Ok(1);
    }
    for s in &decoded.signals {
        ensure!(
            s.physical.is_none_or(f64::is_finite),
            "signal CSV physical value must be finite"
        );
        let (raw_type, raw_value) = match &s.raw {
            Some(RawValue::Signed(v)) => (Some("signed".into()), Some(v.to_string())),
            Some(RawValue::Unsigned(v)) => (Some("unsigned".into()), Some(v.to_string())),
            Some(RawValue::FloatBits(v)) => (Some("float_bits".into()), Some(v.to_string())),
            None => (None, None),
        };
        let signal = vec![
            Some(s.key.clone()),
            Some(s.definition_ordinal.to_string()),
            Some(s.name.clone()),
            Some(s.status.clone()),
            raw_type,
            raw_value,
            s.raw_text.clone(),
            s.physical.map(|v| v.to_string()),
            Some(s.unit.clone()),
            Some(s.description.clone()),
        ];
        write_row(out, &prefix, &signal)?;
    }
    Ok(decoded.signals.len() as u64)
}
