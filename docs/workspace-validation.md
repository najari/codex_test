# Workspace 및 영속 신호 캐시 검증

검사일: 2026-10-03, Windows MSVC. 명령과 지원 범위는 [workspace.md](workspace.md)를 따른다.
아래는 최초 workspace/cache 84-test 빌드의 snapshot이다. 이후 source 재연결·manifest schema 2와 최신 release 검증은 [relink-validation.md](relink-validation.md)에 따로 기록한다.

## 빌드와 회귀 검사

`scripts/build.ps1 -Test -Release`에서 canlog 84개 테스트가 통과했다.
기존 69개와 새 workspace/cache 15개다. Clippy `-D warnings`, rustfmt 검사와 release build가 통과했다.
최종 소스로 Rust 1.88.0의 `cargo check --locked --all-targets`도 통과했다.
`dist/canlog.exe` SHA-256은 `2ca529914ad6d2d84279743bf38bf59dbe3370225684c210c1d10eb13b4c3d8b`다.

새 검사는 다음 동작을 확인한다.

- cold/warm/bypass 결과 동등성, raw 최대 u64와 raw_text 보존
- 같은 크기의 원본 내용 변경, DBC 내용 및 처리 정책 변경 시 이전 cache 미사용
- 로그별 DBC 격리, 이번 실행의 채널 override와 manifest 불변
- sparse index 선택 결과·source revision catalog, filtered index 재사용 거부
- cached row 적중 때도 issue 재검사, strict 실패, 빈 selection의 degraded 유지
- 범위별 lazy coverage와 limit의 unknown quality
- original/config/DBC/다른 로그/hardlink/SQLite 출력 보호, 미래 schema 보존 거부
- building generation 미사용, batch 이후 error unwinding 정리, checksum 변조 검출
- quota 초과 시 저장만 건너뛰고 출력 유지, LRU eviction과 quota 축소 즉시 정리
- workspace 기준 상대 경로 해석
- 취소된 setup에서 cancelled 상태와 기존 출력·완료 cache 유지
- fractional physical 값의 cache round-trip에서 정확한 f64 bit 유지

Model3 원본의 `BMS_maxDischargePower`, raw 19,230 × factor 0.013에서 발견한 JSON f64 복원 차이를 수정했다.
`serde_json`의 `float_roundtrip`을 사용하고 cache adapter version을 v2로 구분한다.
해당 값이 `249.98999999999998`에서 `249.99`로 달라지던 사례를 회귀 테스트로 고정했다.

## 실제 파일 및 독립 해석 비교

`scripts/verify_workspace.py`는 한 workspace에 다운로드한 3개 로그와 5개 DBC 파일을 등록한다.
채널 연결을 저장하고 로그마다 index를 만든 뒤 direct/cold/warm/bypass와 중간부터 마지막까지의 범위를 검사했다.
직접 실행에는 등록된 canonical 경로 표기를 사용해 frame의 모든 field, provenance 문자열과 signal row를 정확히 비교했다.

| 샘플 | frame | 비교 신호 | cold misses | warm hits | 범위 hits | 선택 issue |
|---|---:|---:|---:|---:|---:|---:|
| Motorola ASC + matrix DBC | 50 | 230 | 50 | 50 | 25 | 0 |
| Model3 ASC + Battery/Drive/Thermal DBC | 5,986 | 57,154 | 5,986 | 5,986 | 2,993 | 47 |
| CANoe demo BLF + easy DBC, 채널 1·2 | 1,132 | 2,264 | 1,132 | 1,132 | 566 | 403 |
| 합계 | 7,168 | 59,648 | 7,168 | 7,168 | 3,584 | 450 |

모든 warm/cache 범위 검색에서 misses는 0이었다.
각 모드의 선택 issue count, decode count, selection 완료 여부와 complete/partial 상태가 같았다.
Motorola는 complete/종료 코드 0, Model3와 CANoe demo는 기존 unsupported issue 때문에 partial/종료 코드 3이다.
frame의 DBC 해석 성공이 source의 전체 의미 보존을 뜻하지는 않는다.

warm 결과의 활성 신호 집합, integer raw와 physical 값을 `cantools 43.0.2`와 독립 비교했다.
integer raw는 정확히 동일하며 physical 허용 오차는 relative `1e-10`, absolute `1e-9`다.
direct와 cache 간 비교에는 이 오차를 사용하지 않고 JSON 값 전체가 같아야 한다.
easy의 `ENVVAR_DATA_` 선언만 cantools 입력의 in-memory 문자열에서 제외했으며 Rust에는 수정하지 않은 DBC를 입력했다.
3개 로그 및 5개 DBC의 검사 전후 SHA-256은 모두 같다.

cache clear 전 완료 rows는 7,168개, building generation은 0개였다.
clear 후 rows와 building은 모두 0개이며 manifest checksum은 decode 및 clear 전후 같다.
원본 로그·DBC 연결과 sparse indexes는 그대로 유지했다.

이 작은 샘플에서 warm 전체 실행 시간은 Motorola 약 0.045s, Model3 0.625s, easy 0.199s였다.
직접 해석은 각각 약 0.034s, 0.485s, 0.113s였으므로 속도 향상을 보장한다고 주장하지 않는다.
현재 CLI는 full source hashing, DBC 검증과 원본 parsing을 매번 수행하며 SQLite lookup 비용도 있다.
이번 완료 기준은 영속화·정합성·lazy 재사용과 실제 결과 동등성이다. 큰 corpus의 성능 최적화는 후속 범위다.

## 증거와 재현

최종 30개 실행 명령의 argv, 종료 코드, 경과 시간, stdout/stderr, report 및 checksum은
`artifacts/workspace_samples_2026-10-03_03/results.json`과 같은 폴더에 보관했다.
이 폴더의 `workspace/`는 로그 3개와 연결·인덱스를 유지해 후속 CLI 시험에 사용할 수 있다.
build/MSRV/help 기록은 `artifacts/workspace_2026-10-03_01/`에 있다.
앞선 `_01`은 Windows 경로 표기 비교 준비, `_02`는 f64 문제 진단 당시의 미완료 검증이며 최종 결과로 사용하지 않는다.
원본 데이터와 산출물은 Git에서 제외한다.

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_workspace.py --out .\artifacts\workspace-rerun
.\dist\canlog.exe workspace decode .\artifacts\workspace_samples_2026-10-03_03\workspace motorola
```

Python 3.11 이상과 `cantools==43.0.2`는 검증 환경에만 필요하다.
기존 single-file 전수 index/DBC 검증 snapshot은 [index-dbc-validation.md](index-dbc-validation.md)에 따로 유지한다.
이번 변경에서 같은 pinned Rust DBC engine source를 수정하지 않았다.

## 남은 범위

source 이동 재연결 CLI, manifest snapshot 복구와 중단 scan resume, signal CSV/Parquet,
index 용량 관리, multi-file clock/merge, ISO-TP/UDS/CDD, MF4, GUI와 장비 transport는 미구현이다.
강제 종료 이후 building은 재사용하지 않으며 자동 resume 대신 cache clear로 정리한다.
SQLite 파일 전체 크기는 payload quota와 다르며 clear는 VACUUM을 수행하지 않는다.
