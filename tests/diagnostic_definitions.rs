use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn fixture(dir: &Path) {
    fs::write(dir.join("source.asc"), "base hex timestamps absolute\nno internal events logged\n0.0 1 700 Tx d 8 02 1A 90 00 00 00 00 00\n0.01 1 600 Rx d 8 04 5A 90 12 34 00 00 00\n").unwrap();
    fs::write(dir.join("network.dbc"), "VERSION \"\"\nNS_ :\nBS_:\nBU_: Tester ECU\nBO_ 1792 Request: 8 Tester\n SG_ PciLength : 0|8@1+ (1,0) [0|255] \"\" ECU\nBO_ 1536 Response: 8 ECU\n SG_ PciLength : 0|8@1+ (1,0) [0|255] \"\" Tester\n").unwrap();
    fs::write(dir.join("routes.json"), json!({"schema_version":1,"routes":[{"name":"ecu","channel":1,"kind":"physical","profile":"classic","addressing":"normal","request":{"id":1792,"extended":false},"response":{"id":1536,"extended":false}}]}).to_string()).unwrap();
    fs::write(dir.join("policy.json"), json!({"schema_version":1,"routes":[{"route":"ecu","protocol":"kwp2000_vector","p2_ns":50_000_000,"p2_star_ns":100_000_000,"transaction_max_duration_ns":500_000_000}]}).to_string()).unwrap();
    fs::write(dir.join("model.cdd"), "authored CDD placeholder").unwrap();
}
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_canlog"))
        .current_dir(dir)
        .args([
            "kwp",
            "source.asc",
            "--routes",
            "routes.json",
            "--policy",
            "policy.json",
        ])
        .args(args)
        .output()
        .unwrap()
}
fn parse_rows(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn replay(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_canlog"));
    command
        .current_dir(dir)
        .args([
            "replay",
            "source.asc",
            "--routes",
            "routes.json",
            "--policy",
            "policy.json",
            "--protocol",
            "kwp2000-vector",
            "--dbc",
            "1=network.dbc",
        ])
        .args(args);
    command
}

#[test]
fn diagnostic_replay_emits_live_rows_at_scaled_time_and_keeps_analysis_values() {
    let dir = TempDir::new().unwrap();
    fixture(dir.path());
    let source = fs::read_to_string(dir.path().join("source.asc"))
        .unwrap()
        .replace("0.01 1", "0.4 1");
    fs::write(dir.path().join("source.asc"), source).unwrap();
    let mut policy: Value =
        serde_json::from_slice(&fs::read(dir.path().join("policy.json")).unwrap()).unwrap();
    policy["routes"][0]["p2_ns"] = json!(500_000_000);
    policy["routes"][0]["transaction_max_duration_ns"] = json!(1_000_000_000);
    fs::write(dir.path().join("policy.json"), policy.to_string()).unwrap();
    let analysis = run(dir.path(), &["--dbc", "1=network.dbc"]);
    assert!(analysis.status.success());
    let mut child = replay(
        dir.path(),
        &["--speed", "2", "--report", "replay.report.json"],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let mut stream = BufReader::new(child.stdout.take().unwrap());
    let mut bytes = Vec::new();
    stream.read_until(b'\n', &mut bytes).unwrap();
    let first: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(first["kind"], "decoded_frame");
    let first_at = Instant::now();
    stream.read_to_end(&mut bytes).unwrap();
    assert!(
        first_at.elapsed() >= Duration::from_millis(140),
        "rows were buffered until EOF or playback was not paced"
    );
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(bytes, analysis.stdout);
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("replay.report.json")).unwrap()).unwrap();
    assert_eq!(report["replay"]["speed"], 2.0);
    assert_eq!(report["replay"]["no_wait"], false);
    assert_eq!(report["kwp_counts"]["positive"], 1);
    let instant = replay(
        dir.path(),
        &[
            "--no-wait",
            "-o",
            "replay.jsonl",
            "--report",
            "instant.report.json",
        ],
    )
    .output()
    .unwrap();
    assert!(instant.status.success());
    assert_eq!(
        fs::read(dir.path().join("replay.jsonl")).unwrap(),
        analysis.stdout
    );
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("instant.report.json")).unwrap()).unwrap();
    assert_eq!(report["replay"]["no_wait"], true);
    assert_eq!(report["published"], true);
}

