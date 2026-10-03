use canlog::{
    core::*,
    formats::{self, Format},
    id_map::IdMap,
};
use std::{fs, io::Write, path::Path, process::Command};
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
fn native(p: &Path) -> Vec<NativeRecord> {
    let mut reader = formats::open_reader(path(p), None, Limits::default()).unwrap();
    reader.configure(true, None).unwrap();
    let mut result = vec![];
    while let Some(item) = reader.next_item().unwrap() {
        result.push(match item {
            ReadItem::Frame(f) => f.native.unwrap(),
            ReadItem::Issue(i) => i.native.unwrap(),
        });
    }
    result
}
fn report(p: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

#[test]
fn preserve_relative_decimal_asc_events_errors_and_frame_annotations() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("source.asc");
    let output = dir.path().join("out.asc");
    let r = dir.path().join("report.json");
    fs::write(&input, "base dec timestamps relative\n0.1 Start of measurement\n0.1 1 ErrorFrame Flags=255\n0.1 SV: Counter = 15\n0.1 1 Message Tx d 1 255 ID = 272 Length = 900\n0.1 CANFD 1 Rx 300 0 0 9 12 0 1 2 3 4 5 6 7 8 9 10 11\n0.1 1 Unresolved Rx d 0\n").unwrap();
    let result = cli(&[
        "record",
        "--input",
        path(&input),
        "-o",
        path(&output),
        "--preserve-records",
        "--sync",
        "--report",
        path(&r),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(native(&input), native(&output));
    let r = report(&r);
    assert_eq!(r["frames_written"], 2);
    assert_eq!(r["native_records_written"], 6);
    assert_eq!(r["issues_preserved"], 4);
    assert_eq!(r["status"], "complete");
    assert_eq!(r["losses"], serde_json::json!({}));
    assert_eq!(r["durable"], true);
    assert!(!r["issue_examples"][0]
        .as_object()
        .unwrap()
        .contains_key("native"));
    let replay = dir.path().join("replayed.asc");
    assert!(cli(&[
        "replay",
        path(&output),
        "-o",
        path(&replay),
        "--no-wait",
        "--preserve-records"
    ])
    .status
    .success());
    assert_eq!(native(&input), native(&replay));
}

fn blf(path: &Path, objects: &[Vec<u8>]) {
    let mut header = vec![0u8; 144];
    header[..4].copy_from_slice(b"LOGG");
    header[4..8].copy_from_slice(&144u32.to_le_bytes());
    header[32..36].copy_from_slice(&(objects.len() as u32).to_le_bytes());
    let start: [u16; 8] = [2026, 10, 6, 3, 10, 11, 12, 13];
    let stop: [u16; 8] = [2026, 10, 6, 3, 10, 12, 13, 14];
    for (i, value) in start.into_iter().enumerate() {
        header[40 + 2 * i..42 + 2 * i].copy_from_slice(&value.to_le_bytes());
    }
    for (i, value) in stop.into_iter().enumerate() {
        header[56 + 2 * i..58 + 2 * i].copy_from_slice(&value.to_le_bytes());
    }
    let mut data = vec![];
    for object in objects {
        data.extend(object);
        data.extend(vec![0; object.len() % 4]);
    }
    // Deliberately split inside an object to cover the reader carry path.
    for chunk in data.chunks(51) {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(chunk).unwrap();
        let compressed = encoder.finish().unwrap();
        let size = 32 + compressed.len();
        let mut h = vec![0; 32];
        h[..4].copy_from_slice(b"LOBJ");
        h[4..6].copy_from_slice(&16u16.to_le_bytes());
        h[6..8].copy_from_slice(&1u16.to_le_bytes());
        h[8..12].copy_from_slice(&(size as u32).to_le_bytes());
        h[12..16].copy_from_slice(&10u32.to_le_bytes());
        h[16..18].copy_from_slice(&2u16.to_le_bytes());
        h[24..28].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        header.extend(h);
        header.extend(compressed);
        header.extend(vec![0; size % 4]);
    }
    let size = header.len() as u64;
    header[16..24].copy_from_slice(&size.to_le_bytes());
    fs::write(path, header).unwrap();
}
fn object(typ: u32, version: u16, flags: u32, tick: u64, body: &[u8]) -> Vec<u8> {
    let hs = if version == 2 { 40 } else { 32 };
    let size = hs + body.len();
    let mut b = vec![0; size];
    b[..4].copy_from_slice(b"LOBJ");
    b[4..6].copy_from_slice(&(hs as u16).to_le_bytes());
    b[6..8].copy_from_slice(&version.to_le_bytes());
    b[8..12].copy_from_slice(&(size as u32).to_le_bytes());
    b[12..16].copy_from_slice(&typ.to_le_bytes());
    b[16..20].copy_from_slice(&flags.to_le_bytes());
    b[24..32].copy_from_slice(&tick.to_le_bytes());
    b[hs..].copy_from_slice(body);
    b
}

#[test]
fn preserve_blf_object_bytes_v2_unknown_time_errors_and_large_objects() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.blf");
    let output = dir.path().join("out.blf");
    let r = dir.path().join("report.json");
    let mut can = [0; 24];
    can[..2].copy_from_slice(&1u16.to_le_bytes());
    can[3] = 1;
    can[4..8].copy_from_slice(&0x123u32.to_le_bytes());
    can[8] = 0x42;
    can[16..].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let objects = vec![
        object(65, 1, 0, 999, &[1, 2, 3]),
        object(86, 2, 1, 12, &can),
        object(73, 1, 2, 130000, &[1, 0, 17, 255, 8, 9, 10, 11]),
        object(999, 1, 2, 140000, &vec![0x5a; 150001]),
    ];
    blf(&input, &objects);
    let result = cli(&[
        "convert",
        path(&input),
        path(&output),
        "--preserve-records",
        "--report",
        path(&r),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(native(&input), native(&output));
    assert_eq!(report(&r)["native_records_written"], 4);
    assert_eq!(report(&r)["issues_preserved"], 3);
    assert_eq!(
        &fs::read(&input).unwrap()[40..72],
        &fs::read(&output).unwrap()[40..72]
    );
    assert_eq!(
        u32::from_le_bytes(fs::read(&output).unwrap()[32..36].try_into().unwrap()),
        4
    );
    let replay = dir.path().join("replay.blf");
    assert!(cli(&[
        "replay",
        path(&output),
        "-o",
        path(&replay),
        "--preserve-records",
        "--no-wait"
    ])
    .status
    .success());
    assert_eq!(native(&input), native(&replay));
}

#[test]
fn preservation_refuses_cross_format_conflicting_filters_and_repeat() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    fs::write(&input, "base hex timestamps absolute\n0.0 SV: event\n").unwrap();
    for (name, extra) in [
        ("x.blf", vec![]),
        ("x.asc", vec!["--id", "1"]),
        ("x.asc", vec!["--direction", "rx"]),
    ] {
        let out = dir.path().join(name);
        let mut args = vec![
            "record",
            "--input",
            path(&input),
            "-o",
            path(&out),
            "--preserve-records",
        ];
        args.extend(extra);
        assert!(!cli(&args).status.success());
        assert!(!out.exists());
    }
    let out = dir.path().join("repeat.asc");
    assert_eq!(
        cli(&[
            "replay",
            path(&input),
            "-o",
            path(&out),
            "--preserve-records",
            "--repeat",
            "2"
        ])
        .status
        .code(),
        Some(2)
    );
    assert!(!out.exists());
    assert_eq!(
        cli(&["view", path(&input), "--preserve-records"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn preservation_does_not_bypass_corrupt_frames_or_mixed_base() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let out = dir.path().join("out.asc");
    for text in ["base hex timestamps absolute\n0.0 1 100 Rx d 2 FF\n", "base hex timestamps absolute\n0.0 SV: first\nbase dec timestamps absolute\n0.1 SV: second\n"] {
        fs::write(&input,text).unwrap(); assert_eq!(cli(&["convert",path(&input),path(&out),"--preserve-records"]).status.code(),Some(1)); assert!(!out.exists());
    }
}

#[test]
fn preservation_time_channel_filters_keep_unknown_coordinates_conservatively() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let out = dir.path().join("out.asc");
    let r = dir.path().join("r.json");
    fs::write(&input,"base hex timestamps absolute\n0.1 1 ErrorFrame\n0.2 SV: global\n0.3 2 ErrorFrame\n0.4 2 100 Rx d 0\n").unwrap();
    assert!(cli(&[
        "filter",
        path(&input),
        "-o",
        path(&out),
        "--preserve-records",
        "--start",
        "0.2",
        "--channel",
        "2",
        "--report",
        path(&r)
    ])
    .status
    .success());
    assert_eq!(native(&out).len(), 3);
    assert_eq!(report(&r)["issues_outside_selection"], 1);
    assert_eq!(report(&r)["issues_preserved"], 2);
}

fn mapping(p: &Path) {
    fs::write(p,r#"{"schema_version":1,"mappings":[{"channel":1,"name":"Stress2","id":291,"extended":false},{"channel":2,"name":"Stress2","id":291,"extended":true},{"channel":1,"name":"FdMessage","id":1193046,"extended":true}]}"#).unwrap();
}

#[test]
fn symbolic_id_mapping_is_channel_scoped_and_covers_classic_and_fd() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let map = dir.path().join("map.json");
    let out = dir.path().join("out.blf");
    let r = dir.path().join("r.json");
    mapping(&map);
    fs::write(&input,"base hex timestamps absolute\n0.0 1 Stress2 Rx d 1 AA\n0.1 2 Stress2 Tx r 9\n0.2 CANFD 1 Rx FdMessage 1 0 0 0\n0.3 1 Stress2 Rx d 0 ID = 256\n0.4 1 FE Rx d 0\n").unwrap();
    let result = cli(&[
        "record",
        "--input",
        path(&input),
        "-o",
        path(&out),
        "--id-map",
        path(&map),
        "--allow-loss",
        "field:format-metadata",
        "--report",
        path(&r),
    ]);
    assert_eq!(
        result.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let frames = cli(&["view", path(&out), "--unlimited"]);
    assert!(frames.status.success());
    let frames: Vec<serde_json::Value> = String::from_utf8(frames.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.len(), 5);
    assert_eq!(frames[0]["frame"]["id"], 291);
    assert_eq!(frames[1]["frame"]["extended"], true);
    assert_eq!(frames[2]["frame"]["id"], 1193046);
    assert_eq!(frames[3]["frame"]["id"], 256);
    assert_eq!(frames[4]["frame"]["id"], 254);
    let r = report(&r);
    assert_eq!(r["metadata"]["mapped_frames"], 3);
    assert_eq!(r["id_map"]["mappings"].as_array().unwrap().len(), 3);
    assert_eq!(r["issues"], 0);
    let filtered = cli(&[
        "view",
        path(&input),
        "--id-map",
        path(&map),
        "--id-kind",
        "extended",
        "--id",
        "291",
    ]);
    assert!(filtered.status.success());
    assert_eq!(
        String::from_utf8(filtered.stdout).unwrap().lines().count(),
        1
    );
}

#[test]
fn mapping_without_assignment_stays_unresolved_and_invalid_can_stays_corrupt() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let map = dir.path().join("map.json");
    mapping(&map);
    for text in [
        "base hex timestamps absolute\n0.0 3 Stress2 Rx d 0\n",
        "base hex timestamps absolute\n0.0 CANFD 1 Rx Missing 0 0 0 0\n",
        "base hex timestamps absolute\n0.0 1 Stress2 Rx d 2 AA\n",
    ] {
        fs::write(&input, text).unwrap();
        assert_eq!(
            cli(&["stats", path(&input), "--id-map", path(&map)])
                .status
                .code(),
            Some(1)
        );
    }
}

#[test]
fn id_map_rejects_invalid_ambiguous_and_oversized_input() {
    let dir = TempDir::new().unwrap();
    let map = dir.path().join("map.json");
    let valid = serde_json::json!({"channel":1,"name":"Message","id":256,"extended":false});
    for records in [
        vec![valid.clone(), valid.clone()],
        vec![serde_json::json!({"channel":0,"name":"Message","id":256,"extended":false})],
        vec![serde_json::json!({"channel":1,"name":"AB","id":256,"extended":false})],
        vec![serde_json::json!({"channel":1,"name":"Message","id":2048,"extended":false})],
    ] {
        fs::write(
            &map,
            serde_json::to_vec(&serde_json::json!({"schema_version":1,"mappings":records}))
                .unwrap(),
        )
        .unwrap();
        assert!(IdMap::load(&map).is_err());
    }
    fs::write(&map, vec![b' '; 262145]).unwrap();
    assert!(IdMap::load(&map).is_err());
    fs::write(&map, r#"{"schema_version":2,"mappings":[]}"#).unwrap();
    assert!(IdMap::load(&map).is_err());
    fs::write(
        &map,
        r#"{"schema_version":1,"schema_version":1,"mappings":[]}"#,
    )
    .unwrap();
    assert!(IdMap::load(&map).is_err());
}

#[test]
fn id_map_input_is_protected_and_invalid_combinations_fail_before_output() {
    let dir = TempDir::new().unwrap();
    let map = dir.path().join("map.json");
    mapping(&map);
    let original = fs::read(&map).unwrap();
    let input = dir.path().join("in.asc");
    fs::write(
        &input,
        "base hex timestamps absolute\n0.0 1 Stress2 Rx d 0\n",
    )
    .unwrap();
    assert_eq!(
        cli(&[
            "record",
            "--input",
            path(&input),
            "-o",
            path(&map),
            "--format",
            "blf",
            "--overwrite",
            "--id-map",
            path(&map)
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read(&map).unwrap(), original);
    let out = dir.path().join("out.asc");
    assert_eq!(
        cli(&[
            "convert",
            path(&input),
            path(&out),
            "--preserve-records",
            "--id-map",
            path(&map)
        ])
        .status
        .code(),
        Some(2)
    );
    assert!(!out.exists());
}

#[test]
fn localized_asc_date_has_explicit_origin_without_guessing_timezone() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("in.asc");
    let out = dir.path().join("out.blf");
    fs::write(
        &input,
        "date Don Nov 24 15:27:14 2005\nbase hex timestamps absolute\n0.0 1 100 Rx d 0\n",
    )
    .unwrap();
    assert!(cli(&["convert", path(&input), path(&out)]).status.success());
    let reader = formats::open_reader(path(&out), None, Limits::default()).unwrap();
    assert_eq!(
        reader.metadata().blf_start,
        Some([2005, 11, 4, 24, 15, 27, 14, 0])
    );
    assert_eq!(
        canlog::core::parse_date("Mit Mär 1 10:00:00 2023"),
        Some([2023, 3, 3, 1, 10, 0, 0, 0])
    );
    assert!(canlog::core::parse_date("Unknown Nov 24 15:27:14 2005").is_none());
}

#[test]
fn native_writer_rejects_invalid_boundaries_time_and_line_injection() {
    let dir = TempDir::new().unwrap();
    let mut writer = formats::make_writer(
        fs::File::create(dir.path().join("out.blf")).unwrap(),
        Format::Blf,
        &Metadata::default(),
    )
    .unwrap();
    assert!(writer
        .write_native(&NativeRecord::Blf {
            timestamp_ns: Some(100),
            object: object(65, 1, 2, 200, &[])
        })
        .is_err());
    assert!(writer
        .write_native(&NativeRecord::Blf {
            timestamp_ns: None,
            object: vec![0; 16]
        })
        .is_err());
    let mut writer = formats::make_writer(
        fs::File::create(dir.path().join("out.asc")).unwrap(),
        Format::Asc,
        &Metadata::default(),
    )
    .unwrap();
    assert!(writer
        .write_native(&NativeRecord::Asc {
            timestamp_ns: 0,
            radix: 16,
            body: "SV: x\nbase dec timestamps absolute".into()
        })
        .is_err());
}

#[test]
fn event_only_preserved_replay_obeys_speed_and_keeps_timestamps() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("events.asc");
    let output = dir.path().join("replayed.asc");
    fs::write(
        &input,
        "base hex timestamps absolute\n0.0 SV: value\n0.4 1 ErrorFrame\n",
    )
    .unwrap();
    let start = std::time::Instant::now();
    let result = cli(&[
        "replay",
        path(&input),
        "-o",
        path(&output),
        "--preserve-records",
        "--speed",
        "2",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(start.elapsed() >= std::time::Duration::from_millis(180));
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(native(&input), native(&output));
}
