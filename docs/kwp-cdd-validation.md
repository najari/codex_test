# KWP·CDD 기존 샘플 검증

2026-10-03. 실행 증거는 로컬 `artifacts/kwp_cdd_release_2026-10-03_01/results.json`과 JSONL/report에 보관한다. `results.json`은 실행 명령, 바이너리·원본 SHA-256, 외부 도구 버전과 비교값을 포함한다. Vector 샘플을 Git에 복제하거나 수정하지 않는다.

## 독립 비교

`scripts/verify_kwp_cdd.py`는 Python CAN 4.6.1로 기존 ASC를 읽고 can-isotp 2.0.7의 수동 receiver와 모든 payload를 비교한다. cantools 43.0.2로 DBC 메시지 이름과 CDD 필드 배치를 비교한다. cantools는 BCD를 unsigned bits로 반환하므로 CDD가 `enc="bcd"`로 선언한 필드만 별도로 nibble-to-decimal 변환해 비교한다. 이는 cantools가 KWP 전체 transaction을 검증한다는 의미가 아니다.

| 기존 로그 | CAN frames / ISO-TP payload | KWP positive | CDD 결과 |
|---|---|---|---|
| ComfortDiagData | 20 / 18 | 9 | decoded 6, diagnostics 1, no_match 2 |
| EngineDiagData | 36 / 32 | 7 | decoded 4, diagnostics 1, decode_error 2; 응답 누락/EOF 18개 not_decoded |
| DiagDataA | 18 / 16 | 8 | decoded 6, diagnostics 1, no_match 1 |

ASC 3개에서 DBC 메시지 74개와 CDD 식별 field 값 16개를 비교한다. DBC transport 정의에는 신호가 없으므로 DBC signal 값 비교 수는 0이다. Engine serial number는 3,141,528이며 식별 번호는 BCD `9876`, `5432`, `1000`, `9999`, diagnostic identification `1`이다. Comfort 식별 번호는 `9877`, `5433`, `2000`, `8888`, diagnostic identification `2`다.

부분 성공을 명시적으로 검증한다. `3E 01` 요청의 응답 누락을 suppress 성공으로 바꾸지 않고, `10 81`에서 상위 bit를 삭제하지 않는다. 빈 DTC response proxy를 decoded로 보고하지 않는다. 원본·policy·CDD·DBC와 바이너리 SHA가 검증 전후 동일한지도 확인한다.

```powershell
.\target\verify-env\Scripts\python.exe .\scripts\verify_kwp_cdd.py --exe .\dist\canlog.exe --samples .\can_example\vector_samples\2026-10-03\groups\CANoe_13.0.172\CAN\CANSystemDemo --artifacts .\artifacts\kwp-cdd-new-run
```

Python 환경과 패키지는 검증용이며 canlog runtime 의존성이 아니다. artifacts 폴더는 새 경로를 지정한다.

## ASC·DBC·CDD 통합 CLI

[Comfort·Engine 실행 예제](../examples/asc-dbc-cdd/README.md)는 `--dbc`와 `--cdd`를 한 명령에 지정한다. 검증 스크립트는 기존 독립 분석 6회에 통합 분석 2회를 추가한다. 통합 DBC 프레임과 KWP/CDD transaction이 개별 분석과 같고, 진단의 `data_locations`가 DBC의 원본 frame 위치와 같으며, 모든 행의 `timeline_key`가 공통인지 검사한다.

갱신한 CDD release 바이너리의 증거는 로컬 `artifacts/asc_dbc_cdd_release_2026-10-03_01/results.json`에 있다. 편의 스크립트도 기본 `dist/canlog.exe`로 실행해 `artifacts/asc_dbc_cdd_script_release_2026-10-03_01/`에 결과를 저장했다. 기존 UDS/CDD 30건과 ISO-TP 208 payload의 release 회귀 결과는 각각 `artifacts/uds_combined_regression_2026-10-03_01/`, `artifacts/isotp_combined_regression_2026-10-03_01/`에 보관한다. 원본의 부분 품질과 DBC transport 신호 값 0개는 그대로 유지한다.

