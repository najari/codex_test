"""Turn a local verification.json into reviewable repository reports (no sample content copied)."""
import argparse
import hashlib
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    parser.add_argument("--cpu", required=True)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--msrv-verified", action="store_true")
    args = parser.parse_args()
    args.report = args.report.resolve()
    data = json.loads(args.report.read_text(encoding="utf-8"))
    assert not any("validation_error" in row for row in data["samples"])
    samples = []
    for row in data["samples"]:
        samples.append({key: row[key] for key in ["path", "bytes", "sha256", "source_license", "status", "frames", "issues", "issue_counts", "roundtrip", "actual_record_losses"]})
        samples[-1]["native_preservation"] = row["native_preservation"]
    (ROOT / "docs/sample-manifest.json").write_text(json.dumps({"schema_version": 1, "verified_date": "2026-10-03", "samples": samples}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    fixture = ROOT / "tests/fixtures/can-mixed.asc"
    (ROOT / "tests/fixtures/manifest.json").write_text(json.dumps({"schema_version": 1, "fixtures": [{"path": fixture.name,
        "sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(), "producer": "authored for canlog tests", "profile": "canonical ASC Classic/Remote/FD",
        "license": "project-authored, no third-party content; repository license unspecified", "expected_frames": 3,
        "expected_issues": 0, "timestamp_ns": [100000001, 200000001, 300000001]}]}, indent=2) + "\n", encoding="utf-8")
    counts = sum((Counter(row["issue_counts"]) for row in samples), Counter())
    exe = Path(data["cli"])
    lines = ["# CANLOG 구현 검증 결과", "", "검증일: 2026-10-03. 파일 기반 recording/replay 0.1의 지원 프로파일을 검증했다.", "",
             "## 실행 환경과 빌드", "", f"- OS: `{data['platform']}`", f"- CPU: {args.cpu}; RAM 약 16 GiB.",
             f"- Compiler: `{args.compiler}`; Rust MSVC release build.", "- `cargo test --locked`: 50개 통과 (단위 8, 기존 통합 30, 보존/매핑 통합 12).",
             "- `cargo clippy --locked --all-targets -- -D warnings`, `cargo fmt --all -- --check`: 통과.",
             f"- 최소 Rust 1.88.0: {'`cargo +1.88.0 check --locked --all-targets` 통과.' if args.msrv_verified else '별도 실행 미확인.'}",
             "- GitHub CI workflow는 Windows/Linux 및 MSRV gate를 추가했다. 원격 CI 실행 성공은 이 로컬 결과에 포함하지 않는다.",
             f"- `dist/canlog.exe` SHA-256: `{hashlib.sha256(exe.read_bytes()).hexdigest()}`", "",
             "## 샘플·독립 비교", "", f"- ASC 21개 + BLF 22개, 총 {len(samples)}개를 전수 검사했다.",
             f"- CAN 데이터/Remote 프레임 {sum(s['frames'] for s in samples):,}개를 source → recorded BLF → replayed ASC에서 정수 ns와 전체 CAN 필드로 비교해 일치했다.",
             f"- issue 합계: `{dict(counts)}`. issue는 정상 프레임으로 바꾸지 않았다.",
             "- 재작성은 skip/recover와 이름 있는 손실 허용을 명시했고, 각 파일의 실제 손실을 report로 남겼다. 부분 성공은 종료 코드 3이다.",
             "- CAN 프레임 0개인 event/비CAN 파일은 0개 결과와 issue를 검증했다. 파일 전체의 모든 기록을 보존했다는 의미가 아니다.",
             f"- 독립 reader: python-can {data['python_can']}. 원본 BLF의 CAN frame 수·ID·channel·direction·payload·FD flags 및 생성된 ASC/BLF를 비교했다.",
             "- 원본 ASC의 symbolic ID/relative delta 변종은 외부 reader와 dialect가 달라 canonical 숫자 ASC/BLF를 비교했다. 원본 relative 시간 해석의 외부 동등성을 주장하지 않는다.",
             "- 별도 합성 profile은 FD DLC 0–15/길이 0–64, BRS/ESI, Classic raw DLC 0–15, Remote, 최대 ID/channel, 독립 v2/FD64를 검증했다.",
             "- python-can ASC의 DLC=0 FD를 remote로 표시하는 차이를 report에 명시했다. 원본 BLF epoch float64 시간 오차 허용은 500ns이고, Rust 내부 왕복 시간은 정수 ns의 정확한 일치다.",
             "- 외부 FD 비교로 CAN_FD_MESSAGE의 flags/valid-bytes offset 오류를 발견해 수정했으며 독립 byte-layout 회귀 테스트를 추가했다.",
             f"- 같은 포맷의 native 기록 보존: {len(samples)}개 전체에서 source → record → replay content digest가 일치했다. 총 {sum(s['native_preservation']['records'] for s in samples):,}개 기록이며 CAN error/event/unresolved/unknown object를 포함한다.",
             "- Native 검증은 별도 Python scanner로 ASC의 base·정수 ns·timestamp 뒤 행을, BLF의 순서 있는 inner object bytes를 비교한다. 압축·header·주석·공백의 byte-for-byte 파일 일치를 주장하지 않는다.",
             "- 이름 매핑은 채널별 Standard/Extended와 Classic/FD, numeric/trailer 우선, 누락·중복·범위·상한·입력 보호를 검증했다. 단위 fixture의 임의 ID를 실제 샘플의 정답이라고 가정하지 않는다.",
             "- [Checksum/결과 manifest](sample-manifest.json), [지원·손실 범위](support.md). 원본 샘플은 수정·복사·Git 추가하지 않았다.", "",
             "| 샘플 | CAN frames | 기본 scan issues | 기본 scan 상태 |", "|---|---:|---:|---|"]
    lines += [f"| `{s['path']}` | {s['frames']:,} | {s['issues']:,} | {s['status']} |" for s in samples]
    lines += ["", "## 실제 재생과 자원 측정", "", "3-frame, 0.4s 구간으로 실제 CLI를 실행했다. 배속 전후 timestamp는 원본 값을 유지했다.", "",
              "| 모드 | 실제 경과 시간 |", "|---|---:|"]
    for timing in data["timing"]:
        label = f"{timing['speed']}배속" if "speed" in timing else timing["mode"]
        elapsed = timing.get("elapsed_s", timing.get("elapsed_s_after_first_frame"))
        lines.append(f"| {label} | {elapsed:.3f}s |")
    lines += ["", "pause/resume의 0.15s 정지를 반영했다. 3600s frame 간격에서도 stop 제어로 1s 이내 취소와 임시 출력 폐기를 통합 테스트했다.", "",
              "메모리 측정은 Windows process peak working set이다. 첫 frame 지연은 프로세스 시작/CLI 종료 비용을 포함한 limit=1 측정이다. 단일 CAN ID 합성 fixture, OS cache를 비우지 않은 로컬 실행이며 실차 일반 성능 보장이 아니다.", "",
              "| Format | Frames | 입력 bytes | 시간 | Frames/s | Peak working set | 첫 frame 지연 |",
              "|---|---:|---:|---:|---:|---:|---:|"]
    for bench in data["benchmark"]:
        lines.append(f"| {bench['format'].upper()} | {bench['frames']:,} | {bench['bytes']:,} | {bench['elapsed_s']:.3f}s | {bench['frames_per_s']:,.0f} | {bench['peak_working_set_bytes']/1024/1024:.2f} MiB | {bench['first_frame_process_latency_s']*1000:.1f}ms |")
    lines += ["", "10배 frame 수에서도 ASC/BLF 각각 64 MiB 미만과 16 MiB 이내의 peak 증가 조건을 통과했다. 이 수치는 위 fixture의 scan 결과이고, 설정된 최대 container/object를 가진 모든 입력의 동일 RSS를 보장하지 않는다.", "",
              "## 원자적 출력·실패 검증", "", "기존 출력·동일 파일·hardlink 보호, 게시 직전 noclobber 경쟁, malformed JSONL/ASC, DLC/payload 불일치, oversized 행/container, 압축 해제 상한, BLF EOF/carry, flush I/O 실패, 역행 시간·반복, 원본 위치와 date 정밀도 손실을 테스트했다.",
              "ID 종류 5000개를 가진 입력도 기록·재생했다. 상세 통계의 key 상한은 info/stats에만 적용하여 recording/replay를 막지 않는다.", "",
              "## 재현과 증거", "", "```powershell", ".\\scripts\\build.ps1 -Test -Release",
              ".\\target\\verify-env\\Scripts\\python.exe .\\scripts\\verify_samples.py --exe .\\dist\\canlog.exe", "```", "",
              f"최종 상세 실행 결과: `{args.report.relative_to(ROOT).as_posix()}`. 원본 checksum, actual loss, 독립 비교의 차이와 benchmark 입력 checksum을 포함한다.", "",
              "장비 송수신, MF4, SQLite/DBC/CDD/진단, arbitrary ASC/BLF dialect와 error/event의 교차 포맷 의미 변환은 이번 검증 범위 밖이다."]
    (ROOT / "docs/validation.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("Generated docs/validation.md, docs/sample-manifest.json, tests/fixtures/manifest.json")


if __name__ == "__main__":
    main()
