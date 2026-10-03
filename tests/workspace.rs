use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_canlog"))
        .args(args)
        .output()
        .unwrap()
}
fn ok(out: &std::process::Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn db(path: &Path, factor: u8) {
    fs::write(path,format!("VERSION \"\"\nNS_ :\nBS_:\nBU_: Sender Receiver\nBO_ 256 Msg: 8 Sender\n SG_ X : 0|64@1+ ({factor},0) [0|18446744073709551615] \"V\" Receiver\n")).unwrap();
}
fn fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let d = TempDir::new().unwrap();
    let root = d.path().join("workspace");
    let source = d.path().join("in.asc");
    let dbc = d.path().join("in.dbc");
    fs::write(&source,"base hex timestamps absolute\n0.1 1 100 Rx d 8 FF FF FF FF FF FF FF FF\n0.2 1 100 Rx d 8 02 00 00 00 00 00 00 00\n0.3 2 100 Tx d 8 03 00 00 00 00 00 00 00\n").unwrap();
    db(&dbc, 1);
    ok(&cli(&["workspace", "create", p(&root)]), 0);
    ok(
        &cli(&["workspace", "add", p(&root), p(&source), "--name", "first"]),
        0,
    );
    ok(
        &cli(&[
            "workspace",
            "bind",
            p(&root),
            "first",
            "--dbc",
            &format!("1={}", p(&dbc)),
            "--dbc",
            &format!("2={}", p(&dbc)),
        ]),
        0,
    );
    (d, root, source, dbc)
}
fn info(root: &Path) -> Value {
    let out = cli(&["workspace", "cache", p(root), "info"]);
    ok(&out, 0);
    json(&out.stdout)
}
fn decode(root: &Path, report: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "workspace",
        "decode",
        p(root),
        "first",
        "--report",
        p(report),
    ];
    args.extend_from_slice(extra);
    cli(&args)
}

#[test]
fn cold_warm_and_bypass_preserve_values_and_u64_and_workspace_configuration() {
    let (d, root, _, _) = fixture();
    let manifest = fs::read(root.join("workspace.json")).unwrap();
    let cold = decode(&root, &d.path().join("cold.json"), &[]);
    ok(&cold, 0);
    let warm = decode(&root, &d.path().join("warm.json"), &[]);
    ok(&warm, 0);
    assert_eq!(rows(&cold.stdout), rows(&warm.stdout));
    let bypass = decode(&root, &d.path().join("bypass.json"), &["--no-cache"]);
    ok(&bypass, 0);
    assert_eq!(rows(&cold.stdout), rows(&bypass.stdout));
    let a = json(&fs::read(d.path().join("cold.json")).unwrap());
    let b = json(&fs::read(d.path().join("warm.json")).unwrap());
    assert_eq!(a["cache"]["misses"], 3);
    assert_eq!(a["cache"]["rows_written"], 3);
    assert_eq!(b["cache"]["hits"], 3);
    assert_eq!(b["cache"]["misses"], 0);
    assert_eq!(
        rows(&warm.stdout)[0]["signals"][0]["raw"]["value"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        rows(&warm.stdout)[0]["signals"][0]["raw_text"],
        u64::MAX.to_string()
    );
    assert_eq!(info(&root)["rows"], 3);
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), manifest);
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    let payload: String = c
        .query_row("SELECT typed_json FROM signal_entries LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!payload.contains("\"data\""));
    assert!(!payload.contains("\"frame\""));
}

#[test]
fn source_dbc_and_policy_changes_invalidate_cache_without_mixing_revisions() {
    let (d, root, source, dbc) = fixture();
    ok(&decode(&root, &d.path().join("a.json"), &[]), 0);
    let text = fs::read_to_string(&source)
        .unwrap()
        .replace("02 00", "04 00");
    fs::write(&source, text).unwrap();
    let changed = decode(&root, &d.path().join("b.json"), &[]);
    ok(&changed, 0);
    assert_eq!(rows(&changed.stdout)[1]["signals"][0]["physical"], 4.0);
    assert_eq!(
        json(&fs::read(d.path().join("b.json")).unwrap())["cache"]["hits"],
        0
    );
    db(&dbc, 2);
    let dbc_changed = decode(&root, &d.path().join("c.json"), &[]);
    ok(&dbc_changed, 0);
    assert_eq!(rows(&dbc_changed.stdout)[1]["signals"][0]["physical"], 8.0);
    assert_eq!(
        json(&fs::read(d.path().join("c.json")).unwrap())["cache"]["hits"],
        0
    );
    ok(
        &decode(
            &root,
            &d.path().join("policy.json"),
            &["--unsupported", "skip", "--recover"],
        ),
        0,
    );
    assert_eq!(
        json(&fs::read(d.path().join("policy.json")).unwrap())["cache"]["hits"],
        0
    );
    assert_eq!(info(&root)["generations"], 4);
}

