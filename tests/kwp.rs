use canlog::{
    core::Location,
    isotp::{Config as Transport, Direction, Event, FlowObservation},
    uds::{Config, Matcher, Status},
};
use serde_json::{json, Value};
use std::{fs, process::Command};
use tempfile::TempDir;

fn routes() -> Value {
    json!({"schema_version":1,"routes":[{"name":"ecu","channel":1,"kind":"physical","profile":"classic","addressing":"normal","request":{"id":1792,"extended":false},"response":{"id":1536,"extended":false}}]})
}
fn policy() -> Value {
    json!({"schema_version":1,"routes":[{"route":"ecu","protocol":"kwp2000_vector","p2_ns":50,"p2_star_ns":100,"transaction_max_duration_ns":500}]})
}
fn matcher() -> Matcher {
    Matcher::new(
        serde_json::from_value(policy()).unwrap(),
        &serde_json::from_value::<Transport>(routes()).unwrap(),
        "timeline",
    )
    .unwrap()
}
fn packet(direction: Direction, time: i64, bytes: &[u8]) -> Event {
    let location = Location {
        source: "input.asc".into(),
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
        declared_length: Some(bytes.len()),
        observed_length: bytes.len(),
        data_hex: bytes.iter().map(|b| format!("{b:02X}")).collect(),
        data_locations: vec![location],
        flow_control: FlowObservation::default(),
        fc_observation: "not_required".into(),
        protocol_compliance: "unknown".into(),
        capture_gaps_before: 0,
    }
}
fn request(m: &mut Matcher, time: i64, bytes: &[u8]) {
    assert!(m
        .consume(&packet(Direction::Request, time, bytes))
        .unwrap()
        .is_empty());
}
#[test]
fn kwp_keeps_full_mode_bytes_and_uses_its_own_echo_profile() {
    for (req, resp) in [
        (vec![0x10, 0x81], vec![0x50, 0x81]),
        (vec![0x10, 0], vec![0x50, 0]),
        (vec![0x11, 0], vec![0x51, 0, 0]),
        (vec![0x14, 0xff, 0], vec![0x54, 0xff, 0]),
        (vec![0x18, 2, 0xff, 0], vec![0x58, 1, 0x90, 2, 0x99]),
        (vec![0x1a, 0x90], vec![0x5a, 0x90, 1]),
        (vec![0x20], vec![0x60]),
        (vec![0x21, 0xa0], vec![0x61, 0xa0, 1]),
        (vec![0x3b, 0xa0, 1], vec![0x7b, 0xa0]),
        (vec![0x3e, 1], vec![0x7e]),
    ] {
        let mut m = matcher();
        request(&mut m, 0, &req);
        let row = m
            .consume(&packet(Direction::Response, 10, &resp))
            .unwrap()
            .remove(0);
        assert_eq!(row.status, Status::Positive, "{req:?}");
        assert_eq!(row.kind, "kwp_transaction");
        assert!(!row.header.as_ref().unwrap().suppress_positive_response);
        assert_eq!(
            serde_json::to_value(&row).unwrap()["protocol"],
            "kwp2000_vector"
        );
        if req[0] == 0x10 {
            assert_eq!(row.header.unwrap().diagnostic_mode, Some(req[1]));
        }
    }
}
#[test]
fn wrong_local_id_group_or_mode_never_matches() {
    for (req, resp) in [
        (vec![0x10, 0x81], vec![0x50, 1]),
        (vec![0x11, 0], vec![0x51, 1]),
        (vec![0x14, 0xff, 0], vec![0x54, 0, 0]),
        (vec![0x1a, 0x90], vec![0x5a, 0x92, 1]),
        (vec![0x21, 0xa0], vec![0x61, 0xa1, 1]),
        (vec![0x3b, 0xa0, 1], vec![0x7b, 0xa1]),
    ] {
        let mut m = matcher();
        request(&mut m, 0, &req);
        assert_eq!(
            m.consume(&packet(Direction::Response, 10, &resp)).unwrap()[0].status,
            Status::Orphan
        );
        assert_eq!(m.outstanding(), 1);
    }
}
#[test]
fn response_required_and_no_response_modes_are_distinct_from_uds_suppress_bit() {
    let mut m = matcher();
    request(&mut m, 0, &[0x3e, 1]);
    assert_eq!(m.advance(51, &[])[0].status, Status::NoResponseObserved);
    let mut m = matcher();
    request(&mut m, 0, &[0x3e, 2]);
    assert_eq!(m.advance(51, &[])[0].status, Status::SuppressedExpected);
    let mut m = matcher();
    request(&mut m, 0, &[0x3e, 2]);
    assert_eq!(m.eof(20)[0].status, Status::Incomplete);
    let mut m = matcher();
    let row = m
        .consume(&packet(Direction::Request, 0, &[0x3e, 0x81]))
        .unwrap()
        .remove(0);
    assert_eq!(row.status, Status::Unsupported);
    assert_eq!(row.kind, "kwp_issue");
}
#[test]
fn pending_negative_and_dtc_without_echo_need_unique_context() {
    let mut m = matcher();
    request(&mut m, 0, &[0x1a, 0x90]);
    let pending = m
        .consume(&packet(Direction::Response, 10, &[0x7f, 0x1a, 0x78]))
        .unwrap()
        .remove(0);
    assert_eq!(pending.kind, "kwp_pending");
    assert_eq!(pending.status, Status::Pending);
    assert_eq!(
        m.consume(&packet(Direction::Response, 80, &[0x5a, 0x90, 1]))
            .unwrap()[0]
            .status,
        Status::Positive
    );
    for response in [vec![0x7f, 0x1a, 0x31], vec![0x7f, 0x1a, 0x78]] {
        let mut m = matcher();
        request(&mut m, 0, &[0x1a, 0x90]);
        request(&mut m, 1, &[0x1a, 0x92]);
        let rows = m
            .consume(&packet(Direction::Response, 10, &response))
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.status == Status::Ambiguous));
    }
    let mut m = matcher();
    request(&mut m, 0, &[0x18, 2, 0xff, 0]);
    request(&mut m, 1, &[0x18, 3, 0xff, 0]);
    assert!(m
        .consume(&packet(Direction::Response, 10, &[0x58, 0]))
        .unwrap()
        .iter()
        .all(|r| r.status == Status::Ambiguous));
}
#[test]
fn malformed_dtc_counts_and_uds_service_headers_remain_visible() {
    let mut m = matcher();
    request(&mut m, 0, &[0x18, 2, 0xff, 0]);
    for bytes in [
        vec![0x58, 2, 0x90, 2, 0x99],
        vec![0x7e, 1],
        vec![0x50, 0x81, 0, 50, 1, 0xf4],
        vec![0x54],
    ] {
        assert_eq!(
            m.consume(&packet(Direction::Response, 10, &bytes)).unwrap()[0].status,
            Status::Malformed
        );
    }
    assert_eq!(
        m.consume(&packet(Direction::Request, 11, &[0x22, 0xf1, 0x90]))
            .unwrap()
            .last()
            .unwrap()
            .status,
        Status::Unsupported
    );
    assert_eq!(m.outstanding(), 0);
}
#[test]
fn cli_separates_protocol_policy_and_report_counts() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source.asc");
    let route = dir.path().join("routes.json");
    let p = dir.path().join("policy.json");
    let report = dir.path().join("report.json");
    fs::write(&source,"base hex timestamps absolute\nno internal events logged\n0.0 1 700 Tx d 3 02 10 81\n0.01 1 600 Tx d 3 02 50 81\n").unwrap();
    fs::write(&route, serde_json::to_vec(&routes()).unwrap()).unwrap();
    let mut config = policy();
    config["routes"][0]["p2_ns"] = json!(50_000_000);
    config["routes"][0]["p2_star_ns"] = json!(100_000_000);
    config["routes"][0]["transaction_max_duration_ns"] = json!(500_000_000);
    fs::write(&p, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .arg("kwp")
        .arg(&source)
        .arg("--routes")
        .arg(&route)
        .arg("--policy")
        .arg(&p)
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        rows.iter()
            .filter(|r| r["kind"] == "kwp_transaction")
            .count(),
        1
    );
    let metadata: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(metadata["kwp_counts"]["positive"], 1);
    assert!(metadata.get("uds_counts").is_none());
    assert!(metadata.get("kwp_policy_sha256").is_some());
    let output = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .arg("uds")
        .arg(&source)
        .arg("--routes")
        .arg(&route)
        .arg("--policy")
        .arg(&p)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("protocol must match"));
    config["routes"][0]["protocol"] = json!("uds2013");
    fs::write(&p, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .arg("kwp")
        .arg(&source)
        .arg("--routes")
        .arg(&route)
        .arg("--policy")
        .arg(&p)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let _: Config = serde_json::from_value(policy()).unwrap();
}
