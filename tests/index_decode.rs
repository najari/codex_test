use canlog::{
    core::*,
    dbc::Decoder,
    formats::{self, Format},
    index,
    playback::Cancellation,
};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::Command,
};
use tempfile::TempDir;

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(args)
        .output()
        .unwrap()
}
fn success(out: &std::process::Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn build(input: &Path, dest: &Path, stride: u64) {
    success(
        &cli(&[
            "index",
            "build",
            path(input),
            "-o",
            path(dest),
            "--stride",
            &stride.to_string(),
            "--unsupported",
            "skip",
            "--recover",
        ]),
        0,
    );
}
fn dbc(p: &Path, body: &str) {
    fs::write(
        p,
        format!("VERSION \"\"\nNS_ :\nBS_:\nBU_: Sender Receiver\n{body}\n"),
    )
    .unwrap();
}
fn frame(id: u32, extended: bool, fd: bool, data: Vec<u8>) -> FrameRecord {
    let dlc = if fd {
        FD_LENGTHS.iter().position(|&n| n == data.len()).unwrap() as u8
    } else {
        data.len() as u8
    };
    FrameRecord {
        frame: Frame::new(
            1,
            1,
            id,
            extended,
            Direction::Rx,
            false,
            fd,
            dlc,
            data,
            false,
            false,
        )
        .unwrap(),
        location: Location::default(),
        native: None,
    }
}

#[test]
fn asc_relative_seek_matches_full_scan_and_half_open_selection() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("relative.asc");
    let db = d.path().join("index.sqlite");
    let mut text =
        "base hex timestamps relative\nBegin Triggerblock\n0.4 Start of measurement\n".to_string();
    for _ in 0..1000 {
        text.push_str("0.001 1 100 Rx d 1 AA\n");
    }
    fs::write(&source, text).unwrap();
    build(&source, &db, 25);
    let indexed = cli(&[
        "index",
        "query",
        path(&source),
        "--index",
        path(&db),
        "--start",
        "0.901",
        "--end",
        "0.906",
    ]);
    let full = cli(&[
        "view",
        path(&source),
        "--start",
        "0.901",
        "--end",
        "0.906",
        "--unlimited",
    ]);
    success(&indexed, 0);
    success(&full, 0);
    assert_eq!(rows(&indexed.stdout), rows(&full.stdout));
    assert_eq!(rows(&indexed.stdout).len(), 5);
    assert!(String::from_utf8_lossy(&indexed.stderr).contains("examined=25"));
    let c = rusqlite::Connection::open(&db).unwrap();
    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(tables, ["chunks", "issues", "manifest"]);
}

#[test]
fn regressing_equal_timestamps_and_unknown_issue_quality_match() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("unordered.asc");
    let db = d.path().join("index.sqlite");
    fs::write(&source, "base hex timestamps absolute\n0.9 1 100 Rx d 1 01\n0.1 SV: event\n0.4 2 101 Tx d 1 02\n0.4 1 100 Rx d 1 03\nbad unknown timestamp\n0.3 1 100 Rx d 1 04\n").unwrap();
    success(
        &cli(&[
            "index",
            "build",
            path(&source),
            "-o",
            path(&db),
            "--stride",
            "1",
            "--unsupported",
            "skip",
            "--recover",
        ]),
        3,
    );
    let out = cli(&[
        "index",
        "query",
        path(&source),
        "--index",
        path(&db),
        "--start",
        "0.3",
        "--end",
        "0.5",
        "--channel",
        "1",
        "--unsupported",
        "skip",
        "--recover",
    ]);
    let full = cli(&[
        "view",
        path(&source),
        "--start",
        "0.3",
        "--end",
        "0.5",
        "--channel",
        "1",
        "--unsupported",
        "skip",
        "--recover",
        "--unlimited",
    ]);
    success(&out, 3);
    success(&full, 3);
    assert_eq!(rows(&out.stdout), rows(&full.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).contains("issues=1"));
    success(
        &cli(&[
            "index",
            "query",
            path(&source),
            "--index",
            path(&db),
            "--start",
            "0.3",
            "--end",
            "0.5",
        ]),
        1,
    );
}