#[test]
fn multiple_logs_are_isolated_and_cli_overrides_replace_only_specified_channels() {
    let (d, root, source, dbc) = fixture();
    let second = d.path().join("second.asc");
    fs::copy(&source, &second).unwrap();
    let other = d.path().join("other.dbc");
    db(&other, 3);
    ok(
        &cli(&["workspace", "add", p(&root), p(&second), "--name", "second"]),
        0,
    );
    ok(
        &cli(&[
            "workspace",
            "bind",
            p(&root),
            "second",
            "--dbc",
            &format!("1={}", p(&other)),
            "--dbc",
            &format!("2={}", p(&dbc)),
        ]),
        0,
    );
    let a = decode(&root, &d.path().join("first.json"), &[]);
    ok(&a, 0);
    let b = cli(&["workspace", "decode", p(&root), "second"]);
    ok(&b, 0);
    assert_eq!(rows(&b.stdout)[1]["signals"][0]["physical"], 6.0);
    let override_flag = format!("1={}", p(&other));
    let override_out = decode(
        &root,
        &d.path().join("override.json"),
        &["--dbc", &override_flag],
    );
    ok(&override_out, 0);
    assert_eq!(rows(&override_out.stdout)[1]["signals"][0]["physical"], 6.0);
    assert_eq!(rows(&override_out.stdout)[2]["signals"][0]["physical"], 3.0);
    let again = decode(&root, &d.path().join("again.json"), &[]);
    ok(&again, 0);
    assert_eq!(rows(&a.stdout), rows(&again.stdout));
    assert_eq!(
        json(&fs::read(d.path().join("again.json")).unwrap())["cache"]["hits"],
        3
    );
}

