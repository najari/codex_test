use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
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
fn rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn fixture() -> (TempDir, std::path::PathBuf, String, String) {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("input.asc");
    fs::write(
        &input,
        "base hex timestamps absolute\n0.01 1 100 Rx d 2 01 28\n0.03 2 100 Tx d 2 02 FF\n",
    )
    .unwrap();
    let db = dir.path().join("drive.dbc");
    fs::write(&db, "VERSION \"\"\nNS_ :\nBS_:\nBU_: Sender Receiver\nBO_ 256 Drive: 2 Sender\n SG_ Mode M : 0|2@1+ (1,0) [0|3] \"\" Receiver\n SG_ Speed m1 : 8|8@1+ (0.5,-10) [-10|117.5] \"km/h\" Receiver\n SG_ Temp m2 : 15|8@0- (1,0) [-128|127] \"C\" Receiver\nVAL_ 256 Mode 1 \"Driving\" 2 \"Heating\";\n").unwrap();
    let one = format!("1={}", path(&db));
    let two = format!("2={}", path(&db));
    (dir, input, one, two)
}

#[test]
fn replay_uses_same_typed_codec_as_decode_and_keeps_repeat_provenance() {
    let (dir, input, one, two) = fixture();
    let decode = cli(&["decode", path(&input), "--dbc", &one, "--dbc", &two]);
    check(&decode, 0);
    let replay = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--dbc",
        &two,
        "--no-wait",
    ]);
    check(&replay, 0);
    assert_eq!(rows(&replay.stdout), rows(&decode.stdout));
    let report = dir.path().join("report.json");
    let repeated = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--dbc",
        &two,
        "--no-wait",
        "--speed",
        "2",
        "--repeat",
        "2",
        "--repeat-gap",
        "0.01",
        "--report",
        path(&report),
    ]);
    check(&repeated, 0);
    let repeated = rows(&repeated.stdout);
    assert_eq!(
        repeated
            .iter()
            .map(|r| r["record"]["frame"]["timestamp_ns"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [10_000_000, 30_000_000, 40_000_000, 60_000_000]
    );
    for i in 0..2 {
        assert_eq!(repeated[i]["signals"], repeated[i + 2]["signals"]);
        assert_eq!(
            repeated[i]["record"]["location"],
            repeated[i + 2]["record"]["location"]
        );
    }
    assert_eq!(repeated[0]["signals"][1]["physical"], 10.0);
    assert_eq!(repeated[0]["signals"][2]["status"], "inactive");
    assert_eq!(repeated[1]["signals"][2]["raw"]["type"], "signed");
    assert_eq!(repeated[1]["signals"][2]["physical"], -1.0);
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["decode_counts"]["decoded"], 4);
    assert_eq!(report["databases"].as_array().unwrap().len(), 2);
    assert_eq!(report["engine_revision"], canlog::dbc::ENGINE_REVISION);
    let filtered = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--dbc",
        &two,
        "--no-wait",
        "--channel",
        "2",
        "--start",
        "0.02",
        "--end",
        "0.04",
        "--limit",
        "1",
    ]);
    check(&filtered, 0);
    assert_eq!(
        rows(&filtered.stdout),
        vec![rows(&decode.stdout)[1].clone()]
    );
}

#[test]
fn decoded_file_is_atomic_and_matches_stdout() {
    let (dir, input, one, two) = fixture();
    let output = dir.path().join("signals.jsonl");
    let report = dir.path().join("report.json");
    let expected = cli(&["decode", path(&input), "--dbc", &one, "--dbc", &two]);
    check(&expected, 0);
    let out = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--dbc",
        &two,
        "--no-wait",
        "-o",
        path(&output),
        "--sync",
        "--report",
        path(&report),
    ]);
    check(&out, 0);
    assert!(out.stdout.is_empty());
    assert_eq!(rows(&fs::read(&output).unwrap()), rows(&expected.stdout));
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["published"], true);
    assert_eq!(report["finalized"], true);
    assert_eq!(report["durable"], true);
    assert_eq!(report["frames_written"], 2);
    check(
        &cli(&[
            "replay",
            path(&input),
            "--dbc",
            &one,
            "--no-wait",
            "-o",
            path(&output),
        ]),
        2,
    );
    assert_eq!(rows(&fs::read(output).unwrap()), rows(&expected.stdout));
}