| 기존 로그 | 통합 JSONL 행 | DBC frames | KWP transactions |
|---|---:|---:|---:|
| ComfortDiagData | 48 | 20 | 9 |
| EngineDiagData | 95 | 36 | 25 |

기본 테스트 146개, CDD 포함 테스트 147개와 각 Clippy를 실행한다. 추가 CLI 테스트는 실제 DBC 신호 해석과 독립적인 부분 품질, UDS 경로, 입력 파일 보호와 필수 옵션, 직접 CDD 지정과 기존 policy assignment 충돌을 검사한다. 테스트의 작은 정의 fixture는 authored fixture이며 위 Vector 원본 검증과 구분한다.

## 회귀와 제한

### CDD 진단 replay

`scripts/verify_replay_cdd.py`는 위 독립 비교가 끝난 KWP 결과와 UDS 결과를 baseline으로 사용한다. 최종 배포용 바이너리의 증거는 `artifacts/replay_cdd_release_2026-10-03_02/results.json`에 있다. 새로운 KWP 독립 비교는 `artifacts/replay_cdd_kwp_release_base_2026-10-03_01/results.json`에 보관한다.

Comfort·Engine JSONL의 모든 행이 기존 분석과 동일하다(각각 48행·95행). Comfort 콘솔의 20개 DBC frame 출력 간격을 측정했으며 원본 구간 12.002536초를 2배속으로 재생하는 예상 6.001268초와 일치한다. 콘솔의 CDD 값 `9877`, `5433`, `2000`, `8888`, 미정의/경고 상태도 확인한다. UDS replay 22건은 udsoncan/can-isotp/cantools로 검증된 기존 UDS JSONL과 동일하다. UDS pending·negative 콘솔에서 CDD `NEG` 메시지 2개의 표기도 확인한다. 총 26회 CLI를 비교하며 원본과 바이너리 SHA를 검증 전후 확인한다.

배포용 `dist/canlog.exe`로 [편의 스크립트](../examples/asc-dbc-cdd/replay.ps1)의 기본 2배속 콘솔 및 대기 없는 JSONL 저장 모드를 실행했다. 결과는 `artifacts/replay_cdd_example_release_2026-10-03_02/`, `artifacts/replay_cdd_jsonl_release_2026-10-03_02/`에 보관한다. 원본의 부분 품질을 유지하며 콘솔은 데이터 파일 게시가 없으므로 `published=false`, JSONL은 `published=true`다.

이번 변경의 기본 테스트 149개, CDD 포함 150개와 Clippy·format·core Rust 1.88 검사가 통과했다. 추가 테스트는 live 출력과 배속 간격, 즉시 재생의 값/identity 동등성, 잘못된 옵션·정의 파일 보호, stop과 atomic publication, native CDD의 콘솔·JSONL을 검증한다. 현재 진단 replay는 단일 전체 로그 1회다.

canlog KWP 6개 테스트는 full mode, 다른 echo/orphan, TesterPresent suppression, pending/negative/ambiguity, malformed count/지원 범위와 CLI protocol 분리를 검사한다. 기본/`cdd` 전체 suite, Clippy, format, core Rust 1.88과 release의 기존 UDS/ISO-TP 독립 비교도 실행한다.

별도 CDD 엔진은 codec 98, 선택한 API 35, core message/resolve 16개 테스트와 Clippy를 통과했다. upstream의 모든 corpus suite가 통과했다는 의미는 아니다. 제공된 `vector_example` corpus는 upstream의 고정 기대값과 다르며 `corpus_fields` 2개는 ABS session field P2/P3 이름과 state count 99/176 차이로 실패했다. 기존 CDD checkout에서도 같은 두 차이가 재현됐다. 원본 sample 또는 기존 기대값을 바꿔 통과시키지 않았다.