#[test]
fn indexed_workspace_query_and_source_revision_catalog_match_direct_reader() {
    let (d, root, source, _) = fixture();
    let out = cli(&["workspace", "index", p(&root), "first", "--stride", "1"]);
    ok(&out, 0);
    let query = cli(&[
        "workspace",
        "query",
        p(&root),
        "first",
        "--start",
        "0.2",
        "--end",
        "0.3",
    ]);
    ok(&query, 0);
    let canonical = source.canonicalize().unwrap();
    let direct = cli(&[
        "view",
        p(&canonical),
        "--start",
        "0.2",
        "--end",
        "0.3",
        "--unlimited",
    ]);
    ok(&direct, 0);
    assert_eq!(rows(&query.stdout), rows(&direct.stdout));
    let a = decode(&root, &d.path().join("a.json"), &[]);
    ok(&a, 0);
    assert_eq!(
        json(&fs::read(d.path().join("a.json")).unwrap())["chunks_read"],
        3
    );
    fs::write(
        &source,
        fs::read_to_string(&source)
            .unwrap()
            .replace("02 00", "05 00"),
    )
    .unwrap();
    let b = decode(&root, &d.path().join("b.json"), &[]);
    ok(&b, 0);
    assert_eq!(rows(&b.stdout)[1]["signals"][0]["physical"], 5.0);
    assert!(json(&fs::read(d.path().join("b.json")).unwrap())["chunks_read"].is_null());
    ok(&cli(&["workspace", "index", p(&root), "first"]), 0);
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    let n: i64 = c
        .query_row("SELECT count(*) FROM source_revisions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(fs::read_dir(root.join("indexes")).unwrap().count(), 2);
    ok(
        &cli(&["workspace", "index", p(&root), "first", "--start", "0.2"]),
        1,
    );
}

#[test]
fn quality_is_recomputed_on_cache_hits_and_empty_selection_remains_degraded() {
    let (d, root, source, _) = fixture();
    let mut text = "base hex timestamps absolute\n".to_string();
    for _ in 0..140 {
        text.push_str("0.1 1 100 Rx d 8 01 00 00 00 00 00 00 00\n");
    }
    text.push_str("0.2 SV: event\nbad unknown timestamp\n");
    fs::write(&source, text).unwrap();
    let flags = ["--unsupported", "skip", "--recover"];
    let cold = decode(&root, &d.path().join("cold.json"), &flags);
    ok(&cold, 3);
    let warm = decode(&root, &d.path().join("warm.json"), &flags);
    ok(&warm, 3);
    assert_eq!(rows(&cold.stdout), rows(&warm.stdout));
    let report = json(&fs::read(d.path().join("warm.json")).unwrap());
    assert_eq!(report["issues_selected"], 2);
    assert_eq!(report["cache"]["hits"], 140);
    let strict = decode(&root, &d.path().join("strict.json"), &[]);
    ok(&strict, 1);
    assert_eq!(info(&root)["building_generations"], 0);
    assert_eq!(info(&root)["generations"], 1);
    let empty = decode(
        &root,
        &d.path().join("empty.json"),
        &[
            "--unsupported",
            "skip",
            "--recover",
            "--start",
            "0.8",
            "--end",
            "0.9",
        ],
    );
    ok(&empty, 3);
    assert!(empty.stdout.is_empty());
    assert_eq!(
        json(&fs::read(d.path().join("empty.json")).unwrap())["issues_selected"],
        1
    );
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    let q: String = c
        .query_row("SELECT quality FROM cache_generations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(q, "degraded");
}

#[test]
fn cache_coverage_is_lazy_across_ranges_and_limit_quality_is_unknown() {
    let (d, root, _, _) = fixture();
    ok(
        &decode(
            &root,
            &d.path().join("slice.json"),
            &["--start", "0.1", "--end", "0.3"],
        ),
        0,
    );
    assert_eq!(info(&root)["rows"], 2);
    ok(&decode(&root, &d.path().join("all.json"), &[]), 0);
    let report = json(&fs::read(d.path().join("all.json")).unwrap());
    assert_eq!(report["cache"]["hits"], 2);
    assert_eq!(report["cache"]["misses"], 1);
    assert_eq!(info(&root)["rows"], 3);
    ok(&cli(&["workspace", "cache", p(&root), "clear"]), 0);
    ok(
        &decode(&root, &d.path().join("limit.json"), &["--limit", "1"]),
        0,
    );
    assert_eq!(info(&root)["rows"], 1);
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    let q: String = c
        .query_row("SELECT quality FROM cache_generations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(q, "unknown");
}

#[test]
fn protected_workspace_files_other_logs_and_dbc_are_never_overwritten() {
    let (d, root, source, dbc) = fixture();
    for path in [
        root.join("workspace.json"),
        root.join("workspace.db"),
        source,
        dbc,
    ] {
        let before = fs::read(&path).unwrap();
        let out = cli(&[
            "workspace",
            "decode",
            p(&root),
            "first",
            "-o",
            p(&path),
            "--overwrite",
        ]);
        ok(&out, 1);
        assert_eq!(fs::read(path).unwrap(), before);
    }
    let old = fs::read(root.join("workspace.json")).unwrap();
    ok(&cli(&["workspace", "create", p(&root)]), 1);
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), old);
    let out = d.path().join("signals.jsonl");
    let alias = d.path().join(".").join("signals.jsonl");
    ok(
        &cli(&[
            "workspace",
            "decode",
            p(&root),
            "first",
            "-o",
            p(&out),
            "--report",
            p(&alias),
        ]),
        1,
    );
    assert!(!out.exists());
}

#[test]
fn invalid_binding_and_future_schemas_preserve_manifest_and_cache() {
    let (_d, root, source, dbc) = fixture();
    let before = fs::read(root.join("workspace.json")).unwrap();
    ok(
        &cli(&[
            "workspace",
            "bind",
            p(&root),
            "first",
            "--dbc",
            &format!("1={}", p(&dbc)),
            "--dbc",
            &format!("1={}", p(&dbc)),
        ]),
        1,
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
    ok(
        &cli(&[
            "workspace",
            "add",
            p(&root),
            p(&source),
            "--name",
            "duplicate",
        ]),
        1,
    );
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    c.pragma_update(None, "user_version", 999).unwrap();
    drop(c);
    let db_before = fs::read(root.join("workspace.db")).unwrap();
    ok(&cli(&["workspace", "info", p(&root)]), 1);
    assert_eq!(fs::read(root.join("workspace.db")).unwrap(), db_before);
}

#[test]
fn building_generations_are_ignored_and_corrupt_payloads_are_not_reused() {
    let (d, root, _, _) = fixture();
    let cold = decode(&root, &d.path().join("a.json"), &[]);
    ok(&cold, 0);
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    c.execute(
        "UPDATE cache_generations SET state='building',quality='unknown'",
        [],
    )
    .unwrap();
    drop(c);
    let fresh = decode(&root, &d.path().join("b.json"), &[]);
    ok(&fresh, 0);
    assert_eq!(rows(&cold.stdout), rows(&fresh.stdout));
    assert_eq!(
        json(&fs::read(d.path().join("b.json")).unwrap())["cache"]["hits"],
        0
    );
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    c.execute("UPDATE signal_entries SET typed_json=json_replace(typed_json,'$.status','spoofed') WHERE generation_id=(SELECT max(id) FROM cache_generations)",[]).unwrap();
    drop(c);
    let out = d.path().join("result.jsonl");
    ok(
        &decode(&root, &d.path().join("bad.json"), &["-o", p(&out)]),
        1,
    );
    assert!(!out.exists());
    ok(&cli(&["workspace", "cache", p(&root), "clear"]), 0);
    assert_eq!(info(&root)["rows"], 0);
    assert_eq!(info(&root)["building_generations"], 0);
    ok(&decode(&root, &d.path().join("recovered.json"), &[]), 0);
}

#[test]
fn cache_quota_caps_stored_payload_without_changing_stream_results() {
    let (d, root, source, _) = fixture();
    ok(&cli(&["workspace", "cache", p(&root), "limit", "1"]), 0);
    let mut text = "base hex timestamps absolute\n".to_string();
    for _ in 0..5000 {
        text.push_str("0.1 1 100 Rx d 8 01 00 00 00 00 00 00 00\n");
    }
    fs::write(&source, text).unwrap();
    let cold = decode(&root, &d.path().join("cold.json"), &[]);
    ok(&cold, 0);
    assert_eq!(rows(&cold.stdout).len(), 5000);
    let report = json(&fs::read(d.path().join("cold.json")).unwrap());
    assert!(report["cache"]["quota_skips"].as_u64().unwrap() > 0);
    assert!(info(&root)["payload_bytes"].as_u64().unwrap() <= 1024 * 1024);
    let warm = decode(&root, &d.path().join("warm.json"), &[]);
    ok(&warm, 0);
    assert_eq!(rows(&cold.stdout), rows(&warm.stdout));
    assert!(
        json(&fs::read(d.path().join("warm.json")).unwrap())["cache"]["rows_evicted"]
            .as_u64()
            .unwrap()
            > 0
    );
    let manifest = fs::read(root.join("workspace.json")).unwrap();
    ok(&cli(&["workspace", "cache", p(&root), "clear"]), 0);
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), manifest);
}

