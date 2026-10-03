use canlog::{core::*, formats};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(args)
        .output()
        .unwrap()
}
fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn read(p: &Path, preserve: bool) -> Vec<ReadItem> {
    let mut reader = formats::open_reader(path(p), None, Limits::default()).unwrap();
    reader.configure(preserve, None).unwrap();
    let mut items = vec![];
    while let Some(item) = reader.next_item().unwrap() {
        items.push(item);
    }
    items
}
fn natives(p: &Path) -> Vec<NativeRecord> {
    read(p, true)
        .into_iter()
        .map(|item| match item {
            ReadItem::Frame(f) => f.native.unwrap(),
            ReadItem::Issue(i) => i.native.unwrap(),
        })
        .collect()
}

#[test]
fn trigger_case_and_whitespace_variants_keep_relative_timeline() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    for begin in [
        "Begin Triggerblock",
        "Begin TriggerBlock",
        "BEGIN\tTRIGGERBLOCK",
    ] {
        fs::write(&input, format!("base hex timestamps relative\n{begin} example\n0.1 Start of measurement\n0.2 1 100 Rx d 1 AA\nend\ttriggerblock\n")).unwrap();
        let items = read(&input, false);
        assert_eq!(items.len(), 1);
        let ReadItem::Frame(frame) = &items[0] else {
            panic!("trigger parsed as issue")
        };
        assert_eq!(frame.frame.timestamp_ns(), 300_000_000);
        assert_eq!(frame.frame.data(), &[0xaa]);
    }
    fs::write(&input, "base hex timestamps relative\nBegin TriggerBlock one\n0.1 1 100 Rx d 0\nEND TRIGGERBLOCK\nbegin triggerblock two\n0.1 1 100 Rx d 0\n").unwrap();
    let output = cli(&["stats", path(&input), "--recover", "--unsupported", "skip"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple ASC trigger blocks"));
}

#[test]
fn fd_seven_field_annotations_preserve_all_64_payload_bytes() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let output = dir.path().join("out.asc");
    let report = dir.path().join("report.json");
    let data = (0..64)
        .map(|v| format!("{v:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    fs::write(&input, format!("base hex timestamps absolute\n0.1 CANFD 4 Rx 4EE 0 1 f 64 {data} 100 11 0 1 2 3 4\n0.2 CANFD 4 Tx 1ABCx NamedFrame 1 0 f 64 {data} 0 0 3000 0 0 0 0 0\n")).unwrap();
    let items = read(&input, false);
    assert_eq!(items.len(), 2);
    for (i, item) in items.iter().enumerate() {
        let ReadItem::Frame(frame) = item else {
            panic!("valid FD row rejected")
        };
        assert_eq!(frame.frame.data(), &(0..64).collect::<Vec<u8>>());
        assert_eq!(frame.frame.raw_dlc(), 15);
        assert_eq!(frame.frame.channel(), 4);
        assert_eq!(frame.frame.brs(), i == 1);
        assert_eq!(frame.frame.esi(), i == 0);
    }
    let result = cli(&[
        "record",
        "--input",
        path(&input),
        "-o",
        path(&output),
        "--preserve-records",
        "--report",
        path(&report),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(natives(&input), natives(&output));
    let report: serde_json::Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["frames_written"], 2);
    assert_eq!(report["losses"], serde_json::json!({}));
}

#[test]
fn fd_variants_do_not_accept_invalid_trailers_or_truncated_payloads() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    for row in [
        "0.0 CANFD 1 Rx 100 0 0 1 1 AA 0 0 0 0 0 0",
        "0.0 CANFD 1 Rx 100 0 0 1 1 AA 0 0 0 0 0 0 nope",
        "0.0 CANFD 1 Rx 100 0 0 2 2 AA",
        "0.0 CANFD 1 Rx 100 0 0 2 1 AA 0 0 0 0 0 0 0",
    ] {
        fs::write(&input, format!("base hex timestamps absolute\n{row}\n")).unwrap();
        let items = read(&input, true);
        assert_eq!(items.len(), 1);
        let ReadItem::Issue(issue) = &items[0] else {
            panic!("invalid FD row accepted")
        };
        assert_eq!(issue.kind, IssueKind::CorruptedRegion);
        assert!(issue.native.is_none());
    }
}

#[test]
fn conflicting_numeric_ids_are_reported_skipped_or_preserved_without_guessing() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let output = dir.path().join("out.asc");
    let report = dir.path().join("report.json");
    fs::write(&input, "base hex timestamps absolute\n0.1 1 100 Rx d 1 AA ID = 386\n0.2 1 100 Rx d 1 BB ID = 256\n0.3 1 100x Rx d 0 ID = 256\n0.4 1 Symbolic Rx d 0 ID = 256 ID = 257\n").unwrap();
    let strict = cli(&["view", path(&input), "--unlimited"]);
    assert_eq!(strict.status.code(), Some(1));
    assert!(strict.stdout.is_empty());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("conflicting ASC CAN IDs"));
    let skipped = cli(&["view", path(&input), "--unlimited", "--unsupported", "skip"]);
    assert_eq!(skipped.status.code(), Some(3));
    let frames: Vec<serde_json::Value> = String::from_utf8(skipped.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["frame"]["id"], 256);
    let result = cli(&[
        "record",
        "--input",
        path(&input),
        "-o",
        path(&output),
        "--preserve-records",
        "--report",
        path(&report),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(natives(&input), natives(&output));
    let report: serde_json::Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["frames_written"], 1);
    assert_eq!(report["issues_preserved"], 3);
    assert_eq!(report["preserved_counts"]["ConflictingId"], 3);
    assert_eq!(report["losses"], serde_json::json!({}));
}

#[test]
fn symbolic_trailer_still_resolves_and_malformed_conflict_cannot_be_preserved() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    fs::write(&input, "base hex timestamps absolute\n0.0 1 Symbolic Rx d 1 AA ID = 256\n0.1 1 100 Rx d 2 BB ID = 386\n").unwrap();
    let items = read(&input, true);
    let ReadItem::Frame(frame) = &items[0] else {
        panic!("symbolic trailer rejected")
    };
    assert_eq!(frame.frame.id(), 256);
    let ReadItem::Issue(issue) = &items[1] else {
        panic!("truncated payload accepted")
    };
    assert_eq!(issue.kind, IssueKind::CorruptedRegion);
    assert!(issue.native.is_none());
    let output = dir.path().join("out.asc");
    assert_eq!(
        cli(&[
            "record",
            "--input",
            path(&input),
            "-o",
            path(&output),
            "--preserve-records"
        ])
        .status
        .code(),
        Some(1)
    );
    assert!(!output.exists());
}