#[test]
fn diagnostic_replay_rejects_unsupported_settings_and_protects_definitions() {
    let dir = TempDir::new().unwrap();
    fixture(dir.path());
    let original = fs::read(dir.path().join("network.dbc")).unwrap();
    for args in [
        vec!["--repeat", "2"],
        vec!["--repeat-gap", "0.1"],
        vec!["--speed", "0"],
        vec!["--sink", "csv"],
        vec!["--format", "asc", "-o", "must-not-exist.asc"],
        vec!["--sink", "console", "-o", "must-not-exist.jsonl"],
        vec!["--sync"],
    ] {
        assert_eq!(
            replay(dir.path(), &args).output().unwrap().status.code(),
            Some(2)
        );
    }
    for args in [
        vec!["-o", "network.dbc", "--overwrite"],
        vec!["--report", "network.dbc"],
        vec!["--limit", "1", "-o", "must-not-exist.jsonl"],
    ] {
        assert_eq!(
            replay(dir.path(), &args).output().unwrap().status.code(),
            Some(1)
        );
    }
    assert!(!dir.path().join("must-not-exist.asc").exists());
    assert!(!dir.path().join("must-not-exist.jsonl").exists());
    assert_eq!(fs::read(dir.path().join("network.dbc")).unwrap(), original);
}

#[test]
fn diagnostic_replay_stop_keeps_existing_output_unpublished() {
    let dir = TempDir::new().unwrap();
    fixture(dir.path());
    let source = fs::read_to_string(dir.path().join("source.asc"))
        .unwrap()
        .replace("0.01 1", "10.0 1");
    fs::write(dir.path().join("source.asc"), source).unwrap();
    fs::write(dir.path().join("output.jsonl"), "previous output").unwrap();
    let mut child = replay(
        dir.path(),
        &[
            "--control-stdin",
            "-o",
            "output.jsonl",
            "--overwrite",
            "--report",
            "stopped.report.json",
        ],
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("output.jsonl")).unwrap(),
        "previous output"
    );
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("stopped.report.json")).unwrap()).unwrap();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["published"], false);
    assert_eq!(report["scan_complete"], false);
}

#[test]
fn combined_dbc_and_kwp_rows_share_identity_and_keep_independent_quality() {
    let dir = TempDir::new().unwrap();
    fixture(dir.path());
    let result = run(
        dir.path(),
        &["--dbc", "1=network.dbc", "--report", "report.json"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let rows = parse_rows(&result);
    let frames: Vec<_> = rows
        .iter()
        .filter(|r| r["kind"] == "decoded_frame")
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["message"], "Request");
    assert_eq!(frames[1]["message"], "Response");
    assert_eq!(frames[0]["signals"][0]["raw_text"], "2");
    assert_eq!(frames[1]["signals"][0]["raw_text"], "4");
    let transaction = rows
        .iter()
        .find(|r| r["kind"] == "kwp_transaction")
        .unwrap();
    assert_eq!(
        transaction["request"]["first_location"],
        frames[0]["record"]["location"]
    );
    assert_eq!(
        transaction["response"]["first_location"],
        frames[1]["record"]["location"]
    );
    assert!(rows
        .iter()
        .all(|r| r["timeline_key"] == transaction["timeline_key"]));
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report["output_rows"], 5);
    assert_eq!(report["dbc_counts"]["decoded"], 2);
    assert_eq!(report["kwp_counts"]["positive"], 1);
    assert!(report["dbc_engine_revision"].is_string());
    let missing = run(dir.path(), &["--dbc", "2=network.dbc"]);
    assert_eq!(missing.status.code(), Some(3));
    assert!(parse_rows(&missing)
        .iter()
        .filter(|r| r["kind"] == "decoded_frame")
        .all(|r| r["status"] == "no_database"));
    assert_ne!(
        parse_rows(&missing)[0]["timeline_key"],
        frames[0]["timeline_key"]
    );

    fs::write(dir.path().join("source.asc"), "base hex timestamps absolute\nno internal events logged\n0.0 1 700 Tx d 8 03 22 F1 90 00 00 00 00\n0.01 1 600 Rx d 8 05 62 F1 90 12 34 00 00\n").unwrap();
    let policy = fs::read_to_string(dir.path().join("policy.json"))
        .unwrap()
        .replace("kwp2000_vector", "uds2013");
    fs::write(dir.path().join("policy.json"), policy).unwrap();
    let uds = Command::new(env!("CARGO_BIN_EXE_canlog"))
        .current_dir(dir.path())
        .args([
            "uds",
            "source.asc",
            "--routes",
            "routes.json",
            "--policy",
            "policy.json",
            "--dbc",
            "1=network.dbc",
        ])
        .output()
        .unwrap();
    assert!(
        uds.status.success(),
        "{}",
        String::from_utf8_lossy(&uds.stderr)
    );
    assert_eq!(
        parse_rows(&uds)
            .iter()
            .filter(|r| r["kind"] == "decoded_frame")
            .count(),
        2
    );
    assert_eq!(
        parse_rows(&uds)
            .iter()
            .find(|r| r["kind"] == "uds_transaction")
            .unwrap()["status"],
        "positive"
    );
}