#[test]
fn relative_manifest_paths_are_resolved_from_workspace_and_not_process_directory() {
    let (d, root, source, dbc) = fixture();
    let local = root.join("local.asc");
    let local_db = root.join("local.dbc");
    fs::copy(source, &local).unwrap();
    fs::copy(dbc, &local_db).unwrap();
    let mut manifest = json(&fs::read(root.join("workspace.json")).unwrap());
    manifest["logs"][0]["path"] = "local.asc".into();
    for b in manifest["logs"][0]["bindings"].as_array_mut().unwrap() {
        b["path"] = "local.dbc".into();
    }
    fs::write(
        root.join("workspace.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let out = decode(&root, &d.path().join("relative.json"), &[]);
    ok(&out, 0);
    assert_eq!(rows(&out.stdout).len(), 3);
}

#[test]
fn interrupted_builders_are_removed_and_limited_issue_reports_stay_partial() {
    let (_d, root, _, _) = fixture();
    let payload = canlog::dbc::CachedSignals {
        status: "decoded".into(),
        database_sha256: None,
        message: Some("M".into()),
        error: None,
        signals: vec![],
    };
    let open = || {
        canlog::cache::Session::open(&root.join("workspace.db"), "test-key".into(), 1024 * 1024)
            .unwrap()
    };
    let mut session = open();
    for ordinal in 0..128 {
        session.store(ordinal, &payload).unwrap();
    }
    assert_eq!(info(&root)["building_generations"], 1);
    assert_eq!(info(&root)["rows"], 0);
    let mut observer = open();
    assert!(observer.lookup(0).unwrap().is_none());
    // Cancellation/error unwinding follows this same Drop path after a batch.
    drop(session);
    assert_eq!(info(&root)["building_generations"], 0);
    assert!(observer.lookup(0).unwrap().is_none());
    observer.store(0, &payload).unwrap();
    observer
        .complete(&canlog::analysis::AnalysisReport {
            issues_selected: 1,
            selection_complete: false,
            ..Default::default()
        })
        .unwrap();
    let c = rusqlite::Connection::open(root.join("workspace.db")).unwrap();
    let (quality, report): (String, String) = c
        .query_row(
            "SELECT quality, report_json FROM cache_generations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(quality, "unknown");
    assert_eq!(json(report.as_bytes())["status"], "partial");
}

#[test]
fn reducing_quota_evicts_completed_generations_immediately() {
    let (d, root, source, _) = fixture();
    fs::write(
        &source,
        format!(
            "base hex timestamps absolute\n{}",
            "0.1 1 100 Rx d 8 01 00 00 00 00 00 00 00\n".repeat(5000)
        ),
    )
    .unwrap();
    ok(&decode(&root, &d.path().join("full.json"), &[]), 0);
    assert!(info(&root)["payload_bytes"].as_u64().unwrap() > 1024 * 1024);
    ok(&cli(&["workspace", "cache", p(&root), "limit", "1"]), 0);
    assert!(info(&root)["payload_bytes"].as_u64().unwrap() <= 1024 * 1024);
    assert_eq!(info(&root)["building_generations"], 0);
}

#[test]
fn cached_fractional_physical_values_preserve_exact_f64_bits() {
    let (d, root, source, dbc) = fixture();
    fs::write(&dbc, "VERSION \"\"\nNS_ :\nBS_:\nBU_: Sender Receiver\nBO_ 256 Msg: 8 Sender\n SG_ X : 0|16@1+ (0.013,0) [0|1000] \"kW\" Receiver\n").unwrap();
    fs::write(
        &source,
        "base hex timestamps absolute\n0.1 1 100 Rx d 8 1E 4B 00 00 00 00 00 00\n",
    )
    .unwrap();
    let cold = decode(&root, &d.path().join("cold.json"), &[]);
    let warm = decode(&root, &d.path().join("warm.json"), &[]);
    ok(&cold, 0);
    ok(&warm, 0);
    assert_eq!(cold.stdout, warm.stdout);
    let actual = rows(&warm.stdout)[0]["signals"][0]["physical"]
        .as_f64()
        .unwrap();
    assert_eq!(actual.to_bits(), (19230.0_f64 * 0.013).to_bits());
    assert_eq!(
        json(&fs::read(d.path().join("warm.json")).unwrap())["cache"]["hits"],
        1
    );
}

#[test]
fn cancellation_before_cache_setup_preserves_existing_output_and_cache() {
    use clap::Parser;
    #[derive(Parser)]
    struct Args {
        #[command(flatten)]
        input: canlog::app::InputArgs,
    }
    let (d, root, _, _) = fixture();
    ok(&decode(&root, &d.path().join("initial.json"), &[]), 0);
    let before = info(&root);
    let output = d.path().join("out.jsonl");
    fs::write(&output, "KEEP").unwrap();
    let cancel = canlog::playback::Cancellation::default();
    cancel.cancel();
    let (report, result) = canlog::workspace::stream(
        &root,
        &Args::parse_from(["test", "first"]).input,
        Some(&[]),
        &canlog::analysis::StreamOutput {
            output: Some(output.clone()),
            format: None,
            overwrite: true,
        },
        false,
        &cancel,
    );
    assert!(result.unwrap_err().is::<canlog::playback::Cancelled>());
    assert_eq!(report.status, "cancelled");
    assert!(!report.published);
    assert_eq!(fs::read(&output).unwrap(), b"KEEP");
    assert_eq!(info(&root), before);
}

#[test]
fn moved_log_relinks_identical_bytes_and_rebuilds_path_bound_artifacts() {
    let (d, root, source, _) = fixture();
    ok(
        &cli(&["workspace", "index", p(&root), "first", "--stride", "1"]),
        0,
    );
    let cold = decode(&root, &d.path().join("before.json"), &[]);
    ok(&cold, 0);
    let before = json(&fs::read(root.join("workspace.json")).unwrap());
    let moved = d.path().join("moved.asc");
    fs::rename(&source, &moved).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&moved)]),
        0,
    );
    let after = json(&fs::read(root.join("workspace.json")).unwrap());
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(after["logs"][0]["bindings"], before["logs"][0]["bindings"]);
    assert_eq!(
        after["logs"][0]["source_identity"],
        before["logs"][0]["source_identity"]
    );
    let next = decode(&root, &d.path().join("moved.json"), &[]);
    ok(&next, 0);
    let report = json(&fs::read(d.path().join("moved.json")).unwrap());
    assert_eq!(report["cache"]["hits"], 0);
    assert_eq!(report["cache"]["misses"], 3);
    assert!(report["index"].is_null());
    let canonical = moved.canonicalize().unwrap();
    let mut old_rows = rows(&cold.stdout);
    for row in &mut old_rows {
        row["record"]["location"]["source"] = p(&canonical).into();
    }
    assert_eq!(old_rows, rows(&next.stdout));
    ok(
        &cli(&["workspace", "index", p(&root), "first", "--stride", "1"]),
        0,
    );
    let warm = decode(&root, &d.path().join("warm.json"), &[]);
    ok(&warm, 0);
    assert_eq!(next.stdout, warm.stdout);
    let report = json(&fs::read(d.path().join("warm.json")).unwrap());
    assert_eq!(report["cache"]["hits"], 3);
    assert!(!report["index"].is_null());
    let manifest = fs::read(root.join("workspace.json")).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&moved)]),
        0,
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), manifest);
}

