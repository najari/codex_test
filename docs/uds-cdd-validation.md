# UDS/CDD 실제 검증 기록

2026-10-03 Windows에서 실행했다. 원본 CDD와 외부 엔진 checkout을 수정하지 않았으며 local corpus는 Git에 추가하지 않는다.

## 빌드와 검증

- CDD feature: `.\scripts\build.ps1 -Cdd -Test -Release` 성공. Rust 테스트 138개, Clippy `-D warnings`, release build 통과.
- 기본 feature: `.\scripts\build.ps1 -Test`도 Rust 테스트 138개·Clippy·debug build 통과. CDD 없는 기존 기능과 feature 오류 경로를 함께 확인했다.
- 기본 feature의 `cargo +1.88.0 check --locked --all-targets` 통과. CDD feature의 실제 빌드 toolchain은 Rust 1.98.1이었다.
- `dist/canlog.exe` SHA-256: `6b96a179e1b9d4c7c97f481db86f6b4100e01eac2ca0379dd6b2a7a48a23be64`.
- 독립 UDS/CDD harness: 30개 비교 항목, CLI 28회, `passed: true`. udsoncan 1.26.1 요청 + can-isotp 2.0.7 프레임 생성/수동 수신 + cantools 43.0.2 CDD 필드 비교를 사용했다.
- 기존 ISO-TP 독립 harness: 새 exe에서 208개 payload 결과, CLI 12회, `passed: true`. normal/extended/mixed × 11/29-bit × padding/길이 조합과 Comfort/Engine ASC 및 BLF를 포함한다.
- 저장소 예제 CLI 2회: `uds-demo.asc`의 positive/pending/positive/negative와 `uds-session.asc` + 제공 CDD의 positive/decoded를 검증했다.
- rustfmt check, Git diff whitespace check, Markdown 로컬 파일 link check 통과.

## 검증한 계약

서비스 profile 13개의 요청·응답, DID/subfunction/routine/BSC echo, NRC negative/pending, repeated request ambiguity, P2 내 FF 시작 후 P2 이후 CF 완료, deadline을 지난 orphan, EOF·gap·suppress·자원 상한·overflow·동일 timestamp 순서를 테스트한다. 잘못된 policy와 출력/report의 source·policy·CDD alias 보호도 검사한다.

기본 header 분석은 positive·negative 완료와 pending의 열린 lifecycle을 구분한다. CDD가 없는 data record는 opaque이다. 독립 fixture 로그는 실제 장비 capture가 아니라 외부 UDS/ISO-TP 구현으로 생성한 테스트 입력이다.

| 실제 CDD | 엔진 protocol / 결과 |
|---|---|
| cantools `example-diddatarefs.cdd` | Uds, ECU SBS_Test / Base_Variant. DefaultSession의 P2 raw=50, P2Ex raw=500/physical=5000을 cantools와 비교 |
| cantools `example.cdd` | Unknown, 검사 가능. UDS route 배정 거부 |
| cantools `le-example.cdd` | Unknown, 검사 가능. UDS route 배정 미지원 |
| cantools `invalid-bo-example.cdd` | Unknown, CDD-BIT-004 오류/exit 3. 독립 cantools도 byte order 4321 거부 |
| Vector `UDS-ExampleEcu-5.1.0.cdd` | Uds, Door / CommonDiagnostics. session timing·VIN을 독립 결과와 비교; NRC 78/31은 서비스 정의 목록 밖이라는 CDD-NRC-001 경고와 protocol default 이름을 유지 |

CDD 검사에서 모든 profile은 experimental이다. 위 NRC 경고가 있는 분석은 typed 필드를 읽었더라도 `decoded_with_diagnostics`/partial/exit 3이며 성공으로 숨기지 않는다. 여러 서비스의 subfunction identifier를 cantools가 같은 dictionary key로 합치는 특성이 있어 세션은 전체 definition 목록에서 이름과 identifier로 선택한다. 엔진 DID count와 cantools의 모든 identifier item count는 같은 의미가 아니다.

## 증거 파일

로컬 `artifacts/uds_cdd_release_2026-10-03_01/results.json`에는 실제 argv·exit code·stderr·binary SHA·원본 CDD SHA·비교 항목이 있다. 같은 폴더의 JSONL/report, `*.cdd-info.json`, `examples.json`으로 필드·diagnostics·예제 결과를 확인할 수 있다. `artifacts/isotp_uds_release_2026-10-03_01/results.json`은 transport 회귀 비교이다. 재현 명령은 [uds-cdd.md](uds-cdd.md)를 따른다.

이 검증은 사용한 파일·definition·서비스 profile에 대한 검증이다. 모든 CDD dialect/UDS 서비스, functional/FD, 실시간 장비 통신, 외부 표준 conformance 인증, diagnostic checkpoint/resume를 완료했다는 의미는 아니다.
