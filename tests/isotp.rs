use canlog::{
    core::{Direction as CanDirection, Frame, FrameRecord, Location},
    isotp::{Config, Direction, Reassembler},
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn configuration() -> Value {
    json!({"schema_version":1,"routes":[{"name":"ecu","channel":1,"kind":"physical","profile":"classic","addressing":"normal","request":{"id":1792,"extended":false},"response":{"id":1536,"extended":false},"timeout_ns":100}]})
}
fn engine(config: Value) -> Reassembler {
    Reassembler::new(serde_json::from_value(config).unwrap()).unwrap()
}
fn frame(time: i64, id: u32, data: &[u8]) -> FrameRecord {
    FrameRecord {
        frame: Frame::new(
            time,
            1,
            id,
            false,
            CanDirection::Tx,
            false,
            false,
            data.len() as u8,
            data.to_vec(),
            false,
            false,
        )
        .unwrap(),
        location: Location {
            source: "fixture.asc".into(),
            ordinal: time as u64,
            line: Some(time as u64 + 1),
            ..Default::default()
        },
        native: None,
    }
}
fn send(e: &mut Reassembler, t: i64, id: u32, data: &[u8]) -> Vec<Value> {
    e.feed(&frame(t, id, data))
        .unwrap()
        .into_iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect()
}
#[test]
fn single_frames_preserve_exact_payload_and_ignore_can_rx_tx_for_route_direction() {
    let mut e = engine(configuration());
    let rows = send(
        &mut e,
        0,
        0x700,
        &[3, 0x22, 0xf1, 0x90, 0xaa, 0xaa, 0xaa, 0xaa],
    );
    assert_eq!(rows[0]["data_hex"], "22F190");
    assert_eq!(rows[0]["direction"], "request");
    assert_eq!(rows[0]["fc_observation"], "not_required");
    assert_eq!(rows[0]["protocol_compliance"], "unknown");
    let rows = send(&mut e, 0, 0x600, &[1, 0x7e]);
    assert_eq!(rows[0]["direction"], "response");
    assert!(send(&mut e, 0, 0x123, &[0xff]).is_empty());
    assert!(e.eof().is_empty());
}
#[test]
fn maximum_length_sequence_wrap_and_final_padding_are_reassembled_without_fc() {
    let mut e = engine(configuration());
    let payload: Vec<u8> = (0..4095).map(|i| (i % 251) as u8).collect();
    let mut first = vec![0x1f, 0xff];
    first.extend_from_slice(&payload[..6]);
    assert!(send(&mut e, 0, 0x600, &first).is_empty());
    assert_eq!(e.reserved_payload_bytes(), 4095);
    let mut final_rows = vec![];
    for (i, chunk) in payload[6..].chunks(7).enumerate() {
        let mut data = vec![0x20 | (((i + 1) % 16) as u8)];
        data.extend_from_slice(chunk);
        data.resize(8, 0xaa);
        final_rows = send(&mut e, i as i64 + 1, 0x600, &data);
    }
    let row = &final_rows[0];
    assert_eq!(row["status"], "complete");
    assert_eq!(row["observed_length"], 4095);
    let actual = row["data_hex"].as_str().unwrap();
    for (i, byte) in payload.iter().enumerate() {
        assert_eq!(&actual[i * 2..i * 2 + 2], format!("{byte:02X}"));
    }
    assert_eq!(row["fc_observation"], "missing");
    assert_eq!(row["data_locations"].as_array().unwrap().len(), 586);
    assert_eq!(e.active_sessions(), 0);
    assert_eq!(e.reserved_payload_bytes(), 0);
}
#[test]
fn interleaved_directions_and_channels_do_not_mix_payloads() {
    let mut config = configuration();
    let mut other = config["routes"][0].clone();
    other["name"] = json!("other");
    other["channel"] = json!(2);
    config["routes"].as_array_mut().unwrap().push(other);
    let mut e = engine(config);
    send(&mut e, 0, 0x700, &[0x10, 9, 1, 2, 3, 4, 5, 6]);
    send(&mut e, 1, 0x600, &[0x10, 8, 11, 12, 13, 14, 15, 16]);
    let mut ch2 = frame(2, 0x600, &[0x10, 8, 21, 22, 23, 24, 25, 26]);
    let mut wire = serde_json::to_value(&ch2.frame).unwrap();
    wire["channel"] = json!(2);
    ch2.frame = serde_json::from_value(wire).unwrap();
    e.feed(&ch2).unwrap();
    assert_eq!(e.active_sessions(), 3);
    assert_eq!(
        send(&mut e, 3, 0x700, &[0x21, 7, 8, 9])[0]["data_hex"],
        "010203040506070809"
    );
    assert_eq!(
        send(&mut e, 4, 0x600, &[0x21, 17, 18])[0]["data_hex"],
        "0B0C0D0E0F101112"
    );
    let rows = e.eof();
    assert_eq!(rows[0].route.as_deref(), Some("other"));
    assert_eq!(rows[0].direction, Some(Direction::Response));
    assert_eq!(rows[0].reason.as_deref(), Some("end_of_file"));
}
#[test]
fn loss_duplicate_orphan_and_new_first_frame_never_fabricate_completion() {
    let mut e = engine(configuration());
    send(&mut e, 0, 0x600, &[0x10, 20, 1, 2, 3, 4, 5, 6]);
    send(&mut e, 1, 0x600, &[0x21, 7, 8, 9, 10, 11, 12, 13]);
    let rows = send(&mut e, 2, 0x600, &[0x21, 7, 8, 9, 10, 11, 12, 13]);
    assert_eq!(rows[0]["status"], "incomplete");
    assert_eq!(rows[0]["observed_length"], 13);
    assert_eq!(rows[1]["reason"], "sequence_mismatch_possible_capture_loss");
    assert_eq!(
        send(&mut e, 3, 0x600, &[0x22, 14])[0]["reason"],
        "orphan_consecutive_frame"
    );
    send(&mut e, 4, 0x600, &[0x10, 20, 1, 2, 3, 4, 5, 6]);
    let rows = send(&mut e, 5, 0x600, &[0x10, 9, 11, 12, 13, 14, 15, 16]);
    assert_eq!(rows[0]["status"], "aborted");
    assert_eq!(rows[0]["reason"], "replaced_by_first_frame");
    assert_eq!(
        send(&mut e, 6, 0x600, &[0x21, 17, 18, 19])[0]["status"],
        "complete"
    );
    send(&mut e, 7, 0x600, &[0x10, 20, 1, 2, 3, 4, 5, 6]);
    let rows = send(&mut e, 8, 0x600, &[1, 0x7e]);
    assert_eq!(rows[0]["reason"], "replaced_by_single_frame");
    assert_eq!(rows[1]["data_hex"], "7E");
}
#[test]
fn timeout_uses_unrelated_log_time_equal_deadline_is_allowed_and_eof_is_distinct() {
    let mut e = engine(configuration());
    send(&mut e, 0, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    assert!(send(&mut e, 100, 0x123, &[0]).is_empty());
    assert_eq!(
        send(&mut e, 100, 0x600, &[0x21, 7, 8])[0]["status"],
        "complete"
    );
    send(&mut e, 101, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    let rows = send(&mut e, 202, 0x123, &[0]);
    assert_eq!(rows[0]["reason"], "timeout_observed_in_log");
    assert!(e.feed(&frame(201, 0x123, &[0])).is_err());
    send(&mut e, 203, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    assert_eq!(e.eof()[0].reason.as_deref(), Some("end_of_file"));
}
#[test]
fn flow_control_observation_is_opposite_direction_and_metrics_are_not_verdicts() {
    let mut e = engine(configuration());
    send(&mut e, 0, 0x600, &[0x10, 27, 1, 2, 3, 4, 5, 6]);
    let fc = send(&mut e, 1, 0x700, &[0x30, 1, 0xf1]);
    assert_eq!(fc[0]["flow_control"]["last_stmin_ns"], 100_000);
    send(&mut e, 2, 0x600, &[0x21, 7, 8, 9, 10, 11, 12, 13]);
    send(&mut e, 3, 0x700, &[0x31, 0, 0]);
    send(&mut e, 4, 0x600, &[0x22, 14, 15, 16, 17, 18, 19, 20]);
    send(&mut e, 5, 0x700, &[0x30, 1, 0xf1]);
    let rows = send(&mut e, 6, 0x600, &[0x23, 21, 22, 23, 24, 25, 26, 27]);
    assert_eq!(rows[0]["flow_control"]["count"], 3);
    assert_eq!(rows[0]["flow_control"]["cf_after_wait"], 1);
    assert_eq!(rows[0]["flow_control"]["shorter_than_stmin"], 2);
    assert_eq!(rows[0]["protocol_compliance"], "unknown");
    send(&mut e, 7, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    let rows = send(&mut e, 8, 0x700, &[0x32, 0, 0]);
    assert_eq!(rows[1]["status"], "aborted");
    assert_eq!(rows[1]["reason"], "observed_flow_control_overflow");
    assert_eq!(canlog::isotp::stmin_ns(0xf9), Some(900_000));
    assert_eq!(canlog::isotp::stmin_ns(0x80), None);
    assert_eq!(canlog::isotp::stmin_ns(0xfa), None);
}
#[test]
fn flow_reference_examples_are_bounded_and_reserved_stmin_remains_unknown() {
    let mut e = engine(configuration());
    send(&mut e, 0, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    for t in 1..=32 {
        send(&mut e, t, 0x700, &[0x30, 0, 0x80]);
    }
    let row = send(&mut e, 33, 0x600, &[0x21, 7, 8]).remove(0);
    assert_eq!(row["flow_control"]["count"], 32);
    assert_eq!(
        row["flow_control"]["locations"].as_array().unwrap().len(),
        16
    );
    assert_eq!(row["flow_control"]["omitted_locations"], 16);
    assert_eq!(row["flow_control"]["reserved_stmin"], 32);
    assert!(row["flow_control"]["last_stmin_ns"].is_null());
}
#[test]
fn addressed_routes_and_can_id_kinds_are_explicit() {
    for mode in ["extended", "mixed"] {
        let mut config = configuration();
        config["routes"][0]["addressing"] = json!(mode);
        config["routes"][0]["request"]["address"] = json!(0x11);
        config["routes"][0]["response"]["address"] =
            json!(if mode == "mixed" { 0x11 } else { 0x22 });
        let address = if mode == "mixed" { 0x11 } else { 0x22 };
        let mut e = engine(config);
        assert!(send(&mut e, 0, 0x600, &[0x33, 1, 0x7e]).is_empty());
        assert!(send(&mut e, 1, 0x600, &[address, 0x10, 8, 1, 2, 3, 4, 5]).is_empty());
        assert_eq!(
            send(&mut e, 2, 0x600, &[address, 0x21, 6, 7, 8])[0]["data_hex"],
            "0102030405060708"
        );
    }
    let mut config = configuration();
    config["routes"][0]["response"]["extended"] = json!(true);
    let mut e = engine(config);
    assert!(send(&mut e, 0, 0x600, &[1, 0x7e]).is_empty());
    let mut r = frame(1, 0x600, &[1, 0x7e]);
    let mut wire = serde_json::to_value(&r.frame).unwrap();
    wire["extended"] = json!(true);
    r.frame = serde_json::from_value(wire).unwrap();
    assert_eq!(e.feed(&r).unwrap()[0].data_hex, "7E");
}
#[test]
fn limits_reject_before_allocation_and_release_aborted_reservations() {
    let mut config = configuration();
    config["max_payload_bytes"] = json!(8);
    config["max_total_payload_bytes"] = json!(8);
    let mut e = engine(config);
    assert_eq!(
        send(&mut e, 0, 0x600, &[0x10, 9, 1, 2, 3, 4, 5, 6])[0]["reason"],
        "payload_limit"
    );
    assert_eq!(e.reserved_payload_bytes(), 0);
    send(&mut e, 1, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    assert_eq!(
        send(&mut e, 2, 0x700, &[0x10, 8, 1, 2, 3, 4, 5, 6])[0]["reason"],
        "session_or_total_payload_limit"
    );
    e.eof();
    assert_eq!(e.reserved_payload_bytes(), 0);
    let mut config = configuration();
    config["max_sessions"] = json!(1);
    let mut e = engine(config);
    send(&mut e, 0, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    assert_eq!(
        send(&mut e, 1, 0x700, &[0x10, 8, 1, 2, 3, 4, 5, 6])[0]["reason"],
        "session_or_total_payload_limit"
    );
}
#[test]
fn invalid_layout_padding_and_fd_are_visible_and_break_active_prefix() {
    let mut config = configuration();
    config["routes"][0]["padding_byte"] = json!(0xaa);
    let mut e = engine(config);
    assert_eq!(
        send(&mut e, 0, 0x600, &[1, 0x7e, 0xff])[0]["reason"],
        "invalid_single_frame_or_padding"
    );
    for data in [
        &[0u8][..],
        &[0x10, 0, 1, 2, 3, 4, 5, 6],
        &[0x10, 7, 1, 2, 3, 4, 5, 6],
        &[0x10, 8, 1, 2],
        &[0x33, 0, 0],
        &[0x30, 0],
        &[0x40],
    ] {
        assert!(!send(&mut e, 1, 0x600, data).is_empty());
    }
    send(&mut e, 2, 0x600, &[0x10, 20, 1, 2, 3, 4, 5, 6]);
    assert_eq!(
        send(&mut e, 3, 0x600, &[0x21, 7])[1]["reason"],
        "truncated_consecutive_frame"
    );
    send(&mut e, 4, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    assert_eq!(
        send(&mut e, 5, 0x600, &[0x21, 7, 8, 0xff])[1]["reason"],
        "invalid_final_padding"
    );
    let r = FrameRecord {
        frame: Frame::new(
            6,
            1,
            0x600,
            false,
            CanDirection::Rx,
            false,
            true,
            8,
            vec![1, 0x7e, 0, 0, 0, 0, 0, 0],
            false,
            false,
        )
        .unwrap(),
        location: Location::default(),
        native: None,
    };
    assert_eq!(
        e.feed(&r).unwrap()[0].reason.as_deref(),
        Some("unsupported_frame_profile_or_empty_payload")
    );
}
#[test]
fn route_schema_bounds_and_overlapping_endpoints_are_rejected() {
    for (key, value) in [
        ("schema_version", json!(2)),
        ("max_payload_bytes", json!(4096)),
        ("max_total_payload_bytes", json!(1_048_577)),
        ("max_sessions", json!(129)),
    ] {
        let mut config = configuration();
        config[key] = value;
        assert!(serde_json::from_value::<Config>(config)
            .unwrap()
            .validate()
            .is_err());
    }
    let mut config = configuration();
    let other = config["routes"][0].clone();
    config["routes"].as_array_mut().unwrap().push(other);
    config["routes"][1]["name"] = json!("second");
    assert!(serde_json::from_value::<Config>(config)
        .unwrap()
        .validate()
        .is_err());
    for (key, value) in [
        ("profile", json!("fd")),
        ("kind", json!("functional")),
        ("surprise", json!(true)),
    ] {
        let mut config = configuration();
        config["routes"][0][key] = value;
        assert!(serde_json::from_value::<Config>(config).is_err());
    }
}

#[test]
fn capture_gap_flushes_only_known_channel_and_unknown_gap_flushes_all() {
    let mut config = configuration();
    let mut other = config["routes"][0].clone();
    other["name"] = json!("other");
    other["channel"] = json!(2);
    config["routes"].as_array_mut().unwrap().push(other);
    let mut e = engine(config);
    send(&mut e, 0, 0x600, &[0x10, 8, 1, 2, 3, 4, 5, 6]);
    let mut r = frame(1, 0x600, &[0x10, 8, 11, 12, 13, 14, 15, 16]);
    let mut wire = serde_json::to_value(&r.frame).unwrap();
    wire["channel"] = json!(2);
    r.frame = serde_json::from_value(wire).unwrap();
    e.feed(&r).unwrap();
    let interrupted = e.gap(Some(1));
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].reason.as_deref(), Some("capture_gap"));
    assert_eq!(e.active_sessions(), 1);
    send(&mut e, 2, 0x600, &[1, 0x7e]);
    assert_eq!(e.capture_gaps, 1);
    assert_eq!(e.gap(None).len(), 1);
    assert_eq!(e.active_sessions(), 0);
    assert_eq!(
        send(&mut e, 3, 0x600, &[1, 0x7e])[0]["capture_gaps_before"],
        2
    );
}

#[test]
fn cancellation_does_not_publish_or_replace_destination() {
    use canlog::{
        app::{InputArgs, Unsupported},
        diagnostics::{analyze, IsotpOutput},
        playback::{Cancellation, Cancelled},
    };
    let (dir, input, routes) = fixture("0 1 600 Tx d 2 01 7E\n");
    let destination = dir.path().join("output.jsonl");
    fs::write(&destination, b"existing").unwrap();
    let args = InputArgs {
        input: path(&input).into(),
        input_format: None,
        id_map: None,
        preserve_records: false,
        unsupported: Unsupported::Error,
        recover: false,
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
    let cancel = Cancellation::default();
    cancel.cancel();
    let (report, result) = analyze(
        &args,
        &routes,
        &IsotpOutput {
            output: Some(destination.clone()),
            overwrite: true,
        },
        &cancel,
    );
    assert!(result.unwrap_err().is::<Cancelled>());
    assert_eq!(report.status, "cancelled");
    assert!(!report.scan_complete && !report.published);
    assert_eq!(fs::read(destination).unwrap(), b"existing");
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn cli(args: &[&str], expected: i32) -> std::process::Output {
    let out = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(expected),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn fixture(text: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("input.asc");
    let routes = dir.path().join("routes.json");
    fs::write(&input, format!("base hex timestamps absolute\n{text}")).unwrap();
    let mut config = configuration();
    config["routes"][0]["timeout_ns"] = json!(1_000_000_000);
    fs::write(&routes, serde_json::to_vec(&config).unwrap()).unwrap();
    (dir, input, routes)
}
fn rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
#[test]
fn cli_atomic_jsonl_report_blfs_and_stable_keys_work_end_to_end() {
    let (dir,input,routes) = fixture("0.0 1 600 Tx d 8 10 08 01 02 03 04 05 06\n0.01 1 700 Tx d 3 30 00 14\n0.02 1 600 Tx d 3 21 07 08\n");
    let output = dir.path().join("payloads.jsonl");
    let report = dir.path().join("report.json");
    let stdout = cli(&["isotp", path(&input), "--routes", path(&routes)], 0);
    cli(
        &[
            "isotp",
            path(&input),
            "--routes",
            path(&routes),
            "-o",
            path(&output),
            "--report",
            path(&report),
        ],
        0,
    );
    assert_eq!(stdout.stdout, fs::read(&output).unwrap());
    let result_rows = rows(&stdout.stdout);
    assert_eq!(result_rows[1]["data_hex"], "0102030405060708");
    assert_ne!(result_rows[0]["result_key"], result_rows[1]["result_key"]);
    let metadata: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(metadata["scan_complete"], true);
    assert_eq!(metadata["published"], true);
    assert_eq!(metadata["status"], "complete");
    assert_eq!(metadata["frames_matched"], 3);
    let blf = dir.path().join("input.blf");
    cli(&["convert", path(&input), path(&blf)], 0);
    let blf_result = cli(&["isotp", path(&blf), "--routes", path(&routes)], 0);
    assert_eq!(rows(&blf_result.stdout)[1]["data_hex"], "0102030405060708");
}
#[test]
fn cli_parse_gap_partial_output_cannot_complete_interrupted_session() {
    let (dir,input,routes) = fixture("0.0 1 600 Tx d 8 10 08 01 02 03 04 05 06\n0.01 SV: 1 0 1 ::Demo::Unknown = 1\n0.02 1 600 Tx d 3 21 07 08\n");
    let output = dir.path().join("out.jsonl");
    cli(
        &[
            "isotp",
            path(&input),
            "--routes",
            path(&routes),
            "-o",
            path(&output),
        ],
        1,
    );
    assert!(!output.exists());
    let out = cli(
        &[
            "isotp",
            path(&input),
            "--routes",
            path(&routes),
            "--unsupported",
            "skip",
            "-o",
            path(&output),
        ],
        3,
    );
    assert!(out.stdout.is_empty());
    let result = rows(&fs::read(output).unwrap());
    assert_eq!(result[0]["reason"], "capture_gap");
    assert_eq!(result[1]["kind"], "capture_gap");
    assert_eq!(result[2]["reason"], "orphan_consecutive_frame");
}
#[test]
fn cli_clock_regression_on_unrelated_frames_never_replaces_destination() {
    let (dir, input, routes) = fixture("0.02 1 600 Tx d 2 01 7E\n0.01 2 100 Rx d 1 00\n");
    let output = dir.path().join("out.jsonl");
    fs::write(&output, b"existing").unwrap();
    let report = dir.path().join("failed.json");
    cli(
        &[
            "isotp",
            path(&input),
            "--routes",
            path(&routes),
            "-o",
            path(&output),
            "--overwrite",
            "--report",
            path(&report),
        ],
        1,
    );
    assert_eq!(fs::read(output).unwrap(), b"existing");
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["scan_complete"], false);
    assert_eq!(report["published"], false);
    assert_eq!(report["status"], "failed");
}
#[test]
fn cli_rejects_filters_aliases_and_oversized_routes_without_mutation() {
    let (dir, input, routes) = fixture("0 1 600 Tx d 2 01 7E\n");
    let source = fs::read(&input).unwrap();
    let config = fs::read(&routes).unwrap();
    for flag in ["--id", "--channel", "--limit", "--start"] {
        cli(
            &["isotp", path(&input), "--routes", path(&routes), flag, "1"],
            1,
        );
    }
    for protected in [&input, &routes] {
        cli(
            &[
                "isotp",
                path(&input),
                "--routes",
                path(&routes),
                "-o",
                path(protected),
                "--overwrite",
            ],
            1,
        );
        cli(
            &[
                "isotp",
                path(&input),
                "--routes",
                path(&routes),
                "--report",
                path(protected),
            ],
            1,
        );
    }
    let alias = dir.path().join("alias.json");
    fs::hard_link(&routes, &alias).unwrap();
    cli(
        &[
            "isotp",
            path(&input),
            "--routes",
            path(&routes),
            "-o",
            path(&alias),
            "--overwrite",
        ],
        1,
    );
    assert_eq!(fs::read(&input).unwrap(), source);
    assert_eq!(fs::read(&routes).unwrap(), config);
    let huge = dir.path().join("huge.json");
    fs::write(&huge, vec![b' '; 262_145]).unwrap();
    cli(&["isotp", path(&input), "--routes", path(&huge)], 1);
}