#[test]
fn relink_rejects_changed_content_conflicting_hash_and_protected_aliases() {
    let (d, root, source, dbc) = fixture();
    let before = fs::read(root.join("workspace.json")).unwrap();
    let altered = d.path().join("changed.asc");
    fs::write(
        &altered,
        fs::read_to_string(&source).unwrap().replace("0.2", "0.4"),
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&altered).unwrap().len(),
        fs::metadata(&source).unwrap().len()
    );
    for target in [
        &altered,
        &dbc,
        &root.join("workspace.json"),
        &root.join("workspace.db"),
    ] {
        ok(
            &cli(&["workspace", "relink", p(&root), "first", p(target)]),
            1,
        );
        assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
    }
    let copied = d.path().join("copied.asc");
    fs::copy(&source, &copied).unwrap();
    ok(
        &cli(&[
            "workspace",
            "relink",
            p(&root),
            "first",
            p(&copied),
            "--expected-sha256",
            &"0".repeat(64),
        ]),
        1,
    );
    ok(
        &cli(&[
            "workspace",
            "relink",
            p(&root),
            "first",
            p(&copied),
            "--expected-sha256",
            "invalid",
        ]),
        1,
    );
    ok(
        &cli(&["workspace", "add", p(&root), p(&copied), "--name", "other"]),
        0,
    );
    let before = fs::read(root.join("workspace.json")).unwrap();
    let alias = d.path().join("alias.asc");
    fs::hard_link(&copied, &alias).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&alias)]),
        1,
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
    let index = root.join("indexes").join("fake.asc");
    fs::copy(&source, &index).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&index)]),
        1,
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
}