#[test]
fn index_rejects_same_size_source_change_settings_and_future_schema() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source.asc");
    let db = d.path().join("index.sqlite");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.1 1 100 Rx d 1 AA\n",
    )
    .unwrap();
    build(&source, &db, 1);
    let wrong = cli(&[
        "index",
        "query",
        path(&source),
        "--index",
        path(&db),
        "--max-line-bytes",
        "1000",
    ]);
    success(&wrong, 1);
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("parser limits"));
    fs::write(
        &source,
        "base hex timestamps absolute\n0.1 1 100 Rx d 1 BB\n",
    )
    .unwrap();
    let stale = cli(&["index", "query", path(&source), "--index", path(&db)]);
    success(&stale, 1);
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale index"));
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    let old = fs::read(&db).unwrap();
    success(&cli(&["index", "info", path(&db)]), 1);
    assert_eq!(fs::read(&db).unwrap(), old);
}

#[test]
fn failed_build_preserves_existing_index_and_no_partial_is_published() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("source.asc");
    let db = d.path().join("index.sqlite");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.1 1 100 Rx d 1 AA\n",
    )
    .unwrap();
    build(&source, &db, 1);
    let before = fs::read(&db).unwrap();
    fs::write(
        &source,
        "base hex timestamps relative\n0.1 1 100 Rx d 1 AA\nbad SV: event\n",
    )
    .unwrap();
    success(
        &cli(&[
            "index",
            "build",
            path(&source),
            "-o",
            path(&db),
            "--overwrite",
            "--recover",
            "--unsupported",
            "skip",
            "--stride",
            "1",
        ]),
        1,
    );
    assert_eq!(fs::read(&db).unwrap(), before);
    assert!(fs::read_dir(d.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".partial")));
    let mut reader = formats::open_reader(path(&source), None, Limits::default()).unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    // Cancellation is checked before a new generation is created.
    let args = canlog::app::InputArgs {
        input: path(&source).into(),
        input_format: None,
        id_map: None,
        preserve_records: false,
        unsupported: canlog::app::Unsupported::Skip,
        recover: true,
        channel: vec![],
        id: vec![],
        id_kind: None,
        direction: None,
        start: None,
        end: None,
        limit: None,
        report: None,
        max_line_bytes: 65536,
        max_object_bytes: 1048576,
        max_container_bytes: 8388608,
    };
    assert!(index::build(&args, &db, true, 1, &cancel)
        .unwrap_err()
        .is::<canlog::playback::Cancelled>());
    assert_eq!(fs::read(&db).unwrap(), before);
    assert!(reader.next_item().unwrap().is_some());
}

fn container_file(p: &Path, containers: &[Vec<u8>]) {
    let mut file = vec![0; 144];
    file[..4].copy_from_slice(b"LOGG");
    file[4..8].copy_from_slice(&144u32.to_le_bytes());
    for data in containers {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        let bytes = enc.finish().unwrap();
        let size = 32 + bytes.len();
        let mut header = vec![0; 32];
        header[..4].copy_from_slice(b"LOBJ");
        header[4..6].copy_from_slice(&16u16.to_le_bytes());
        header[6..8].copy_from_slice(&1u16.to_le_bytes());
        header[8..12].copy_from_slice(&(size as u32).to_le_bytes());
        header[12..16].copy_from_slice(&10u32.to_le_bytes());
        header[16..18].copy_from_slice(&2u16.to_le_bytes());
        header[24..28].copy_from_slice(&(data.len() as u32).to_le_bytes());
        file.extend(header);
        file.extend(bytes);
        file.extend(vec![0; size % 4]);
    }
    let size = file.len() as u64;
    file[16..24].copy_from_slice(&size.to_le_bytes());
    fs::write(p, file).unwrap();
}

