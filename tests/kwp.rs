#[cfg(feature = "cdd")]
use canlog::uds::Status;
use canlog::{
    core::Location,
    isotp::{Config as Transport, Direction, Event, FlowObservation},
    uds::{Config, Matcher},
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
#[test]
fn without_cdd_does_not_interpret_or_reserve_raw_kwp_payloads() {
    let mut m = matcher();
    for (direction, data) in [
        (Direction::Request, vec![0x82, 0x90]),
        (Direction::Response, vec![0xc2, 0x90, 1]),
    ] {
        assert!(m.consume(&packet(direction, 0, &data)).unwrap().is_empty());
        assert_eq!(m.outstanding(), 0);
        assert_eq!(m.reserved_request_bytes(), 0);
    }
}
#[cfg(feature = "cdd")]
fn native(dir: &TempDir, xml: &str) -> (Matcher, canlog::cdd::Decoder) {
    let path = dir.path().join("native.cdd");
    fs::write(&path, xml).unwrap();
    let mut p = policy();
    p["routes"][0]["cdd"] = json!({"path":path,"ecu":"E","variant":"V","allow_experimental":true});
    let config: Config = serde_json::from_value(p).unwrap();
    let decoder = canlog::cdd::Decoder::load(&config, &Default::default()).unwrap();
    (
        Matcher::new(
            config,
            &serde_json::from_value::<Transport>(routes()).unwrap(),
            "timeline",
        )
        .unwrap(),
        decoder,
    )
}
#[cfg(feature = "cdd")]
fn consume(
    m: &mut Matcher,
    d: &canlog::cdd::Decoder,
    direction: Direction,
    time: i64,
    bytes: &[u8],
) -> Vec<canlog::uds::Observation> {
    m.consume_with_cdd(&packet(direction, time, bytes), Some(d))
        .unwrap()
}
#[cfg(feature = "cdd")]
const NATIVE: &str = include_str!("fixtures/kwp-native.cdd");
#[cfg(feature = "cdd")]
#[test]
fn cdd_decides_service_identity_including_sids_outside_the_removed_profile() {
    for (xml, sid, response_sid) in [
        (NATIVE.to_owned(), 0x1a, 0x5a),
        (
            NATIVE
                .replace("spec='sid' bl='8' v='26'", "spec='sid' bl='8' v='130'")
                .replace("spec='sid' bl='8' v='90'", "spec='sid' bl='8' v='194'"),
            0x82,
            0xc2,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let (mut m, d) = native(&dir, &xml);
        assert!(consume(&mut m, &d, Direction::Request, 0, &[sid, 0x90]).is_empty());
        let row = consume(
            &mut m,
            &d,
            Direction::Response,
            10,
            &[response_sid, 0x90, 0x12, 0x34],
        )
        .remove(0);
        assert_eq!(row.status, Status::Positive);
        assert_eq!(row.header.as_ref().unwrap().service_id, sid);
        assert!(row.header.unwrap().service_key.is_some());
        let decoded = row.cdd.unwrap();
        assert_eq!(decoded.status, "decoded");
        assert_eq!(
            decoded.response.as_ref().unwrap()["provenance"]["protocol"],
            "kwp2000"
        );
        assert_eq!(
            decoded.response.unwrap()["fields"][1]["raw"]["value"],
            "4660"
        );
        assert_eq!(m.outstanding(), 0);
        assert_eq!(m.reserved_request_bytes(), 0);
    }
}
#[cfg(feature = "cdd")]
#[test]
fn undefined_requests_and_invalid_echo_or_length_never_use_fallback_rules() {
    let dir = TempDir::new().unwrap();
    let (mut m, d) = native(&dir, NATIVE);
    let row = consume(&mut m, &d, Direction::Request, 0, &[0x10, 0]).remove(0);
    assert_eq!(row.status, Status::Unsupported);
    assert_eq!(row.cdd.unwrap().status, "no_match");
    consume(&mut m, &d, Direction::Request, 1, &[0x1a, 0x90]);
    for data in [
        vec![0x5a, 0x92, 0x12, 0x34],
        vec![0x5a, 0x90, 0x12],
        vec![0x5a, 0x90, 0x12, 0x34, 0],
    ] {
        assert_eq!(
            consume(&mut m, &d, Direction::Response, 10, &data)[0].status,
            Status::Orphan
        );
        assert_eq!(m.outstanding(), 1);
    }
    assert_eq!(m.eof(20)[0].status, Status::Incomplete);
}
#[cfg(feature = "cdd")]
#[test]
fn native_negative_pending_extends_deadline_and_preserves_nrc_context() {
    let dir = TempDir::new().unwrap();
    let (mut m, d) = native(&dir, NATIVE);
    consume(&mut m, &d, Direction::Request, 0, &[0x1a, 0x90]);
    let row = consume(&mut m, &d, Direction::Response, 10, &[0x7f, 0x1a, 0x78]).remove(0);
    assert_eq!(row.status, Status::Pending);
    assert_eq!(row.nrc, Some(0x78));
    assert_eq!(row.p2_deadline_ns, Some(110));
    assert_eq!(row.cdd.unwrap().status, "decoded");
    assert!(m.advance(60, &[]).is_empty());
    let row = consume(&mut m, &d, Direction::Response, 80, &[0x7f, 0x1a, 0x31]).remove(0);
    assert_eq!(row.status, Status::Negative);
    assert_eq!(row.nrc, Some(0x31));
    assert_eq!(row.pending_count, 1);
    assert_eq!(m.outstanding(), 0);
}
#[cfg(feature = "cdd")]
#[test]
fn native_matching_does_not_assign_ambiguous_negative_or_duplicate_requests_fifo() {
    for response in [
        vec![0x7f, 0x1a, 0x31],
        vec![0x7f, 0x1a, 0x78],
        vec![0x5a, 0x90, 0x12, 0x34],
    ] {
        let dir = TempDir::new().unwrap();
        let (mut m, d) = native(&dir, NATIVE);
        consume(&mut m, &d, Direction::Request, 0, &[0x1a, 0x90]);
        consume(&mut m, &d, Direction::Request, 1, &[0x1a, 0x90]);
        let rows = consume(&mut m, &d, Direction::Response, 10, &response);
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|r| r.status == Status::Ambiguous && r.candidate_transaction_keys.len() == 2));
        assert_eq!(m.outstanding(), 0);
        assert_eq!(m.reserved_request_bytes(), 0);
    }
    let dir = TempDir::new().unwrap();
    let (mut m, d) = native(&dir, NATIVE);
    consume(&mut m, &d, Direction::Request, 0, &[0x1a, 0x90]);
    consume(&mut m, &d, Direction::Request, 1, &[0x1a, 0x92]);
    let row = consume(
        &mut m,
        &d,
        Direction::Response,
        10,
        &[0x5a, 0x92, 0x12, 0x34],
    )
    .remove(0);
    assert_eq!(row.status, Status::Positive);
    assert_eq!(row.request.unwrap().data_hex, "1A92");
    assert_eq!(m.outstanding(), 1);
}
#[cfg(feature = "cdd")]
#[test]
fn unsupported_native_response_layout_is_unverified_and_timeout_stays_bounded() {
    let dir = TempDir::new().unwrap();
    let (mut m, d) = native(&dir, &NATIVE.replace("dtref='_u16'", "dtref='_missing'"));
    consume(&mut m, &d, Direction::Request, 0, &[0x1a, 0x90]);
    let row = consume(
        &mut m,
        &d,
        Direction::Response,
        10,
        &[0x5a, 0x90, 0x12, 0x34],
    )
    .remove(0);
    assert_eq!(row.status, Status::Ambiguous);
    assert_eq!(row.cdd.unwrap().status, "unverified");
    let dir = TempDir::new().unwrap();
    let (mut m, d) = native(&dir, NATIVE);
    consume(&mut m, &d, Direction::Request, 0, &[0x1a, 0x90]);
    assert_eq!(m.advance(51, &[])[0].status, Status::NoResponseObserved);
    assert_eq!(m.reserved_request_bytes(), 0);
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
    assert!(rows.iter().all(|r| r["kind"] == "payload"));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["data_hex"], "1081");
    assert_eq!(rows[1]["data_hex"], "5081");
    let metadata: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(metadata["kwp_counts"], json!({}));
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
