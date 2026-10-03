use canlog::{
    core::*,
    formats::{self, Format},
    output::AtomicOutput,
};
use std::{
    fs,
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(args)
        .output()
        .unwrap()
}
fn path(p: &std::path::Path) -> &str {
    p.to_str().unwrap()
}
fn fixture() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("input.asc");
    fs::write(&p,"base hex timestamps absolute\n0.000000001 1 100 Rx d 2 AA BB\n0.010000001 2 100x Tx r 9\n0.020000001 CANFD 1 Rx 200 1 1 9 12 00 01 02 03 04 05 06 07 08 09 0A 0B\n").unwrap();
    (dir, p)
}
#[test]
fn invalid_relative_delta_cannot_be_recovered() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("relative.asc");
    fs::write(
        &source,
        "base hex timestamps relative\n0.1 1 100 Rx d 0\nbad SV: event\n0.1 1 100 Rx d 0\n",
    )
    .unwrap();
    let output = cli(&["stats", path(&source), "--recover", "--unsupported", "skip"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("subsequent timestamps cannot be recovered"));
}
fn frames(p: &std::path::Path) -> Vec<Frame> {
    let mut reader = formats::open_reader(path(p), None, Limits::default()).unwrap();
    let mut result = Vec::new();
    while let Some(item) = reader.next_item().unwrap() {
        match item {
            ReadItem::Frame(r) => result.push(r.frame),
            ReadItem::Issue(i) => panic!("unexpected issue: {i:?}"),
        }
    }
    result
}
#[test]
fn asc_blf_jsonl_semantic_roundtrip() {
    let (dir, source) = fixture();
    let expected = frames(&source);
    let blf = dir.path().join("out.blf");
    let asc = dir.path().join("out.asc");
    assert!(cli(&[
        "record",
        "--input",
        path(&source),
        "-o",
        path(&blf),
        "--sync"
    ])
    .status
    .success());
    assert_eq!(frames(&blf), expected);
    assert!(cli(&["replay", path(&blf), "--no-wait", "-o", path(&asc)])
        .status
        .success());
    assert_eq!(frames(&asc), expected);
    let json = dir.path().join("out.jsonl");
    assert!(
        cli(&[
            "export",
            path(&asc),
            "-o",
            path(&json),
            "--allow-loss",
            "field:source-metadata"
        ])
        .status
        .code()
            == Some(0)
    );
    assert_eq!(frames(&json), expected);
}
#[test]
fn fd_lengths_flags_and_raw_dlc_roundtrip() {
    let dir = TempDir::new().unwrap();
    let mut expected = Vec::new();
    for dlc in 0..16 {
        expected.push(
            Frame::new(
                dlc as i64,
                1,
                0x1fff_ffff,
                true,
                Direction::Tx,
                false,
                true,
                dlc,
                vec![dlc; FD_LENGTHS[dlc as usize]],
                true,
                true,
            )
            .unwrap(),
        );
    }
    for dlc in 9..16 {
        expected.push(
            Frame::new(
                100 + dlc as i64,
                2,
                0x7ff,
                false,
                Direction::Rx,
                false,
                false,
                dlc,
                vec![dlc; 8],
                false,
                false,
            )
            .unwrap(),
        );
    }
    for format in [Format::Asc, Format::Blf, Format::Jsonl] {
        let p = dir
            .path()
            .join(format!("out.{format:?}").to_ascii_lowercase());
        let mut writer =
            formats::make_writer(fs::File::create(&p).unwrap(), format, &Metadata::default())
                .unwrap();
        for f in &expected {
            writer.write_frame(f).unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        assert_eq!(frames(&p), expected);
    }
}
#[test]
fn existing_output_same_path_and_hardlink_are_protected() {
    let (dir, source) = fixture();
    let out = dir.path().join("out.asc");
    fs::write(&out, "KEEP").unwrap();
    assert_eq!(
        cli(&["convert", path(&source), path(&out)]).status.code(),
        Some(2)
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), "KEEP");
    assert_eq!(
        cli(&["convert", path(&source), path(&source), "--overwrite"])
            .status
            .code(),
        Some(2)
    );
    let hard = dir.path().join("hard.asc");
    fs::hard_link(&source, &hard).unwrap();
    assert_eq!(
        cli(&["convert", path(&source), path(&hard), "--overwrite"])
            .status
            .code(),
        Some(2)
    );
}
#[test]
fn failure_rolls_back_and_reports() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("bad.asc");
    let out = dir.path().join("out.asc");
    let report = dir.path().join("report.json");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.0 1 100 Rx d 1 AA\n1.0 1 100 Rx d 2 FF\n",
    )
    .unwrap();
    fs::write(&out, "KEEP").unwrap();
    assert_eq!(
        cli(&[
            "convert",
            path(&source),
            path(&out),
            "--overwrite",
            "--report",
            path(&report)
        ])
        .status
        .code(),
        Some(1)
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), "KEEP");
    let r: serde_json::Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(r["status"], "failed");
    assert_eq!(r["published"], false);
    assert!(fs::read_dir(dir.path()).unwrap().all(|p| !p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".partial")));
}
#[test]
fn skip_is_not_loss_authorization() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("events.asc");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.0 SV: unknown\n0.1 1 100 Rx d 0\n",
    )
    .unwrap();
    let out = dir.path().join("out.blf");
    assert_eq!(
        cli(&[
            "convert",
            path(&source),
            path(&out),
            "--unsupported",
            "skip"
        ])
        .status
        .code(),
        Some(1)
    );
    assert!(!out.exists());
    assert_eq!(
        cli(&[
            "convert",
            path(&source),
            path(&out),
            "--unsupported",
            "skip",
            "--allow-loss",
            "unsupported-record"
        ])
        .status
        .code(),
        Some(3)
    );
    assert_eq!(frames(&out).len(), 1);
}
#[test]
fn recover_does_not_override_strict_or_resource_limits() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("bad.asc");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.0 1 100 Rx d 2 FF\n0.1 1 100 Rx d 0\n",
    )
    .unwrap();
    let out = dir.path().join("out.blf");
    assert_eq!(
        cli(&[
            "convert",
            path(&source),
            path(&out),
            "--allow-loss",
            "corrupted-region"
        ])
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        cli(&[
            "convert",
            path(&source),
            path(&out),
            "--recover",
            "--allow-loss",
            "corrupted-region"
        ])
        .status
        .code(),
        Some(3)
    );
    fs::write(
        &source,
        format!("base hex timestamps absolute\n0.0 {}\n", "A".repeat(1024)),
    )
    .unwrap();
    assert_eq!(
        cli(&[
            "stats",
            path(&source),
            "--recover",
            "--max-line-bytes",
            "256"
        ])
        .status
        .code(),
        Some(1)
    );
}
#[test]
fn filter_is_half_open_and_distinguishes_id_kind() {
    let (dir, source) = fixture();
    let out = dir.path().join("out.blf");
    assert!(cli(&[
        "filter",
        path(&source),
        "-o",
        path(&out),
        "--start",
        "0.010000001",
        "--end",
        "0.020000001",
        "--id",
        "0x100",
        "--id-kind",
        "extended"
    ])
    .status
    .success());
    let result = frames(&out);
    assert_eq!(result.len(), 1);
    assert!(result[0].extended());
    assert!(result[0].remote());
}
#[test]
fn repeat_preserves_source_intervals_and_speed_does_not_change_timestamps() {
    let (dir, source) = fixture();
    let out = dir.path().join("out.blf");
    assert!(cli(&[
        "replay",
        path(&source),
        "--no-wait",
        "--speed",
        "2",
        "--repeat",
        "2",
        "--repeat-gap",
        "0.005",
        "-o",
        path(&out)
    ])
    .status
    .success());
    let result = frames(&out);
    assert_eq!(result.len(), 6);
    assert_eq!(
        result.iter().map(Frame::timestamp_ns).collect::<Vec<_>>(),
        vec![1, 10_000_001, 20_000_001, 25_000_001, 35_000_001, 45_000_001]
    );
}
#[test]
fn replay_regression_and_invalid_options() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("reverse.asc");
    fs::write(
        &source,
        "base hex timestamps absolute\n1.0 1 100 Rx d 0\n0.0 1 100 Rx d 0\n",
    )
    .unwrap();
    assert_eq!(
        cli(&["replay", path(&source), "--no-wait"]).status.code(),
        Some(1)
    );
    assert!(cli(&[
        "replay",
        path(&source),
        "--no-wait",
        "--on-regression",
        "immediate"
    ])
    .status
    .success());
    for extra in [
        vec!["--speed", "NaN"],
        vec!["--repeat", "0"],
        vec!["--start", "2", "--end", "1"],
    ] {
        let mut a = vec!["replay", path(&source)];
        a.extend(extra);
        assert_eq!(cli(&a).status.code(), Some(2));
    }
}
#[test]
fn stop_control_interrupts_long_wait_and_discards_output() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("long.asc");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.0 1 100 Rx d 0\n3600.0 1 100 Rx d 0\n",
    )
    .unwrap();
    let out = dir.path().join("out.blf");
    let mut child = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(["replay", path(&source), "-o", path(&out), "--control-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(130));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!out.exists());
}
#[test]
fn empty_and_unresolved_inputs_are_explicit() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("empty.asc");
    fs::write(&source, "base hex timestamps absolute\n").unwrap();
    assert!(
        cli(&["replay", path(&source), "--repeat", "5", "--no-wait"])
            .status
            .success()
    );
    fs::write(
        &source,
        "base hex timestamps absolute\n0.0 1 Stress2 Tx d 0\n",
    )
    .unwrap();
    let r = cli(&["stats", path(&source), "--unsupported", "skip"]);
    assert_eq!(r.status.code(), Some(3));
    let j: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(j["issue_counts"]["UnresolvedId"], 1);
}
#[test]
fn jsonl_stdin_is_validated() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("out.blf");
    let mut child = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args([
            "record",
            "--input",
            "-",
            "--input-format",
            "jsonl",
            "-o",
            path(&out),
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let f = Frame::new(
        1,
        1,
        1,
        false,
        Direction::Rx,
        false,
        false,
        0,
        vec![],
        false,
        false,
    )
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    canlog::formats::jsonl::write_json(&mut stdin, &f).unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    assert_eq!(frames(&out), vec![f]);
}

fn container_file(p: &std::path::Path, containers: &[Vec<u8>], declared_override: Option<u32>) {
    let mut file = vec![0; 144];
    file[..4].copy_from_slice(b"LOGG");
    file[4..8].copy_from_slice(&144u32.to_le_bytes());
    file[32..36].copy_from_slice(&1u32.to_le_bytes());
    for data in containers {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        let bytes = enc.finish().unwrap();
        let size = 32 + bytes.len();
        let mut head = vec![0; 32];
        head[..4].copy_from_slice(b"LOBJ");
        head[4..6].copy_from_slice(&16u16.to_le_bytes());
        head[6..8].copy_from_slice(&1u16.to_le_bytes());
        head[8..12].copy_from_slice(&(size as u32).to_le_bytes());
        head[12..16].copy_from_slice(&10u32.to_le_bytes());
        head[16..18].copy_from_slice(&2u16.to_le_bytes());
        head[24..28].copy_from_slice(&declared_override.unwrap_or(data.len() as u32).to_le_bytes());
        file.extend(head);
        file.extend(bytes);
        file.extend(vec![0; size % 4]);
    }
    let len = file.len() as u64;
    file[16..24].copy_from_slice(&len.to_le_bytes());
    fs::write(p, file).unwrap();
}
fn one_object() -> Vec<u8> {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("obj.blf");
    let mut w = formats::make_writer(
        fs::File::create(&p).unwrap(),
        Format::Blf,
        &Metadata::default(),
    )
    .unwrap();
    w.write_frame(
        &Frame::new(
            1234,
            1,
            0x123,
            false,
            Direction::Rx,
            false,
            false,
            1,
            vec![0xab],
            false,
            false,
        )
        .unwrap(),
    )
    .unwrap();
    w.finish().unwrap();
    drop(w);
    let b = fs::read(p).unwrap();
    let size = u32::from_le_bytes(b[152..156].try_into().unwrap()) as usize;
    let mut obj = Vec::new();
    flate2::read::ZlibDecoder::new(&b[176..144 + size])
        .read_to_end(&mut obj)
        .unwrap();
    obj
}
#[test]
fn blf_cross_container_at_every_byte() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("split.blf");
    let obj = one_object();
    for split in 1..obj.len() {
        container_file(&p, &[obj[..split].to_vec(), obj[split..].to_vec()], None);
        assert_eq!(frames(&p)[0].timestamp_ns(), 1234);
    }
}
#[test]
fn blf_truncation_sizes_and_compression_budget() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("bad.blf");
    let mut obj = one_object();
    obj.pop();
    container_file(&p, &[obj], None);
    assert_eq!(
        cli(&["stats", path(&p), "--recover"]).status.code(),
        Some(1)
    );
    container_file(&p, &[one_object()], Some(100_000_000));
    assert_eq!(cli(&["stats", path(&p)]).status.code(), Some(1));
    container_file(&p, &[vec![0; 1024]], Some(128));
    assert_eq!(
        cli(&["stats", path(&p), "--max-container-bytes", "256"])
            .status
            .code(),
        Some(1)
    );
}
#[test]
fn atomic_publication_noclobber_race() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("out");
    let output = AtomicOutput::new(&p, "-", false).unwrap();
    output.file().unwrap().write_all(b"NEW").unwrap();
    fs::write(&p, b"KEEP").unwrap();
    assert!(output.publish(false).is_err());
    assert_eq!(fs::read(p).unwrap(), b"KEEP");
}

