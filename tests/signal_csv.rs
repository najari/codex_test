use canlog::{core::*, dbc::Decoder, formats::Format, playback::Cancellation, signal_export};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
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
fn check(out: &std::process::Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn report(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn cell(text: &str) -> Option<String> {
    if text == "\\N" {
        None
    } else {
        Some(
            text.strip_prefix("\\\\")
                .map_or(text, |_| &text[1..])
                .to_owned(),
        )
    }
}
fn csv_rows(bytes: &[u8]) -> Vec<BTreeMap<String, Option<String>>> {
    let mut reader = csv::Reader::from_reader(bytes);
    let headers = reader.headers().unwrap().clone();
    assert_eq!(
        headers.iter().collect::<Vec<_>>(),
        signal_export::CSV_COLUMNS
    );
    reader
        .records()
        .map(|r| {
            headers
                .iter()
                .zip(r.unwrap().iter())
                .map(|(k, v)| (k.to_owned(), cell(v)))
                .collect()
        })
        .collect()
}
fn text<'a>(row: &'a BTreeMap<String, Option<String>>, name: &str) -> &'a str {
    row[name].as_deref().unwrap()
}
fn fixture() -> (TempDir, PathBuf, PathBuf, String) {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("input.asc");
    let db = dir.path().join("typed.dbc");
    fs::write(&db, "VERSION \"\"\nNS_ :\nBS_:\nBU_: Sender Receiver\n\
BO_ 256 Wide: 8 Sender\n SG_ U : 0|64@1+ (1,0) [0|18446744073709551615] \"\" Receiver\n\
BO_ 2147483904 Signed: 8 Sender\n SG_ I : 0|64@1- (1,0) [-9223372036854775808|9223372036854775807] \"\" Receiver\n\
BO_ 258 Mux: 2 Sender\n SG_ Mode M : 0|2@1+ (1,0) [0|3] \"\" Receiver\n SG_ X m1 : 8|8@1+ (0.1,0.2) [0.2|25.7] \"V\" Receiver\n SG_ Y m2 : 15|8@0- (1,0) [-128|127] \"C\" Receiver\n\
BO_ 259 FloatMsg: 8 Sender\n SG_ F : 0|64@1+ (1,0) [0|1] \"\" Receiver\nSIG_VALTYPE_ 259 F : 2;\n").unwrap();
    fs::write(&input, "base hex timestamps absolute\n0 1 100 Rx d 8 FF FF FF FF FF FF FF FF\n0.01 1 100x Tx d 8 00 00 00 00 00 00 00 80\n0.02 1 102 Rx d 2 01 03\n0.03 1 100 Rx r 8\n0.04 2 100 Rx d 8 00 00 00 00 00 00 00 00\n0.05 1 101 Rx d 0\n0.06 1 100 Rx d 1 01\n").unwrap();
    let binding = format!("1={}", path(&db));
    (dir, input, db, binding)
}

