# DBC 엔진 업데이트 및 호환성 검증

검사일: 2026-10-03, Windows MSVC.
Git dependency, Cargo.lock과 report/cache의 engine identity를
`da64ad9ccf10237fce0993d1b83f460416d3da53`에서 `c51f18848dc4e32afd96b1a6e5bc975da70c6828`로 갱신했다.

## 변경 내용과 연동

upstream의 [c51f188 커밋](https://github.com/najari/candb-csharp-clone/commit/c51f18848dc4e32afd96b1a6e5bc975da70c6828)은
HTTP API, CLI 명령과 [API 가이드](https://github.com/najari/candb-csharp-clone/blob/c51f18848dc4e32afd96b1a6e5bc975da70c6828/docs/API.md)를 추가했다.
두 revision 사이의 engine 변경 파일은 cli.rs/http.rs/lib.rs/main.rs와 tests/api.rs다.
Document parser, analysis 검사, codec/model 소스 및 engine 의존성은 변경되지 않았다.
canlog는 기존 `Document::from_bytes`, `analysis::check`, `CompiledMessage`와 `Scratch`의 직접 Rust 호출을 유지한다.
새 HTTP API는 별도로 엔진 서버를 실행할 때 사용하는 upstream 기능이며 canlog 실행에는 서버가 필요하지 않다.

## 실행 검사

`scripts/build.ps1 -Test -Release -TestEngine -EngineSamples C:\Users\admin\candb_csharp_clone\samples`에서
고정한 새 engine 테스트 **88개**, canlog 테스트 **90개**가 통과했다.
engine 결과에는 HTTP/CLI API 9개가 포함되며 fuzz/performance/benchmark 6개는 upstream의 ignored다.
필요한 DBC corpus는 pinned source 복사본에 로컬 샘플을 추가했으며 upstream checkout과 원본 샘플을 수정하지 않았다.
canlog Clippy `-D warnings`, rustfmt, release build와 Rust 1.88.0의 `cargo check --locked --all-targets`가 통과했다.
이 검사는 Rust engine/library/CLI 범위이며 CANdb WPF UI·installer 검증은 포함하지 않는다.

업데이트 전 실행 파일은 `target/canlog-before-c51f188.exe`에 보관했다.
최종 `dist/canlog.exe` SHA-256은 `f6b7a44ff4d566adbc8af1ff9bff9afec445b22c363f0a65e84ef33ecb33c90d`다.

## 기존 캐시 및 실제 샘플 결과

`scripts/verify_engine_update.py`는 이전 실행 파일로 workspace/index/cache를 만든 뒤 최신 실행 파일로 같은 workspace를 해석한다.
모든 typed frame·signal row와 issue/decode count, selection 완료 상태 및 complete/partial 상태가 같다.
engine revision이 다른 기존 신호 cache에는 적중하지 않으며 새 generation을 채운 후 다음 실행부터 적중한다.
원시 parser identity가 바뀌지 않았으므로 기존 sparse index는 검증 후 재사용한다.
decode 실행에서 manifest를 변경하지 않았다.

| 샘플 | frame | valid 신호 | 새 엔진 첫 misses | 다음 hits | issue |
|---|---:|---:|---:|---:|---:|
| Motorola ASC | 50 | 230 | 50 | 50 | 0 |
| Model3 ASC + 3 DBC | 5,986 | 57,154 | 5,986 | 5,986 | 47 |
| CANoe demo BLF + easy DBC 채널 1·2 | 1,132 | 2,264 | 1,132 | 1,132 | 403 |
| 합계 | 7,168 | 59,648 | 7,168 | 7,168 | 450 |

신규 실행 파일로 `scripts/verify_workspace.py`도 다시 실행했다.
direct/cold/warm/bypass와 범위 query 결과가 같고, valid 신호 값 59,648개가 `cantools 43.0.2`와 일치했다.
integer raw는 정확히 동일하며 physical의 독립 oracle 허용 오차는 relative `1e-10`, absolute `1e-9`다.
canlog 실행 모드 간 비교는 JSON row의 정확한 동등성을 요구한다.
easy의 ENVVAR_DATA_ 선언은 cantools 입력 문자열에서만 제외했으며 원본 DBC와 Rust 입력은 그대로 사용했다.
Motorola는 complete/코드 0, Model3/easy는 기존 unsupported issue를 유지해 partial/코드 3이다.
두 검증에서 3개 로그·5개 DBC 원본 8개의 SHA-256은 검사 전후 같다.

## 명령과 증거

```powershell
.\scripts\build.ps1 -Test -Release -TestEngine -EngineSamples C:\Users\admin\candb_csharp_clone\samples
.\target\verify-env\Scripts\python.exe .\scripts\verify_engine_update.py --before .\target\canlog-before-c51f188.exe --out .\artifacts\engine-update-rerun
.\target\verify-env\Scripts\python.exe .\scripts\verify_workspace.py --out .\artifacts\dbc-oracle-rerun
```

`--before`는 업데이트 전 엔진 revision의 실행 파일이어야 하며 `--out`은 새 폴더를 지정한다.
build/dependency/MSRV 기록은 `artifacts/engine_update_2026-10-03_01/`에,
old/new 캐시 및 값 비교는 `artifacts/engine_update_samples_2026-10-03_01/results.json`에,
독립 DBC 비교는 `artifacts/engine_update_oracle_2026-10-03_01/results.json`에 보관했다.
원본 샘플과 검사 산출물은 기존 Git 제외 규칙을 유지한다.
이전 검증 hash·집계는 각 validation 문서에 historical snapshot으로 보존한다.