#[test]
fn blf_seek_across_every_object_split_matches_locations_and_frames() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("input.blf");
    let db = d.path().join("index.sqlite");
    let original = d.path().join("original.blf");
    let mut writer = formats::make_writer(
        fs::File::create(&original).unwrap(),
        Format::Blf,
        &Metadata::default(),
    )
    .unwrap();
    for t in 1..=3 {
        writer
            .write_frame(
                &Frame::new(
                    t,
                    1,
                    0x100,
                    false,
                    Direction::Rx,
                    false,
                    false,
                    1,
                    vec![t as u8],
                    false,
                    false,
                )
                .unwrap(),
            )
            .unwrap();
    }
    writer.finish().unwrap();
    drop(writer);
    let bytes = fs::read(original).unwrap();
    let size = u32::from_le_bytes(bytes[152..156].try_into().unwrap()) as usize;
    let mut objects = vec![];
    flate2::read::ZlibDecoder::new(&bytes[176..144 + size])
        .read_to_end(&mut objects)
        .unwrap();
    for split in 1..96 {
        container_file(
            &source,
            &[
                objects[..split].to_vec(),
                objects[split..96].to_vec(),
                objects[96..].to_vec(),
            ],
        );
        success(
            &cli(&[
                "index",
                "build",
                path(&source),
                "-o",
                path(&db),
                "--overwrite",
                "--stride",
                "1",
            ]),
            0,
        );
        let out = cli(&[
            "index",
            "query",
            path(&source),
            "--index",
            path(&db),
            "--start",
            "0.000000002",
        ]);
        let full = cli(&[
            "view",
            path(&source),
            "--start",
            "0.000000002",
            "--unlimited",
        ]);
        success(&out, 0);
        success(&full, 0);
        assert_eq!(rows(&out.stdout), rows(&full.stdout), "split {split}");
    }
}