#[test]
fn csv_decode_and_replay_preserve_typed_values_quality_and_repeated_locations() {
    let (dir, input, _db, binding) = fixture();
    let a_report = dir.path().join("decode.json");
    let a = cli(&[
        "decode",
        path(&input),
        "--dbc",
        &binding,
        "--format",
        "csv",
        "--report",
        path(&a_report),
    ]);
    check(&a, 3);
    let b = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &binding,
        "--sink",
        "csv",
        "--no-wait",
    ]);
    check(&b, 3);
    assert_eq!(a.stdout, b.stdout);
    let rows = csv_rows(&a.stdout);
    assert_eq!(rows.len(), 9);
    assert_eq!(text(&rows[0], "raw_value"), u64::MAX.to_string());
    assert_eq!(text(&rows[1], "raw_type"), "signed");
    assert_eq!(text(&rows[1], "raw_value"), i64::MIN.to_string());
    assert_eq!(
        text(&rows[3], "physical").parse::<f64>().unwrap().to_bits(),
        (0.1_f64 * 3.0 + 0.2).to_bits()
    );
    assert_eq!(text(&rows[4], "signal_status"), "inactive");
    assert_eq!(rows[4]["raw_value"], None);
    assert_eq!(rows[0]["unit"], Some(String::new()));
    for (r, status) in
        rows[5..]
            .iter()
            .zip(["remote", "no_database", "no_message", "length_mismatch"])
    {
        assert_eq!(text(r, "row_kind"), "frame_status");
        assert_eq!(text(r, "frame_status"), status);
        assert_eq!(r["signal_name"], None);
    }
    let metadata = report(&a_report);
    assert_eq!(metadata["output_rows"], 9);
    assert_eq!(metadata["frames_selected"], 7);
    let limited_report = dir.path().join("limited.json");
    let limited = cli(&[
        "decode",
        path(&input),
        "--dbc",
        &binding,
        "--format",
        "csv",
        "--id",
        "0x102",
        "--limit",
        "1",
        "--report",
        path(&limited_report),
    ]);
    check(&limited, 0);
    assert_eq!(csv_rows(&limited.stdout).len(), 3);
    assert_eq!(report(&limited_report)["frames_selected"], 1);
    assert_eq!(report(&limited_report)["output_rows"], 3);
    assert_eq!(metadata["export_schema"]["id"], "canlog-signal-csv-v1");
    assert_eq!(
        metadata["export_schema"]["columns"]
            .as_array()
            .unwrap()
            .len(),
        33
    );
    assert!(a.stdout.windows(2).filter(|w| *w == b"\r\n").count() == 10);
    let repeated = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &binding,
        "--sink",
        "csv",
        "--no-wait",
        "--repeat",
        "2",
        "--repeat-gap",
        "0.001",
    ]);
    check(&repeated, 3);
    let repeated = csv_rows(&repeated.stdout);
    assert_eq!(repeated.len(), 18);
    for (first, second) in repeated[..9].iter().zip(&repeated[9..]) {
        let mut expected = first.clone();
        expected.insert(
            "timestamp_ns".into(),
            Some((text(first, "timestamp_ns").parse::<i64>().unwrap() + 61_000_000).to_string()),
        );
        assert_eq!(&expected, second);
    }
}

#[test]
fn escaping_unicode_multiline_literal_null_and_float_bits_use_independent_csv_reader() {
    let (_dir, _input, db, binding) = fixture();
    let original = fs::read(&db).unwrap();
    let mut decoder = Decoder::load(&[binding], &Cancellation::default()).unwrap();
    let frame = Frame::new(
        9_007_199_254_740_993,
        1,
        259,
        false,
        Direction::Unknown,
        false,
        false,
        8,
        0x7ff8_0000_0000_0021_u64.to_le_bytes().to_vec(),
        false,
        false,
    )
    .unwrap();
    let mut decoded = decoder
        .decode(FrameRecord {
            frame,
            location: Location {
                source: "\\N\r\n한국어,\"source\"".into(),
                ordinal: u64::MAX,
                line: Some(u64::MAX),
                container: None,
                object_offset: Some(u64::MAX),
            },
            native: None,
        })
        .unwrap();
    decoded.signals[0].unit = "\\N".into();
    decoded.signals[0].description = "한국어,\"quoted\"\nsecond\r\nthird".into();
    decoded.error = Some(String::new());
    let mut bytes = vec![];
    signal_export::write_header(&mut bytes, Format::Csv).unwrap();
    assert_eq!(
        signal_export::write_frame(&mut bytes, &decoded, Format::Csv).unwrap(),
        1
    );
    let rows = csv_rows(&bytes);
    assert_eq!(rows.len(), 1);
    assert_eq!(text(&rows[0], "timestamp_ns"), "9007199254740993");
    assert_eq!(text(&rows[0], "ordinal"), u64::MAX.to_string());
    assert_eq!(text(&rows[0], "object_offset"), u64::MAX.to_string());
    assert_eq!(text(&rows[0], "source"), decoded.record.location.source);
    assert_eq!(text(&rows[0], "unit"), "\\N");
    assert_eq!(
        text(&rows[0], "description"),
        decoded.signals[0].description
    );
    assert_eq!(text(&rows[0], "raw_type"), "float_bits");
    assert_eq!(
        text(&rows[0], "raw_value"),
        0x7ff8_0000_0000_0021_u64.to_string()
    );
    assert_eq!(text(&rows[0], "signal_status"), "non_finite");
    assert_eq!(rows[0]["physical"], None);
    assert_eq!(rows[0]["error"], Some(String::new()));
    assert_eq!(rows[0]["container"], None);
    assert_eq!(fs::read(db).unwrap(), original);
    decoded.signals[0].physical = Some(-0.0);
    let mut bytes = vec![];
    signal_export::write_header(&mut bytes, Format::Csv).unwrap();
    signal_export::write_frame(&mut bytes, &decoded, Format::Csv).unwrap();
    assert_eq!(
        text(&csv_rows(&bytes)[0], "physical")
            .parse::<f64>()
            .unwrap()
            .to_bits(),
        (-0.0_f64).to_bits()
    );
}