#[test]
fn legacy_manifest_requires_explicit_hash_when_original_is_missing() {
    let (d, root, source, _) = fixture();
    let hash = canlog::index::hash_file(&source, &Default::default()).unwrap();
    let mut manifest = json(&fs::read(root.join("workspace.json")).unwrap());
    manifest["schema_version"] = 1.into();
    manifest["logs"][0]
        .as_object_mut()
        .unwrap()
        .remove("source_identity");
    fs::write(
        root.join("workspace.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let moved = root.join("moved.asc");
    fs::rename(source, &moved).unwrap();
    let before = fs::read(root.join("workspace.json")).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&moved)]),
        1,
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
    ok(
        &cli(&[
            "workspace",
            "relink",
            p(&root),
            "first",
            p(&moved),
            "--expected-sha256",
            &hash.to_uppercase(),
        ]),
        0,
    );
    let after = json(&fs::read(root.join("workspace.json")).unwrap());
    assert_eq!(after["schema_version"], 2);
    assert_eq!(after["logs"][0]["path"], "moved.asc");
    assert_eq!(after["logs"][0]["source_identity"]["sha256"], hash);
    ok(&decode(&root, &d.path().join("legacy.json"), &[]), 0);
}

#[test]
fn legacy_relink_can_verify_existing_original_and_records_current_revision() {
    let (d, root, source, _) = fixture();
    let mut manifest = json(&fs::read(root.join("workspace.json")).unwrap());
    manifest["schema_version"] = 1.into();
    manifest["logs"][0]
        .as_object_mut()
        .unwrap()
        .remove("source_identity");
    fs::write(
        root.join("workspace.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    // A current, reachable source is authoritative after an intentional edit.
    fs::write(
        &source,
        fs::read_to_string(&source).unwrap().replace("0.2", "0.4"),
    )
    .unwrap();
    let copy = d.path().join("copy.asc");
    fs::copy(&source, &copy).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&copy)]),
        0,
    );
    let after = json(&fs::read(root.join("workspace.json")).unwrap());
    assert_eq!(after["schema_version"], 2);
    assert_eq!(
        after["logs"][0]["source_identity"]["sha256"],
        canlog::index::hash_file(&copy, &Default::default()).unwrap()
    );
    assert!(source.exists());
}