#[test]
fn definition_outputs_reports_and_incomplete_cli_assignments_are_guarded() {
    let dir = TempDir::new().unwrap();
    fixture(dir.path());
    let dbc = fs::read(dir.path().join("network.dbc")).unwrap();
    let cdd = fs::read(dir.path().join("model.cdd")).unwrap();
    for args in [
        vec!["--dbc", "1=network.dbc", "-o", "network.dbc", "--overwrite"],
        vec!["--dbc", "1=network.dbc", "--report", "network.dbc"],
        vec!["--dbc", "not-a-binding", "-o", "must-not-exist.jsonl"],
        vec![
            "--dbc",
            "1=network.dbc",
            "--cdd",
            "model.cdd",
            "--ecu",
            "E",
            "--variant",
            "V",
            "--allow-experimental",
            "-o",
            "model.cdd",
            "--overwrite",
        ],
        vec![
            "--cdd",
            "model.cdd",
            "--ecu",
            "E",
            "--variant",
            "V",
            "--allow-experimental",
            "--report",
            "model.cdd",
        ],
    ] {
        assert_eq!(run(dir.path(), &args).status.code(), Some(1));
    }
    assert_eq!(
        run(dir.path(), &["--cdd", "model.cdd"]).status.code(),
        Some(2)
    );
    assert_eq!(run(dir.path(), &["--ecu", "E"]).status.code(), Some(2));
    assert!(!dir.path().join("must-not-exist.jsonl").exists());
    assert_eq!(fs::read(dir.path().join("network.dbc")).unwrap(), dbc);
    assert_eq!(fs::read(dir.path().join("model.cdd")).unwrap(), cdd);
}