#[test]
fn inferred_csv_file_explicit_jsonl_override_and_filtered_index_match() {
    let (dir, input, _db, binding) = fixture();
    let output = dir.path().join("signals.csv");
    let metadata = dir.path().join("signals.metadata.json");
    let index = dir.path().join("input.sqlite");
    check(
        &cli(&[
            "index",
            "build",
            path(&input),
            "-o",
            path(&index),
            "--stride",
            "1",
        ]),
        0,
    );
    let base = [
        "decode",
        path(&input),
        "--dbc",
        &binding,
        "--index",
        path(&index),
        "--id-kind",
        "extended",
    ];
    let out = cli(&[
        base.as_slice(),
        &["-o", path(&output), "--report", path(&metadata)],
    ]
    .concat());
    check(&out, 0);
    assert!(out.stdout.is_empty());
    assert_eq!(csv_rows(&fs::read(&output).unwrap()).len(), 1);
    assert!(report(&metadata)["published"].as_bool().unwrap());
    let stdout = cli(&[base.as_slice(), &["--format", "csv"]].concat());
    check(&stdout, 0);
    assert_eq!(stdout.stdout, fs::read(&output).unwrap());
    check(&cli(&[base.as_slice(), &["-o", path(&output)]].concat()), 1);
    let replacement = cli(&[
        base.as_slice(),
        &["-o", path(&output), "--overwrite", "--format", "jsonl"],
    ]
    .concat());
    check(&replacement, 0);
    assert!(fs::read(&output)
        .unwrap()
        .starts_with(b"{\"schema_version\""));
    let replayed = dir.path().join("replay.csv");
    check(
        &cli(&[
            "replay",
            path(&input),
            "--dbc",
            &binding,
            "--id-kind",
            "extended",
            "--no-wait",
            "-o",
            path(&replayed),
        ]),
        0,
    );
    assert_eq!(stdout.stdout, fs::read(replayed).unwrap());
}

#[test]
fn workspace_jsonl_and_csv_share_cache_without_changing_values_or_bindings() {
    let (dir, input, _db, binding) = fixture();
    let root = dir.path().join("workspace");
    check(&cli(&["workspace", "create", path(&root)]), 0);
    check(
        &cli(&[
            "workspace",
            "add",
            path(&root),
            path(&input),
            "--name",
            "log",
        ]),
        0,
    );
    check(
        &cli(&["workspace", "bind", path(&root), "log", "--dbc", &binding]),
        0,
    );
    check(
        &cli(&["workspace", "index", path(&root), "log", "--stride", "1"]),
        0,
    );
    let manifest = fs::read(root.join("workspace.json")).unwrap();
    let cold_report = dir.path().join("cold.json");
    check(
        &cli(&[
            "workspace",
            "decode",
            path(&root),
            "log",
            "--report",
            path(&cold_report),
        ]),
        3,
    );
    let warm_report = dir.path().join("warm.json");
    let warm = cli(&[
        "workspace",
        "decode",
        path(&root),
        "log",
        "--format",
        "csv",
        "--report",
        path(&warm_report),
    ]);
    check(&warm, 3);
    let bypass = cli(&[
        "workspace",
        "decode",
        path(&root),
        "log",
        "--format",
        "csv",
        "--no-cache",
    ]);
    check(&bypass, 3);
    assert_eq!(warm.stdout, bypass.stdout);
    assert_eq!(csv_rows(&warm.stdout).len(), 9);
    assert_eq!(
        report(&cold_report)["cache"]["key"],
        report(&warm_report)["cache"]["key"]
    );
    assert_eq!(report(&warm_report)["cache"]["hits"], 7);
    assert_eq!(report(&warm_report)["cache"]["misses"], 0);
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), manifest);
}

#[test]
fn empty_csv_has_schema_header_and_zero_data_rows() {
    let (dir, input, _db, binding) = fixture();
    fs::write(&input, "base hex timestamps absolute\n").unwrap();
    let metadata = dir.path().join("empty.json");
    let out = cli(&[
        "decode",
        path(&input),
        "--dbc",
        &binding,
        "--format",
        "csv",
        "--report",
        path(&metadata),
    ]);
    check(&out, 0);
    assert_eq!(
        out.stdout,
        format!("{}\r\n", signal_export::CSV_COLUMNS.join(",")).as_bytes()
    );
    assert_eq!(report(&metadata)["output_rows"], 0);
    let replay = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &binding,
        "--sink",
        "csv",
        "--no-wait",
    ]);
    check(&replay, 0);
    assert_eq!(out.stdout, replay.stdout);
}