#[test]
fn missing_unrelated_source_directory_does_not_block_output_protection() {
    let (d, root, _, _) = fixture();
    let folder = d.path().join("old-folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("other.asc");
    fs::write(&file, "base hex timestamps absolute\n0.1 1 100 Rx d 0\n").unwrap();
    ok(
        &cli(&["workspace", "add", p(&root), p(&file), "--name", "other"]),
        0,
    );
    fs::remove_file(&file).unwrap();
    fs::remove_dir(&folder).unwrap();
    let output = d.path().join("out.jsonl");
    let good = decode(&root, &d.path().join("ok.json"), &["-o", p(&output)]);
    ok(&good, 0);
    assert_eq!(rows(&fs::read(output).unwrap()).len(), 3);
    // Missing parent paths still normalize identically, including dot segments.
    assert!(canlog::output::ensure_distinct_paths(
        &file,
        &folder.join("..").join("old-folder").join("other.asc")
    )
    .is_err());
}

#[test]
fn cancelled_relink_and_unknown_identity_schema_leave_manifest_unchanged() {
    let (_d, root, source, _) = fixture();
    let before = fs::read(root.join("workspace.json")).unwrap();
    let cancel = canlog::playback::Cancellation::default();
    cancel.cancel();
    assert!(
        canlog::workspace::relink(&root, "first", &source, None, &cancel)
            .unwrap_err()
            .is::<canlog::playback::Cancelled>()
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
    let mut manifest = json(&before);
    manifest["schema_version"] = 999.into();
    fs::write(
        root.join("workspace.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let before = fs::read(root.join("workspace.json")).unwrap();
    ok(
        &cli(&["workspace", "relink", p(&root), "first", p(&source)]),
        1,
    );
    assert_eq!(fs::read(root.join("workspace.json")).unwrap(), before);
}
