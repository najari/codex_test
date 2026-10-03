# SQLite 인덱스 및 DBC adapter 검증

검사일: 2026-10-03. 지원 범위와 명령은 [index-dbc.md](index-dbc.md)를 따른다.

## canlog 검사

`scripts/build.ps1 -Test -Release`에서 Rust 69개 테스트가 통과했다.
기존 55개와 새 index/decode 14개다. Clippy `-D warnings`, fmt 검사와 release build도 통과했다.
Rust 1.88.0 MSVC의 `cargo check --locked --all-targets`도 통과해 기존 최소 compiler 계약을 확인했다.
최종 release SHA-256: `b79642233b7bbc70ae0a1880803c721d468cc02915a9778d67b5884ccbd6660b`.
새 테스트는 상대 시간 ASC 상태 복원, BLF 객체 95개 분할 위치, 역행/동일 시각,
unknown-time issue, 원본의 동일 크기 변경, limits/ID map 변경, 미래 schema 거부,
실패/취소 시 기존 index 보호, output/report 충돌, u64/i64, Motorola,
Standard/Extended 동일 숫자, 기본/extended multiplex, FD 64-byte/short FD,
길이 오류/remote/미지정 DB, NaN raw bits와 invalid/ambiguous DBC를 포함한다.

## 실제 원본 및 인덱스 비교

`scripts/verify_index_decode.py`로 정리된 Vector 59개와 웹 17개, 총 76개 ASC/BLF를 검사했다.

| 항목 | 결과 |
|---|---:|
| 인덱스 생성 및 full-scan 동등성 | 70개 통과 |
| 유효 CAN 데이터가 있는 로그 | 34개 |
| CAN 프레임 합계 | 113,397개 |
| 전체/중간/마지막 또는 빈 검색 구간 | 174개 통과 |
| 잘못된 원본 거부 및 index 미게시 | 6개 |
| 검증 실행 명령 | 439개 |

프레임의 모든 field와 원본 위치를 비교했다. report의 선택 issue 수와 종료 상태도 전체 scan과 같았다.
CAN이 없는 36개 로그는 빈 결과와 event/issue quality의 비교이며 CAN decode 성공으로 집계하지 않았다.
시간 역행은 원본 순서를 유지했다. 불명확한 issue 시간은 모든 질의에 보수적으로 포함했다.
6개 거부 원본은 이전 검증과 같은 invalid BLF header/size/FD64 offset 문제다.
입력 SHA-256은 manifest와 검사 전후 모두 같았다.

## DBC 독립 oracle 비교

고정 Rust engine의 JSON decode row를 typed 결과로 바꾸고 `cantools 43.0.2`와 비교했다.
full/indexed decode 출력도 같았다.

| 실제 샘플 | frame | 비교한 signal 값 | 결과 |
|---|---:|---:|---|
| Motorola ASC + matrix DBC | 50 | 230 | 통과 |
| Model3 ASC + Battery/Drive/Thermal 3 DBC | 5,986 | 57,154 | 통과 |
| CANoe demo BLF + easy DBC, 채널 1·2 | 1,132 | 2,264 | 통과 |
| 합계 | 7,168 | 59,648 | 통과 |

활성 신호 집합, physical 값과 integer raw를 비교했다.
float physical 허용 오차는 relative `1e-10`, absolute `1e-9`이며 exact raw 정수는 동일해야 한다.
easy의 cantools parser는 unrelated `ENVVAR_DATA_` 선언을 지원하지 않아 해당 선언만 oracle의 in-memory 문자열에서 제외했다.
Rust engine에는 수정 없는 원본 DBC를 그대로 입력했다.
Model3의 unsupported event 47개, CANoe demo의 unsupported record 403개는 report에 유지하므로 frame decode와 별개로 종료 코드 3이다.

## 고정 engine 검사와 증거

engine source는 `da64ad9ccf10237fce0993d1b83f460416d3da53`이다.
Vector corpus가 upstream Git에 포함되지 않으므로 pinned source 복사본에 기존 로컬 engine의 112개 Vector DBC 및 cantools/model3 sample을 추가해 실행한다.
source code를 바꾸지 않고 원본 sample을 복사하며 검사 산출물은 `target/`와 `artifacts/`에만 저장한다.
`scripts/build.ps1 -TestEngine -EngineSamples C:\Users\admin\candb_csharp_clone\samples`에서
동일 pinned engine의 83개 테스트가 통과했다. fuzz/performance/benchmark 6개는 upstream의 의도적인 ignored다.

전수 결과와 각 명령 argv/시간/종료 코드/stdout/stderr는 로컬
`artifacts/index_decode_corpus_2026-10-03_01/results.json` 및 `README.md`에 보관했다.
최종 output/report 경로 보호 보완 후 동일 전수 검사를 다시 실행한 결과는 `artifacts/index_decode_corpus_2026-10-03_02/`다.
최종 실행 파일로도 70개/174구간 및 독립 DBC 비교가 모두 통과했다.
Vector 복사본 286개와 웹 corpus/evidence 55개, 총 341개 파일의 SHA-256을 다시 확인했으며 모두 원래 manifest와 같았다.
build와 engine 실행 기록은 `artifacts/index_decode_2026-10-03/`다.
샘플 원본·출력과 결과 데이터는 Git에서 제외되어 있다.

## 미완료 범위

아래는 이 검증 snapshot 당시의 범위다. 이후 workspace manifest, 영속 DBC binding과 typed cache 구현 및 검증은 [workspace-validation.md](workspace-validation.md)를 따른다.

이 구현은 single-file sparse index 및 DBC CLI다. 전체 phase 4/5 완료로 표시하지 않는다.
multi-file workspace manifest/revision history, 영속 DBC binding/typed cache,
source 이동 재연결, resume/crash checkpoint 복구, signal CSV/Parquet export는 후속 작업이다.
ISO-TP/UDS/CDD, MF4, multi-clock merge, GUI와 장비 transport는 아직 구현하지 않았다.