#[test]
fn console_shows_units_raw_enum_and_inactive_signals() {
    let (_dir, input, one, two) = fixture();
    let out = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--dbc",
        &two,
        "--no-wait",
        "--sink",
        "console",
    ]);
    check(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Drive [decoded]"));
    assert!(text.contains("Speed = 10 km/h (raw=40, status=valid)"));
    assert!(text.contains("Driving"));
    assert!(text.contains("Temp = - C (raw=-, status=inactive)"));
    assert!(text.contains("Temp = -1 C (raw=-1, status=valid)"));
}

#[test]
fn quality_failures_and_remote_are_explicit_not_dropped() {
    let (dir, input, one, _two) = fixture();
    fs::write(&input, "base hex timestamps absolute\n0 1 100 Rx d 2 01 28\n0.01 2 100 Rx d 2 01 28\n0.02 1 101 Rx d 2 01 28\n0.03 1 100 Rx d 1 01\n0.04 1 100 Rx r 2\n").unwrap();
    let report = dir.path().join("report.json");
    let out = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--no-wait",
        "--report",
        path(&report),
    ]);
    check(&out, 3);
    assert_eq!(
        rows(&out.stdout)
            .iter()
            .map(|r| r["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "decoded",
            "no_database",
            "no_message",
            "length_mismatch",
            "remote"
        ]
    );
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["status"], "partial");
    assert_eq!(report["frames_selected"], 5);
    assert_eq!(report["issues"], 0);
    assert_eq!(report["decode_counts"]["remote"], 1);
}

#[test]
fn incompatible_formats_invalid_dbc_and_database_alias_are_protected() {
    let (dir, input, one, _two) = fixture();
    let db = dir.path().join("drive.dbc");
    let original = fs::read(&db).unwrap();
    for (name, extra) in [
        ("out.asc", vec![]),
        ("out.blf", vec![]),
        ("out.jsonl", vec!["--preserve-records"]),
    ] {
        let output = dir.path().join(name);
        let mut args = vec![
            "replay",
            path(&input),
            "--dbc",
            &one,
            "--no-wait",
            "-o",
            path(&output),
        ];
        args.extend(extra);
        check(&cli(&args), 2);
        assert!(!output.exists());
    }
    let alias = dir.path().join("db-alias.jsonl");
    fs::hard_link(&db, &alias).unwrap();
    let out = cli(&[
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--no-wait",
        "-o",
        path(&alias),
        "--overwrite",
    ]);
    check(&out, 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("output cannot replace DBC input"));
    assert_eq!(fs::read(&alias).unwrap(), original);
    check(
        &cli(&["replay", path(&input), "--dbc", &one, "--report", path(&db)]),
        2,
    );
    let output = dir.path().join("invalid.jsonl");
    check(
        &cli(&[
            "replay",
            path(&input),
            "--dbc",
            "bad",
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
fn skipped_events_still_require_named_file_loss_and_strict_failures_do_not_publish() {
    let (dir, input, one, _two) = fixture();
    fs::write(
        &input,
        "base hex timestamps absolute\n0 1 100 Rx d 2 01 28\n0.01 SV: event\n",
    )
    .unwrap();
    let output = dir.path().join("signals.jsonl");
    let base = [
        "replay",
        path(&input),
        "--dbc",
        &one,
        "--no-wait",
        "-o",
        path(&output),
    ];
    check(&cli(&base), 1);
    assert!(!output.exists());
    check(
        &cli(&[base.as_slice(), &["--unsupported", "skip"]].concat()),
        1,
    );
    assert!(!output.exists());
    check(
        &cli(&[
            base.as_slice(),
            &[
                "--unsupported",
                "skip",
                "--allow-loss",
                "unsupported-record",
            ],
        ]
        .concat()),
        3,
    );
    assert_eq!(rows(&fs::read(output).unwrap()).len(), 1);
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".partial")));
}

#[test]
fn timed_decoded_replay_obeys_pause_resume_and_stop() {
    let (_dir, input, one, _two) = fixture();
    fs::write(&input, "base hex timestamps absolute\n0 1 100 Rx d 2 01 28\n0.2 1 100 Rx d 2 01 29\n3600 1 100 Rx d 2 01 2A\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args([
            "replay",
            path(&input),
            "--dbc",
            &one,
            "--speed",
            "2",
            "--control-stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let first: Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
    assert_eq!(first["record"]["frame"]["timestamp_ns"], 0);
    child.stdin.as_mut().unwrap().write_all(b"pause\n").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    let resumed = Instant::now();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"resume\n")
        .unwrap();
    let second: Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
    assert!(resumed.elapsed() >= Duration::from_millis(50));
    assert_eq!(second["record"]["frame"]["timestamp_ns"], 200_000_000);
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(130));
    reader.join().unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn changed_database_rejects_atomic_publication() {
    let (dir, input, one, _two) = fixture();
    fs::write(
        &input,
        "base hex timestamps absolute\n0 1 100 Rx d 2 01 28\n0.5 1 100 Rx d 2 01 29\n",
    )
    .unwrap();
    let output = dir.path().join("signals.jsonl");
    let mut child = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(["replay", path(&input), "--dbc", &one, "-o", path(&output)])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Output temp creation proves the DBC has already been loaded.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_dir(dir.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".partial")
        }) {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("replay did not open its temporary output");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let db = dir.path().join("drive.dbc");
    let mut changed = fs::read(&db).unwrap();
    changed.extend_from_slice(b"\nCM_ \"Changed during replay\";\n");
    fs::write(db, changed).unwrap();
    let result = child.wait_with_output().unwrap();
    check(&result, 1);
    assert!(String::from_utf8_lossy(&result.stderr).contains("DBC changed"));
    assert!(!output.exists());
}