#[test]
fn json_deserialization_cannot_bypass_invariants() {
    let valid = Frame::new(
        0,
        1,
        0x7ff,
        false,
        Direction::Rx,
        false,
        false,
        0,
        vec![],
        false,
        false,
    )
    .unwrap();
    let mut raw = serde_json::to_value(valid).unwrap();
    raw["id"] = 0x800.into();
    assert!(serde_json::from_value::<Frame>(raw).is_err());
}
#[test]
fn channel_statistics_and_signal_rows_are_unsupported_not_corrupt() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("events.asc");
    fs::write(&source,"base hex timestamps relative\n0.1 1 Statistic: D 0 R 0\n0.1 1 Network::Message::Signal = 5\n0.1 1 100 Rx d 0\n").unwrap();
    let r = cli(&["stats", path(&source), "--unsupported", "skip"]);
    assert_eq!(r.status.code(), Some(3));
    let j: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(j["issue_counts"]["UnsupportedRecord"], 2);
    assert_eq!(j["first_timestamp_ns"], 300_000_000);
}
#[test]
fn blf_global_metadata_without_time_unit_is_skippable() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("global.blf");
    let mut object = one_object();
    object[12..16].copy_from_slice(&65u32.to_le_bytes());
    object[16..20].copy_from_slice(&0u32.to_le_bytes());
    container_file(&source, &[object], None);
    let result = cli(&["stats", path(&source), "--unsupported", "skip"]);
    assert_eq!(result.status.code(), Some(3));
    let j: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(j["issue_examples"][0]["timestamp_ns"].is_null());
}
#[test]
fn blf_v2_and_ten_microsecond_timestamp_unit() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("v2.blf");
    let old = one_object();
    let mut object = old[..32].to_vec();
    object.extend_from_slice(&[0; 8]);
    object.extend_from_slice(&old[32..]);
    object[4..6].copy_from_slice(&40u16.to_le_bytes());
    object[6..8].copy_from_slice(&2u16.to_le_bytes());
    let size = object.len() as u32;
    object[8..12].copy_from_slice(&size.to_le_bytes());
    object[16..20].copy_from_slice(&1u32.to_le_bytes());
    object[24..32].copy_from_slice(&12u64.to_le_bytes());
    container_file(&p, &[object], None);
    assert_eq!(frames(&p)[0].timestamp_ns(), 120_000);
}
#[test]
fn blf_fd64_and_truncation_no_padding() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("fd64.blf");
    let base = one_object();
    let mut object = vec![0; 32 + 40 + 12];
    object[..32].copy_from_slice(&base[..32]);
    let size = object.len() as u32;
    object[8..12].copy_from_slice(&size.to_le_bytes());
    object[12..16].copy_from_slice(&101u32.to_le_bytes());
    let b = &mut object[32..];
    b[0] = 7;
    b[1] = 9;
    b[2] = 12;
    b[4..8].copy_from_slice(&0x9fff_ffffu32.to_le_bytes());
    b[12..16].copy_from_slice(&0x7000u32.to_le_bytes());
    b[34] = 1;
    for i in 0..12 {
        b[40 + i] = i as u8;
    }
    container_file(&p, &[object.clone()], None);
    let expected = Frame::new(
        1234,
        7,
        0x1fff_ffff,
        true,
        Direction::Tx,
        false,
        true,
        9,
        (0..12).collect(),
        true,
        true,
    )
    .unwrap();
    assert_eq!(frames(&p), vec![expected]);
    object.pop();
    let size = object.len() as u32;
    object[8..12].copy_from_slice(&size.to_le_bytes());
    container_file(&p, &[object], None);
    assert_eq!(cli(&["stats", path(&p)]).status.code(), Some(1));
}
#[test]
fn finalize_reports_io_errors() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("readonly.asc");
    fs::write(&p, "KEEP").unwrap();
    let mut writer = formats::make_writer(
        fs::File::open(&p).unwrap(),
        Format::Asc,
        &Metadata::default(),
    )
    .unwrap();
    assert!(writer.finish().is_err());
    drop(writer);
    assert_eq!(fs::read_to_string(&p).unwrap(), "KEEP");
}