#[cfg(feature = "cdd")]
#[test]
fn one_cli_reads_asc_dbc_and_cdd_with_explicit_ecu_and_variant() {
    let dir = TempDir::new().unwrap();
    fixture(dir.path());
    // Authored minimal KWP definition; external Vector samples are exercised separately.
    fs::write(dir.path().join("model.cdd"), "<CANDELA><ECUDOC><PROTOCOLSTANDARD>KWP</PROTOCOLSTANDARD><DATATYPES><IDENT id='_u16'><QUAL>NumberType</QUAL><CVALUETYPE bl='16' bo='21' enc='uns' qty='atom' minsz='1' maxsz='1'/></IDENT></DATATYPES><PROTOCOLSERVICES><PROTOCOLSERVICE id='_ps'><QUAL>ReadIdentification</QUAL><REQ><CONSTCOMP spec='sid' bl='8' v='26'/><STATICCOMP id='_req_id' spec='sub' bl='8'><QUAL>LocalIdentifier</QUAL></STATICCOMP></REQ><POS><CONSTCOMP spec='sid' bl='8' v='90'/><STATICCOMP id='_pos_id' spec='sub' bl='8'><QUAL>LocalIdentifier</QUAL></STATICCOMP><SIMPLEPROXYCOMP id='_data' dest='data' minbl='16'><QUAL>Data</QUAL></SIMPLEPROXYCOMP></POS></PROTOCOLSERVICE></PROTOCOLSERVICES><DCLTMPLS><DCLTMPL id='_t'><DCLSRVTMPL id='_s' tmplref='_ps'/><SHSTATIC id='_id' spec='sub'><STATICCOMPREF idref='_req_id'/><STATICCOMPREF idref='_pos_id'/></SHSTATIC><SHPROXY id='_fill' dest='data'><PROXYCOMPREF idref='_data'/></SHPROXY></DCLTMPL></DCLTMPLS><ECU><QUAL>E</QUAL><VAR><QUAL>V</QUAL><DIAGCLASS tmplref='_t'><QUAL>Identification</QUAL><DIAGINST tmplref='_t'><QUAL>Number</QUAL><SERVICE tmplref='_s'><QUAL>Read</QUAL></SERVICE><STATICVALUE shstaticref='_id' v='144'/><SIMPLECOMPCONT shproxyref='_fill'><DATAOBJ spec='no' dtref='_u16'><QUAL>Number</QUAL></DATAOBJ></SIMPLECOMPCONT></DIAGINST></DIAGCLASS></VAR></ECU></ECUDOC></CANDELA>").unwrap();
    let result = run(
        dir.path(),
        &[
            "--dbc",
            "1=network.dbc",
            "--cdd",
            "model.cdd",
            "--ecu",
            "E",
            "--variant",
            "V",
            "--allow-experimental",
            "--report",
            "report.json",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let rows = parse_rows(&result);
    let transaction = rows
        .iter()
        .find(|r| r["kind"] == "kwp_transaction")
        .unwrap();
    assert_eq!(transaction["cdd"]["status"], "decoded");
    assert_eq!(
        transaction["cdd"]["response"]["fields"][1]["raw"]["value"],
        "4660"
    );
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report["cdd_counts"]["decoded"], 1);
    assert_eq!(report["dbc_counts"]["decoded"], 2);
    assert!(report["kwp_policy"]["routes"][0]["cdd"]["path"]
        .as_str()
        .unwrap()
        .ends_with("model.cdd"));
    let replay_options = [
        "--no-wait",
        "--cdd",
        "model.cdd",
        "--ecu",
        "E",
        "--variant",
        "V",
        "--allow-experimental",
    ];
    let replayed = replay(dir.path(), &replay_options).output().unwrap();
    assert!(
        replayed.status.success(),
        "{}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert_eq!(replayed.stdout, result.stdout);
    let console = replay(dir.path(), &replay_options)
        .args(["--sink", "console"])
        .output()
        .unwrap();
    assert!(console.status.success());
    let text = String::from_utf8(console.stdout).unwrap();
    assert!(
        text.contains("Request") && text.contains("Response") && text.contains("CDD [decoded]")
    );
    assert!(text.contains("/Number = 4660"));
    let mut policy: Value =
        serde_json::from_slice(&fs::read(dir.path().join("policy.json")).unwrap()).unwrap();
    policy["routes"][0]["cdd"] =
        json!({"path":"model.cdd","ecu":"E","variant":"V","allow_experimental":true});
    fs::write(dir.path().join("policy.json"), policy.to_string()).unwrap();
    let duplicate = run(
        dir.path(),
        &[
            "--cdd",
            "model.cdd",
            "--ecu",
            "E",
            "--variant",
            "V",
            "--allow-experimental",
            "-o",
            "must-not-exist.jsonl",
        ],
    );
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(!dir.path().join("must-not-exist.jsonl").exists());
}
