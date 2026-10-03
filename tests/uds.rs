use canlog::{
    core::Location,
    isotp::{Config as Transport, Direction, Event, FlowObservation, ResponseStart},
    uds::{Config, Matcher, Status},
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;
fn routes() -> Value {
    json!({"schema_version":1,"routes":[{"name":"ecu","channel":1,"kind":"physical","profile":"classic","addressing":"normal","request":{"id":2016,"extended":false},"response":{"id":2024,"extended":false}}]})
}
fn policy() -> Value {
    json!({"schema_version":1,"routes":[{"route":"ecu","protocol":"uds2013","p2_ns":50,"p2_star_ns":100,"transaction_max_duration_ns":500}]})
}
fn matcher(value: Value) -> Matcher {
    Matcher::new(
        serde_json::from_value(value).unwrap(),
        &serde_json::from_value(routes()).unwrap(),
        "timeline",
    )
    .unwrap()
}
fn packet(direction: Direction, time: i64, data: &[u8]) -> Event {
    let location = Location {
        source: "source.asc".into(),
        ordinal: time as u64,
        line: Some(time as u64 + 1),
        ..Default::default()
    };
    Event {
        kind: "payload".into(),
        route: Some("ecu".into()),
        direction: Some(direction),
        status: "complete".into(),
        reason: None,
        first_timestamp_ns: Some(time),
        last_timestamp_ns: Some(time),
        first_location: location.clone(),
        last_location: location.clone(),
        declared_length: Some(data.len()),
        observed_length: data.len(),
        data_hex: data.iter().map(|b| format!("{b:02X}")).collect(),
        data_locations: vec![location],
        flow_control: FlowObservation::default(),
        fc_observation: "not_required".into(),
        protocol_compliance: "unknown".into(),
        capture_gaps_before: 0,
    }
}
fn request(m: &mut Matcher, time: i64, data: &[u8]) {
    assert!(m
        .consume(&packet(Direction::Request, time, data))
        .unwrap()
        .is_empty());
}
#[test]
fn service_headers_and_echoes_match_with_raw_data_and_explicit_latency() {
    let cases: &[(&[u8], &[u8])] = &[
        (&[0x10, 3], &[0x50, 3, 0, 50, 1, 0xf4]),
        (&[0x11, 1], &[0x51, 1]),
        (&[0x14, 0xff, 0xff, 0xff], &[0x54]),
        (&[0x19, 1, 0xff], &[0x59, 1, 0xff, 1, 0, 4]),
        (&[0x19, 2, 0xff], &[0x59, 2, 0xff, 1, 2, 3, 0x78]),
        (&[0x22, 0xf1, 0x90], &[0x62, 0xf1, 0x90, 0x57]),
        (&[0x27, 1], &[0x67, 1, 0xaa, 0xbb]),
        (&[0x2e, 0xf1, 0x91, 1], &[0x6e, 0xf1, 0x91]),
        (&[0x31, 1, 0xff, 0], &[0x71, 1, 0xff, 0, 1]),
        (&[0x34, 0, 0x22, 0x10, 0, 0x40, 0], &[0x74, 0x20, 0, 0xff]),
        (&[0x36, 0x81, 1], &[0x76, 0x81]),
        (&[0x37], &[0x77]),
        (&[0x3e, 0], &[0x7e, 0]),
    ];
    for (req, resp) in cases {
        let mut m = matcher(policy());
        request(&mut m, 0, req);
        let row = m
            .consume(&packet(Direction::Response, 10, resp))
            .unwrap()
            .remove(0);
        assert_eq!(row.status, Status::Positive, "{req:?}");
        assert_eq!(row.response_match, "matched");
        assert_eq!(row.request_last_to_response_first_ns, Some(10));
        assert_eq!(row.request_first_to_response_last_ns, Some(10));
        assert_eq!(row.data_semantics, "opaque_no_cdd");
        assert!(row.cdd.is_none());
        assert_eq!(m.outstanding(), 0);
        assert_eq!(m.reserved_request_bytes(), 0);
    }
}
#[test]
fn did_subfunction_routine_and_transfer_counters_are_not_matched_on_sid_alone() {
    let cases: &[(&[u8], &[u8])] = &[
        (&[0x10, 3], &[0x50, 2, 0, 50, 0, 1]),
        (&[0x22, 0xf1, 0x90], &[0x62, 0xf1, 0x91, 1]),
        (&[0x31, 1, 0xff, 0], &[0x71, 2, 0xff, 0]),
        (&[0x36, 0x81], &[0x76, 1]),
    ];
    for (req, resp) in cases {
        let mut m = matcher(policy());
        request(&mut m, 0, req);
        assert_eq!(
            m.consume(&packet(Direction::Response, 10, resp)).unwrap()[0].status,
            Status::Orphan
        );
        assert_eq!(m.outstanding(), 1);
    }
    let mut m = matcher(policy());
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    request(&mut m, 1, &[0x22, 0xf1, 0x91]);
    let matched = m
        .consume(&packet(Direction::Response, 2, &[0x62, 0xf1, 0x90, 1]))
        .unwrap();
    assert_eq!(matched[0].header.as_ref().unwrap().identifiers, [0xf190]);
    assert_eq!(m.outstanding(), 1);
}
#[test]
fn negative_and_pending_nrc_require_unambiguous_request_context() {
    let mut m = matcher(policy());
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    let negative = m
        .consume(&packet(Direction::Response, 10, &[0x7f, 0x22, 0x31]))
        .unwrap()
        .remove(0);
    assert_eq!(negative.status, Status::Negative);
    assert_eq!(negative.nrc, Some(0x31));
    request(&mut m, 11, &[0x22, 0xf1, 0x90]);
    request(&mut m, 12, &[0x22, 0xf1, 0x91]);
    let ambiguity = m
        .consume(&packet(Direction::Response, 13, &[0x7f, 0x22, 0x78]))
        .unwrap();
    assert_eq!(ambiguity.len(), 2);
    for row in ambiguity {
        assert_eq!(row.status, Status::Ambiguous);
        assert_eq!(row.candidate_transaction_keys.len(), 2);
        assert!(row.request_last_to_response_first_ns.is_none());
    }
    assert_eq!(m.outstanding(), 0);
    request(&mut m, 14, &[0x36, 1]);
    request(&mut m, 15, &[0x36, 1]);
    assert!(m
        .consume(&packet(Direction::Response, 16, &[0x76, 1]))
        .unwrap()
        .iter()
        .all(|r| r.status == Status::Ambiguous));
}
#[test]
fn multi_did_headers_preserve_list_without_inventing_response_boundaries() {
    let mut m = matcher(policy());
    request(&mut m, 0, &[0x22, 0xf1, 0x90, 0xf1, 0x91]);
    let row = m
        .consume(&packet(
            Direction::Response,
            10,
            &[0x62, 0xf1, 0x90, 1, 2, 3],
        ))
        .unwrap()
        .remove(0);
    assert_eq!(row.status, Status::Ambiguous);
    assert_eq!(
        row.reason.as_deref(),
        Some("multi_did_response_requires_definition")
    );
    assert_eq!(row.header.unwrap().identifiers, [0xf190, 0xf191]);
}
#[test]
fn pending_updates_only_matching_route_and_is_bounded_by_total_duration() {
    let mut transport: Transport = serde_json::from_value(routes()).unwrap();
    let mut second = transport.routes[0].clone();
    second.name = "other".into();
    second.channel = 2;
    transport.routes.push(second);
    let mut value = policy();
    let mut second = value["routes"][0].clone();
    second["route"] = json!("other");
    value["routes"].as_array_mut().unwrap().push(second);
    let mut m = Matcher::new(serde_json::from_value(value).unwrap(), &transport, "clock").unwrap();
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    let mut other = packet(Direction::Request, 1, &[0x22, 0xf1, 0x90]);
    other.route = Some("other".into());
    m.consume(&other).unwrap();
    let pending = m
        .consume(&packet(Direction::Response, 20, &[0x7f, 0x22, 0x78]))
        .unwrap()
        .remove(0);
    assert_eq!(pending.status, Status::Pending);
    assert_eq!(pending.lifecycle, "open");
    assert_eq!(pending.p2_deadline_ns, Some(120));
    let expired = m.advance(60, &[]);
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].route, "other");
    assert_eq!(expired[0].status, Status::NoResponseObserved);
    for time in [119, 218, 317, 416, 499] {
        assert_eq!(
            m.consume(&packet(Direction::Response, time, &[0x7f, 0x22, 0x78]))
                .unwrap()[0]
                .status,
            Status::Pending
        );
    }
    let final_row = m.advance(501, &[]).remove(0);
    assert_eq!(final_row.status, Status::PendingOnly);
    assert_eq!(final_row.reason.as_deref(), Some("observation_limit"));
    assert_eq!(final_row.total_deadline_ns, Some(500));
}
#[test]
fn pending_examples_and_count_are_bounded_and_final_response_closes_context() {
    let mut value = policy();
    value["max_pending_examples"] = json!(1);
    value["max_pending_per_transaction"] = json!(3);
    let mut m = matcher(value);
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    m.consume(&packet(Direction::Response, 10, &[0x7f, 0x22, 0x78]))
        .unwrap();
    m.consume(&packet(Direction::Response, 20, &[0x7f, 0x22, 0x78]))
        .unwrap();
    let row = m
        .consume(&packet(Direction::Response, 30, &[0x7f, 0x22, 0x78]))
        .unwrap()
        .remove(0);
    assert_eq!(row.status, Status::PendingOnly);
    assert_eq!(row.pending_count, 3);
    assert_eq!(row.pending_examples.len(), 1);
    assert_eq!(row.omitted_pending_examples, 2);
    assert_eq!(m.reserved_request_bytes(), 0);
    let mut m = matcher(policy());
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    m.consume(&packet(Direction::Response, 20, &[0x7f, 0x22, 0x78]))
        .unwrap();
    let row = m
        .consume(&packet(Direction::Response, 110, &[0x62, 0xf1, 0x90, 1]))
        .unwrap()
        .remove(0);
    assert_eq!(row.status, Status::Positive);
    assert_eq!(row.pending_count, 1);
}
#[test]
fn p2_uses_response_start_and_equal_timestamp_uses_source_order() {
    let mut m = matcher(policy());
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    let started = ResponseStart {
        route: "ecu".into(),
        first_timestamp_ns: 40,
        first_ordinal: 40,
    };
    assert!(m.advance(90, &[started]).is_empty());
    let mut response = packet(Direction::Response, 40, &[0x62, 0xf1, 0x90, 1]);
    response.last_timestamp_ns = Some(90);
    response.last_location.ordinal = 90;
    let row = m.consume(&response).unwrap().remove(0);
    assert_eq!(row.status, Status::Positive);
    assert_eq!(row.request_last_to_response_first_ns, Some(40));
    assert_eq!(row.request_first_to_response_last_ns, Some(90));
    let mut m = matcher(policy());
    let mut req = packet(Direction::Request, 0, &[0x22, 0xf1, 0x90]);
    req.first_location.ordinal = 10;
    req.last_location.ordinal = 10;
    m.consume(&req).unwrap();
    let mut resp = packet(Direction::Response, 0, &[0x62, 0xf1, 0x90, 1]);
    resp.first_location.ordinal = 9;
    assert_eq!(m.consume(&resp).unwrap()[0].status, Status::Orphan);
    resp.first_location.ordinal = 11;
    assert_eq!(m.consume(&resp).unwrap()[0].status, Status::Positive);
}
#[test]
fn eof_gap_observed_timeout_and_suppression_are_distinct() {
    let mut m = matcher(policy());
    request(&mut m, 0, &[0x3e, 0x80]);
    assert_eq!(m.eof(0)[0].status, Status::Incomplete);
    request(&mut m, 1, &[0x3e, 0x80]);
    assert!(m.advance(51, &[]).is_empty());
    assert_eq!(m.advance(52, &[])[0].status, Status::SuppressedExpected);
    request(&mut m, 53, &[0x22, 0xf1, 0x90]);
    assert_eq!(m.advance(104, &[])[0].status, Status::NoResponseObserved);
    request(&mut m, 105, &[0x22, 0xf1, 0x90]);
    assert_eq!(m.gap(Some(2), 110).len(), 0);
    assert_eq!(m.gap(Some(1), 110)[0].status, Status::Incomplete);
    assert_eq!(m.reserved_request_bytes(), 0);
}
#[test]
fn unsupported_malformed_and_resource_limit_do_not_guess_new_context() {
    let mut m = matcher(policy());
    for data in [
        &[0x1a, 0x90][..],
        &[0x19, 3],
        &[0x14, 0, 0],
        &[0x10],
        &[0x34, 0, 0],
    ] {
        assert!(!m
            .consume(&packet(Direction::Request, 0, data))
            .unwrap()
            .is_empty());
    }
    assert_eq!(m.outstanding(), 0);
    request(&mut m, 1, &[0x22, 0xf1, 0x90]);
    assert_eq!(
        m.consume(&packet(Direction::Response, 2, &[0x7f, 0x22]))
            .unwrap()[0]
            .status,
        Status::Malformed
    );
    assert_eq!(m.outstanding(), 1);
    let mut value = policy();
    value["max_outstanding"] = json!(1);
    value["max_total_request_bytes"] = json!(3);
    let mut m = matcher(value);
    request(&mut m, 0, &[0x22, 0xf1, 0x90]);
    let rows = m
        .consume(&packet(Direction::Request, 1, &[0x22, 0xf1, 0x91]))
        .unwrap();
    assert_eq!(rows[0].status, Status::Incomplete);
    assert_eq!(rows[1].status, Status::ResourceLimit);
    assert_eq!(m.outstanding(), 0);
}
#[test]
fn finite_policy_and_checked_deadline_addition_reject_invalid_inputs() {
    let transport: Transport = serde_json::from_value(routes()).unwrap();
    for (key, value) in [
        ("schema_version", json!(2)),
        ("max_outstanding", json!(129)),
        ("max_pending_per_transaction", json!(0)),
    ] {
        let mut config = policy();
        config[key] = value;
        assert!(serde_json::from_value::<Config>(config)
            .unwrap()
            .validate(&transport)
            .is_err());
    }
    let mut config = policy();
    config["routes"][0]["p2_ns"] = json!(0);
    assert!(serde_json::from_value::<Config>(config)
        .unwrap()
        .validate(&transport)
        .is_err());
    let mut config = policy();
    config["routes"][0]["protocol"] = json!("kwp2000");
    assert!(serde_json::from_value::<Config>(config).is_err());
    let mut m = matcher(policy());
    assert!(m
        .consume(&packet(
            Direction::Request,
            i64::MAX - 10,
            &[0x22, 0xf1, 0x90]
        ))
        .is_err());
    assert_eq!(m.outstanding(), 0);
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
fn fixture(
    text: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("input.asc");
    let route = dir.path().join("routes.json");
    let config = dir.path().join("policy.json");
    fs::write(
        &source,
        format!("base hex timestamps absolute\nno internal events logged\n{text}"),
    )
    .unwrap();
    fs::write(&route, serde_json::to_vec(&routes()).unwrap()).unwrap();
    let mut value = policy();
    value["routes"][0]["p2_ns"] = json!(50_000_000);
    value["routes"][0]["p2_star_ns"] = json!(100_000_000);
    value["routes"][0]["transaction_max_duration_ns"] = json!(500_000_000);
    fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
    (dir, source, route, config)
}
fn rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
#[test]
fn cli_holds_timely_first_frame_until_late_completion_and_preserves_isotp_rows() {
    let (dir,source,route,policy)=fixture("0.0 1 7E0 Tx d 4 03 22 F1 90\n0.04 1 7E8 Rx d 8 10 08 62 F1 90 01 02 03\n0.06 1 100 Rx d 1 00\n0.09 1 7E8 Rx d 3 21 04 05\n");
    let report = dir.path().join("report.json");
    let result = cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "--report",
            path(&report),
        ],
        0,
    );
    let data = rows(&result.stdout);
    let transaction = data
        .iter()
        .find(|r| r["kind"] == "uds_transaction")
        .unwrap();
    assert_eq!(transaction["status"], "positive");
    assert_eq!(transaction["request_last_to_response_first_ns"], 40_000_000);
    assert_eq!(transaction["request_first_to_response_last_ns"], 90_000_000);
    let before = cli(&["isotp", path(&source), "--routes", path(&route)], 0);
    let before = rows(&before.stdout);
    assert_eq!(
        before.len(),
        data.iter().filter(|r| r["kind"] == "payload").count()
    );
    let metadata: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(metadata["uds_counts"]["positive"], 1);
    assert_eq!(metadata["scan_complete"], true);
}
#[test]
fn cli_handles_pending_gap_eof_and_policy_identity_without_overwriting_inputs() {
    let (dir,source,route,policy)=fixture("0.0 1 7E0 Tx d 4 03 22 F1 90\n0.02 1 7E8 Rx d 4 03 7F 22 78\n0.03 SV: 1 0 1 ::Test::Unknown = 1\n");
    let out = cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "--unsupported",
            "skip",
        ],
        3,
    );
    let data = rows(&out.stdout);
    assert!(data.iter().any(|r| r["kind"] == "uds_pending"));
    let closed = data
        .iter()
        .find(|r| r["kind"] == "uds_transaction")
        .unwrap();
    assert_eq!(closed["status"], "incomplete");
    assert_eq!(closed["reason"], "capture_gap");
    let saved = fs::read(&policy).unwrap();
    cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "-o",
            path(&policy),
            "--overwrite",
        ],
        1,
    );
    assert_eq!(fs::read(&policy).unwrap(), saved);
    let link = dir.path().join("policy-link.json");
    fs::hard_link(&policy, &link).unwrap();
    cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "--report",
            path(&link),
        ],
        1,
    );
    assert_eq!(fs::read(policy).unwrap(), saved);
}
#[test]
fn cdd_opt_in_and_paths_are_guarded_before_publication() {
    let (dir, source, route, policy) = fixture("0 1 7E0 Tx d 4 03 22 F1 90\n");
    let cdd = dir.path().join("model.cdd");
    fs::write(&cdd, b"original").unwrap();
    let mut config: Value = serde_json::from_slice(&fs::read(&policy).unwrap()).unwrap();
    config["routes"][0]["cdd"] =
        json!({"path":"model.cdd","ecu":"ecu","variant":"variant","allow_experimental":false});
    fs::write(&policy, serde_json::to_vec(&config).unwrap()).unwrap();
    cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "-o",
            path(&cdd),
            "--overwrite",
        ],
        1,
    );
    cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "--report",
            path(&cdd),
        ],
        1,
    );
    assert_eq!(fs::read(&cdd).unwrap(), b"original");
    let output = dir.path().join("out.jsonl");
    cli(
        &[
            "uds",
            path(&source),
            "--routes",
            path(&route),
            "--policy",
            path(&policy),
            "-o",
            path(&output),
        ],
        1,
    );
    assert!(!output.exists());
}