#[test]
fn legacy_encoding_bom_and_date_precision_are_reported() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("legacy.asc");
    fs::write(
        &source,
        b"date Die M\xe4r 13 11:58:02 2007\nbase hex timestamps absolute\n0.0 1 100 Rx d 0\n",
    )
    .unwrap();
    let result = cli(&["stats", path(&source)]);
    assert!(result.status.success());
    let r: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(r["metadata"]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n == "Windows-1252 input"));
    fs::write(&source,"\u{feff}date Thu Jan 1 00:00:00.123456 1970\nbase hex timestamps absolute\n0.0 1 100 Rx d 0\n").unwrap();
    let out = dir.path().join("out.blf");
    assert_eq!(
        cli(&["record", "--input", path(&source), "-o", path(&out)])
            .status
            .code(),
        Some(1)
    );
    assert!(!out.exists());
    assert_eq!(
        cli(&[
            "record",
            "--input",
            path(&source),
            "-o",
            path(&out),
            "--allow-loss",
            "field:source-metadata"
        ])
        .status
        .code(),
        Some(3)
    );
}
#[test]
fn view_includes_source_location_and_trailing_bytes_are_not_hidden() {
    let (dir, source) = fixture();
    let result = cli(&["view", path(&source), "--limit", "1"]);
    assert!(result.status.success());
    let r: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(r["source_location"]["line"], 2);
    let bad = dir.path().join("extra.asc");
    fs::write(
        &bad,
        "base hex timestamps absolute\n0.0 1 100 Rx d 1 AA BB\n",
    )
    .unwrap();
    assert_eq!(cli(&["stats", path(&bad)]).status.code(), Some(1));
}
#[test]
fn separate_trigger_clocks_are_not_merged() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("triggers.asc");
    fs::write(&source,"base hex timestamps absolute\nBegin Triggerblock first\n0.0 1 100 Rx d 0\nEnd Triggerblock\nBegin Triggerblock second\n0.0 1 100 Rx d 0\n").unwrap();
    assert_eq!(
        cli(&["stats", path(&source), "--recover", "--unsupported", "skip"])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn independent_fd100_layout_reads_and_writes_flags_at_byte_13() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("fd100.blf");
    let mut obj = vec![0; 116];
    obj[..4].copy_from_slice(b"LOBJ");
    obj[4..6].copy_from_slice(&32u16.to_le_bytes());
    obj[6..8].copy_from_slice(&1u16.to_le_bytes());
    obj[8..12].copy_from_slice(&116u32.to_le_bytes());
    obj[12..16].copy_from_slice(&100u32.to_le_bytes());
    obj[16..20].copy_from_slice(&2u32.to_le_bytes());
    let b = &mut obj[32..];
    b[..2].copy_from_slice(&1u16.to_le_bytes());
    b[3] = 9;
    b[4..8].copy_from_slice(&0x100u32.to_le_bytes());
    b[12] = 100;
    b[13] = 7;
    b[14] = 12;
    b[20..32].copy_from_slice(&[0xaa; 12]);
    container_file(&p, &[obj], None);
    let expected = Frame::new(
        0,
        1,
        0x100,
        false,
        Direction::Rx,
        false,
        true,
        9,
        vec![0xaa; 12],
        true,
        true,
    )
    .unwrap();
    assert_eq!(frames(&p), vec![expected.clone()]);
    let output = dir.path().join("written.blf");
    let mut w = formats::make_writer(
        fs::File::create(&output).unwrap(),
        Format::Blf,
        &Metadata::default(),
    )
    .unwrap();
    w.write_frame(&expected).unwrap();
    w.finish().unwrap();
    drop(w);
    let bytes = fs::read(output).unwrap();
    let size = u32::from_le_bytes(bytes[152..156].try_into().unwrap()) as usize;
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&bytes[176..144 + size])
        .read_to_end(&mut raw)
        .unwrap();
    assert_eq!(raw[32 + 13], 7);
    assert_eq!(raw[32 + 14], 12);
    assert_eq!(&raw[32 + 20..32 + 32], &[0xaa; 12]);
}