#[test]
fn invalid_projection_protected_alias_and_strict_issue_do_not_publish_csv() {
    let (dir, input, db, binding) = fixture();
    check(
        &cli(&["replay", path(&input), "--sink", "csv", "--no-wait"]),
        2,
    );
    let output = dir.path().join("signals.csv");
    check(
        &cli(&[
            "decode",
            path(&input),
            "--dbc",
            &binding,
            "--format",
            "blf",
            "-o",
            path(&output),
        ]),
        2,
    );
    assert!(!output.exists());
    let rejected_query = cli(&[
        "index",
        "query",
        path(&input),
        "--index",
        path(&dir.path().join("unused.sqlite")),
        "--format",
        "csv",
        "-o",
        path(&output),
    ]);
    check(&rejected_query, 1);
    assert!(String::from_utf8_lossy(&rejected_query.stderr).contains("signal CSV requires DBC"));
    assert!(!output.exists());
    let alias = dir.path().join("alias.csv");
    fs::hard_link(&db, &alias).unwrap();
    let original = fs::read(&db).unwrap();
    check(
        &cli(&[
            "decode",
            path(&input),
            "--dbc",
            &binding,
            "-o",
            path(&alias),
            "--overwrite",
        ]),
        1,
    );
    check(
        &cli(&[
            "replay",
            path(&input),
            "--dbc",
            &binding,
            "--no-wait",
            "-o",
            path(&alias),
            "--overwrite",
        ]),
        1,
    );
    assert_eq!(fs::read(alias).unwrap(), original);
    fs::write(
        &input,
        "base hex timestamps absolute\n0 1 100 Rx d 8 FF FF FF FF FF FF FF FF\n0.1 SV: event\n",
    )
    .unwrap();
    check(
        &cli(&[
            "decode",
            path(&input),
            "--dbc",
            &binding,
            "-o",
            path(&output),
        ]),
        1,
    );
    assert!(!output.exists());
    check(
        &cli(&[
            "replay",
            path(&input),
            "--dbc",
            &binding,
            "--no-wait",
            "-o",
            path(&output),
        ]),
        1,
    );
    assert!(!output.exists());
    assert_eq!(fs::read(db).unwrap(), original);
}

#[test]
fn cancelling_csv_replay_keeps_previous_output() {
    let (dir, input, _db, binding) = fixture();
    fs::write(&input, "base hex timestamps absolute\n0 1 100 Rx d 8 FF FF FF FF FF FF FF FF\n3600 1 100 Rx d 8 FF FF FF FF FF FF FF FF\n").unwrap();
    let output = dir.path().join("signals.csv");
    fs::write(&output, "previous").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args([
            "replay",
            path(&input),
            "--dbc",
            &binding,
            "-o",
            path(&output),
            "--overwrite",
            "--control-stdin",
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let stopped = Instant::now();
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    check(&child.wait_with_output().unwrap(), 130);
    assert!(stopped.elapsed() < Duration::from_secs(2));
    assert_eq!(fs::read(output).unwrap(), b"previous");
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".partial")));
}

#[test]
fn csv_export_propagates_sink_errors_and_rejects_nonfinite_physical_values() {
    struct Fail;
    impl Write for Fail {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("injected write failure"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (_dir, input, _db, binding) = fixture();
    let decoded = cli(&["decode", path(&input), "--dbc", &binding, "--limit", "1"]);
    check(&decoded, 0);
    let mut decoder = Decoder::load(&[binding], &Cancellation::default()).unwrap();
    let frame = Frame::new(
        0,
        1,
        256,
        false,
        Direction::Rx,
        false,
        false,
        8,
        vec![255; 8],
        false,
        false,
    )
    .unwrap();
    let mut decoded = decoder
        .decode(FrameRecord {
            frame,
            location: Location::default(),
            native: None,
        })
        .unwrap();
    assert!(signal_export::write_header(&mut Fail, Format::Csv)
        .unwrap_err()
        .to_string()
        .contains("injected"));
    assert!(signal_export::write_frame(&mut Fail, &decoded, Format::Csv)
        .unwrap_err()
        .to_string()
        .contains("injected"));
    decoded.signals[0].physical = Some(f64::INFINITY);
    assert!(
        signal_export::write_frame(&mut Vec::new(), &decoded, Format::Csv)
            .unwrap_err()
            .to_string()
            .contains("finite")
    );
}