#[test]
fn stop_preserves_existing_decoded_output_and_reports_cancellation() {
    let (dir, input, one, _two) = fixture();
    fs::write(
        &input,
        "base hex timestamps absolute\n0 1 100 Rx d 2 01 28\n3600 1 100 Rx d 2 01 29\n",
    )
    .unwrap();
    let output = dir.path().join("signals.jsonl");
    let report = dir.path().join("report.json");
    fs::write(&output, b"previous output\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args([
            "replay",
            path(&input),
            "--dbc",
            &one,
            "-o",
            path(&output),
            "--overwrite",
            "--control-stdin",
            "--report",
            path(&report),
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
    assert_eq!(fs::read(output).unwrap(), b"previous output\n");
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["published"], false);
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".partial")));
}

#[test]
fn input_jsonl_provenance_loss_remains_explicit_for_decoded_files() {
    let (dir, input, one, _two) = fixture();
    let raw = cli(&["view", path(&input), "--channel", "1"]);
    check(&raw, 0);
    let jsonl = dir.path().join("source.jsonl");
    fs::write(&jsonl, raw.stdout).unwrap();
    let output = dir.path().join("decoded.jsonl");
    let report = dir.path().join("report.json");
    let args = [
        "replay",
        path(&jsonl),
        "--dbc",
        &one,
        "--no-wait",
        "-o",
        path(&output),
    ];
    let rejected = cli(&args);
    check(&rejected, 1);
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("field:source-metadata"));
    assert!(!output.exists());
    check(
        &cli(&[
            args.as_slice(),
            &[
                "--allow-loss",
                "field:source-metadata",
                "--report",
                path(&report),
            ],
        ]
        .concat()),
        3,
    );
    let result: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(result["losses"]["field:source-metadata"], 1);
    assert_eq!(result["published"], true);
    assert_eq!(
        rows(&fs::read(output).unwrap())[0]["record"]["location"]["source"],
        path(&jsonl)
    );
}