#[test]
fn recording_is_not_limited_by_optional_statistics_key_budget() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("ids.asc");
    let out = dir.path().join("ids.blf");
    let mut text = "base hex timestamps absolute\n".to_owned();
    for i in 0..5000 {
        text.push_str(&format!("0.0 1 {i:X}x Rx d 0\n"));
    }
    fs::write(&source, text).unwrap();
    assert!(cli(&["record", "--input", path(&source), "-o", path(&out)])
        .status
        .success());
    assert_eq!(frames(&out).len(), 5000);
    let replay = cli(&["replay", path(&out), "--no-wait"]);
    assert!(replay.status.success());
    assert_eq!(
        replay
            .stdout
            .split(|&c| c == b'\n')
            .filter(|s| !s.is_empty())
            .count(),
        5000
    );
}

#[test]
fn issues_proven_outside_selection_do_not_degrade_selected_output() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("input.asc");
    let out = dir.path().join("out.blf");
    let report = dir.path().join("report.json");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.0 SV: value\n1.0 1 100 Rx d 0\n",
    )
    .unwrap();
    assert!(cli(&[
        "filter",
        path(&source),
        "-o",
        path(&out),
        "--start",
        "1",
        "--end",
        "2",
        "--report",
        path(&report)
    ])
    .status
    .success());
    let r: serde_json::Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(r["issues"], 1);
    assert_eq!(r["issues_outside_selection"], 1);
    assert_eq!(r["status"], "complete");
}
#[test]
fn unknown_origin_remains_unknown_after_canonical_asc_write() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("unknown.asc");
    let mut writer = formats::make_writer(
        fs::File::create(&source).unwrap(),
        Format::Asc,
        &Metadata::default(),
    )
    .unwrap();
    writer
        .write_frame(
            &Frame::new(
                1,
                1,
                1,
                false,
                Direction::Rx,
                false,
                false,
                0,
                vec![],
                false,
                false,
            )
            .unwrap(),
        )
        .unwrap();
    writer.finish().unwrap();
    drop(writer);
    let mut reader = formats::open_reader(path(&source), None, Limits::default()).unwrap();
    reader.next_item().unwrap();
    assert!(reader.metadata().date_text.is_none());
    assert!(reader.metadata().blf_start.is_none());
}

#[test]
fn blf_padding_is_bound_to_an_object_and_cumulative_across_containers() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("padding.blf");
    container_file(&p, &[vec![0; 2]], None);
    assert_eq!(cli(&["stats", path(&p)]).status.code(), Some(1));
    let mut first = one_object();
    first.extend([0; 2]);
    let mut second = vec![0; 2];
    second.extend(one_object());
    container_file(&p, &[first, second], None);
    assert_eq!(cli(&["stats", path(&p)]).status.code(), Some(1));
}