#[test]
fn symbolic_id_mapping_is_bound_to_index_identity() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("input.asc");
    let map = d.path().join("map.json");
    let db = d.path().join("index.sqlite");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.1 1 Symbol Rx d 1 AA\n0.2 1 Symbol Rx d 1 BB\n",
    )
    .unwrap();
    fs::write(&map,r#"{"schema_version":1,"mappings":[{"channel":1,"name":"Symbol","id":256,"extended":false}]}"#).unwrap();
    success(
        &cli(&[
            "index",
            "build",
            path(&source),
            "-o",
            path(&db),
            "--id-map",
            path(&map),
            "--stride",
            "1",
        ]),
        0,
    );
    let out = cli(&[
        "index",
        "query",
        path(&source),
        "--index",
        path(&db),
        "--id-map",
        path(&map),
        "--start",
        "0.2",
    ]);
    success(&out, 0);
    assert_eq!(rows(&out.stdout)[0]["frame"]["id"], 256);
    fs::write(&map,r#"{"schema_version":1,"mappings":[{"channel":1,"name":"Symbol","id":257,"extended":false}]}"#).unwrap();
    success(
        &cli(&[
            "index",
            "query",
            path(&source),
            "--index",
            path(&db),
            "--id-map",
            path(&map),
        ]),
        1,
    );
}

#[test]
fn typed_u64_signed_motorola_and_extended_id_are_preserved() {
    let d = TempDir::new().unwrap();
    let db = d.path().join("typed.dbc");
    dbc(&db,"BO_ 256 Wide: 8 Sender\n SG_ Unsigned : 0|64@1+ (1,0) [0|18446744073709551615] \"\" Receiver\n\
BO_ 2147483904 Signed: 8 Sender\n SG_ Signed : 0|64@1- (1,0) [-9223372036854775808|9223372036854775807] \"\" Receiver\n\
BO_ 257 Motorola: 2 Sender\n SG_ BigEndian : 7|16@0+ (0.5,-1) [0|65535] \"V\" Receiver");
    let mut decoder =
        Decoder::load(&[format!("1={}", path(&db))], &Cancellation::default()).unwrap();
    let wide = serde_json::to_value(
        decoder
            .decode(frame(256, false, false, vec![255; 8]))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(wide["signals"][0]["raw"]["value"].as_u64(), Some(u64::MAX));
    assert_eq!(wide["signals"][0]["raw_text"], u64::MAX.to_string());
    let signed = serde_json::to_value(
        decoder
            .decode(frame(256, true, false, vec![255; 8]))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(signed["message"], "Signed");
    assert_eq!(signed["signals"][0]["raw"]["value"], -1);
    let big = serde_json::to_value(
        decoder
            .decode(frame(257, false, false, vec![0x12, 0x34]))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(big["signals"][0]["raw"]["value"], 4660);
    assert_eq!(big["signals"][0]["physical"], 2329.0);
}

#[test]
fn multiplex_inactive_length_remote_and_unassigned_have_distinct_status() {
    let d = TempDir::new().unwrap();
    let db = d.path().join("mux.dbc");
    dbc(&db,"BO_ 256 MuxMsg: 2 Sender\n SG_ Selector M : 0|8@1+ (1,0) [0|255] \"\" Receiver\n SG_ A m1 : 8|8@1+ (1,0) [0|255] \"\" Receiver\n SG_ B m2 : 8|8@1+ (1,0) [0|255] \"\" Receiver");
    let mut decoder =
        Decoder::load(&[format!("1={}", path(&db))], &Cancellation::default()).unwrap();
    let decoded = serde_json::to_value(
        decoder
            .decode(frame(256, false, false, vec![1, 42]))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(decoded["signals"][1]["status"], "valid");
    assert_eq!(decoded["signals"][2]["status"], "inactive");
    assert_eq!(
        decoder
            .decode(frame(256, false, false, vec![1]))
            .unwrap()
            .status,
        "length_mismatch"
    );
    assert_eq!(
        decoder
            .decode(frame(257, false, false, vec![1, 2]))
            .unwrap()
            .status,
        "no_message"
    );
    let mut other = frame(256, false, false, vec![1, 2]);
    other.frame = Frame::new(
        1,
        2,
        256,
        false,
        Direction::Rx,
        false,
        false,
        2,
        vec![1, 2],
        false,
        false,
    )
    .unwrap();
    assert_eq!(decoder.decode(other).unwrap().status, "no_database");
    let remote = FrameRecord {
        frame: Frame::new(
            1,
            1,
            256,
            false,
            Direction::Rx,
            true,
            false,
            2,
            vec![],
            false,
            false,
        )
        .unwrap(),
        location: Location::default(),
        native: None,
    };
    assert_eq!(decoder.decode(remote).unwrap().status, "remote");
}

#[test]
fn fd_64_byte_payload_and_non_finite_float_bits() {
    let d = TempDir::new().unwrap();
    let db = d.path().join("fd.dbc");
    dbc(&db,"BO_ 256 Long: 64 Sender\n SG_ Tail : 504|8@1+ (1,0) [0|255] \"\" Receiver\n\
BO_ 257 FloatMsg: 8 Sender\n SG_ Float : 0|64@1+ (1,0) [0|1] \"\" Receiver\nSIG_VALTYPE_ 257 Float : 2;\nBA_DEF_ BO_ \"VFrameFormat\" ENUM \"StandardCAN\",\"ExtendedCAN\",\"StandardCAN_FD\";\nBA_ \"VFrameFormat\" BO_ 256 2;");
    let mut decoder =
        Decoder::load(&[format!("1={}", path(&db))], &Cancellation::default()).unwrap();
    let mut data = vec![0; 64];
    data[63] = 42;
    let decoded =
        serde_json::to_value(decoder.decode(frame(256, false, true, data)).unwrap()).unwrap();
    assert_eq!(decoded["signals"][0]["raw"]["value"], 42);
    let nan = serde_json::to_value(
        decoder
            .decode(frame(257, false, false, f64::NAN.to_le_bytes().to_vec()))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(nan["signals"][0]["status"], "non_finite");
    assert!(nan["signals"][0]["physical"].is_null());
    assert_eq!(
        nan["signals"][0]["raw"]["value"].as_u64(),
        Some(f64::NAN.to_bits())
    );
    assert_eq!(
        decoder
            .decode(frame(256, false, false, vec![0; 8]))
            .unwrap()
            .status,
        "format_mismatch"
    );
}

#[test]
fn ambiguous_invalid_and_changed_databases_are_rejected() {
    let d = TempDir::new().unwrap();
    let db = d.path().join("valid.dbc");
    dbc(
        &db,
        "BO_ 256 Msg: 1 Sender\n SG_ X : 0|8@1+ (1,0) [0|255] \"\" Receiver",
    );
    let binding = format!("1={}", path(&db));
    let cancel = Cancellation::default();
    assert!(Decoder::load(&[binding.clone(), binding.clone()], &cancel).is_err());
    let decoder = Decoder::load(std::slice::from_ref(&binding), &cancel).unwrap();
    dbc(
        &db,
        "BO_ 256 Msg: 1 Sender\n SG_ X : 0|8@1+ (1,1) [0|255] \"\" Receiver",
    );
    assert!(decoder.verify_databases(&cancel).is_err());
    dbc(&db,"BO_ 256 Msg: 1 Sender\n SG_ X : 0|8@1+ (1,0) [0|255] \"\" Receiver\n SG_ X : 0|8@1+ (1,0) [0|255] \"\" Receiver");
    assert!(Decoder::load(std::slice::from_ref(&binding), &cancel).is_err());
    dbc(
        &db,
        "BO_ 65536 Msg: 1 Sender\n SG_ X : 0|8@1+ (1,0) [0|255] \"\" Receiver",
    );
    assert!(Decoder::load(&[binding], &cancel).is_err());
}

#[test]
fn cli_decode_index_equivalence_atomic_outputs_and_quality() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("in.asc");
    let dbc_path = d.path().join("db.dbc");
    let index_path = d.path().join("in.sqlite");
    dbc(
        &dbc_path,
        "BO_ 256 Msg: 1 Sender\n SG_ X : 0|8@1+ (2,1) [0|511] \"V\" Receiver",
    );
    let binding = format!("1={}", path(&dbc_path));
    fs::write(&source,"base hex timestamps absolute\n0.1 1 100 Rx d 1 01\n0.2 1 100 Rx d 1 02\n0.3 1 101 Rx d 1 03\n").unwrap();
    build(&source, &index_path, 1);
    let full = cli(&["decode", path(&source), "--dbc", &binding]);
    let indexed = cli(&[
        "decode",
        path(&source),
        "--dbc",
        &binding,
        "--index",
        path(&index_path),
    ]);
    success(&full, 3);
    success(&indexed, 3);
    assert_eq!(rows(&full.stdout), rows(&indexed.stdout));
    assert_eq!(rows(&full.stdout)[0]["signals"][0]["physical"], 3.0);
    assert_eq!(rows(&full.stdout)[2]["status"], "no_message");
    let old = fs::read(&dbc_path).unwrap();
    success(
        &cli(&[
            "decode",
            path(&source),
            "--dbc",
            &binding,
            "-o",
            path(&dbc_path),
            "--overwrite",
        ]),
        1,
    );
    assert_eq!(fs::read(&dbc_path).unwrap(), old);
    let source_bytes = fs::read(&source).unwrap();
    success(
        &cli(&[
            "index",
            "build",
            path(&source),
            "-o",
            path(&source),
            "--overwrite",
        ]),
        1,
    );
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    let out = d.path().join("decoded.jsonl");
    let report = d.path().join("report.json");
    success(
        &cli(&[
            "decode",
            path(&source),
            "--dbc",
            &binding,
            "-o",
            path(&out),
            "--report",
            path(&report),
            "--limit",
            "1",
        ]),
        0,
    );
    let r: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(r["selection_complete"], false);
    assert_eq!(r["published"], true);
    assert_eq!(rows(&fs::read(out).unwrap()).len(), 1);
}

#[test]
fn explicit_fd_frame_format_requires_fd_even_for_short_payload() {
    let d = TempDir::new().unwrap();
    let db = d.path().join("fd.dbc");
    dbc(&db,"BO_ 256 Msg: 1 Sender\n SG_ X : 0|8@1+ (1,0) [0|255] \"\" Receiver\nBA_DEF_ BO_ \"VFrameFormat\" ENUM \"StandardCAN\",\"ExtendedCAN\",\"StandardCAN_FD\";\nBA_ \"VFrameFormat\" BO_ 256 2;");
    let mut decoder =
        Decoder::load(&[format!("1={}", path(&db))], &Cancellation::default()).unwrap();
    assert_eq!(
        decoder
            .decode(frame(256, false, false, vec![1]))
            .unwrap()
            .status,
        "format_mismatch"
    );
    assert_eq!(
        decoder
            .decode(frame(256, false, true, vec![1]))
            .unwrap()
            .status,
        "decoded"
    );
}

#[test]
fn extended_mux_ranges_use_engine_and_warning_labels_remain_values() {
    let d = TempDir::new().unwrap();
    let db = d.path().join("extended-mux.dbc");
    dbc(&db, "BO_ 256 Msg: 2 Sender\n SG_ Selector M : 0|8@1+ (1,0) [0|255] \"\" Receiver\n SG_ X m1 : 8|8@1+ (1,0) [0|255] \"\" Receiver\nSG_MUL_VAL_ 256 X Selector 3-5;\nVAL_ 256 X 42 \"⚠ label\";");
    let mut decoder =
        Decoder::load(&[format!("1={}", path(&db))], &Cancellation::default()).unwrap();
    let active = serde_json::to_value(
        decoder
            .decode(frame(256, false, false, vec![4, 42]))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active["signals"][1]["status"], "valid");
    assert_eq!(active["signals"][1]["description"], "⚠ label");
    let inactive = serde_json::to_value(
        decoder
            .decode(frame(256, false, false, vec![1, 42]))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(inactive["signals"][1]["status"], "inactive");
}

#[test]
fn report_alias_and_index_overwrite_are_rejected_before_publication() {
    let d = TempDir::new().unwrap();
    let source = d.path().join("input.asc");
    let db = d.path().join("input.sqlite");
    fs::write(
        &source,
        "base hex timestamps absolute\n0.1 1 100 Rx d 1 AA\n",
    )
    .unwrap();
    let alias = d.path().join(".").join("input.sqlite");
    success(
        &cli(&[
            "index",
            "build",
            path(&source),
            "-o",
            path(&db),
            "--report",
            path(&alias),
        ]),
        1,
    );
    assert!(!db.exists());
    build(&source, &db, 1);
    let before = fs::read(&db).unwrap();
    success(
        &cli(&[
            "index",
            "query",
            path(&source),
            "--index",
            path(&db),
            "-o",
            path(&db),
            "--overwrite",
        ]),
        1,
    );
    assert_eq!(fs::read(&db).unwrap(), before);
    let out = d.path().join("frames.jsonl");
    let out_alias = d.path().join(".").join("frames.jsonl");
    success(
        &cli(&[
            "index",
            "query",
            path(&source),
            "--index",
            path(&db),
            "-o",
            path(&out),
            "--report",
            path(&out_alias),
        ]),
        1,
    );
    assert!(!out.exists());
    #[cfg(windows)]
    {
        let case_alias = d.path().join("FRAMES.JSONL");
        success(
            &cli(&[
                "index",
                "query",
                path(&source),
                "--index",
                path(&db),
                "-o",
                path(&out),
                "--report",
                path(&case_alias),
            ]),
            1,
        );
        assert!(!out.exists());
    }
}
