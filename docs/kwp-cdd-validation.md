# Native KWP·CDD 검증

2026-10-04. 내부 KWP 헤더/echo profile을 제거하고 `cdd-rust-engine` master commit `ecd4a6a42792636a8386439653950e8693818d02`의 서비스 식별·요청 컨텍스트·응답 decoder를 사용한다. CDD가 없는 경로는 raw ISO-TP payload를 출력한다.

## 실제 Vector 원본 비교

`scripts/verify_kwp_cdd.py`는 python-can 4.6.1로 ASC를 읽고 can-isotp 2.0.7과 모든 payload를 비교한다. cantools 43.0.2와 DBC 메시지 이름 및 CDD 필드 배치를 비교한다. cantools의 BCD unsigned bits는 CDD가 `enc="bcd"`로 선언한 필드만 nibble-to-decimal 변환해 비교한다. KWP transaction의 전체 규격 적합성을 외부 도구가 인증한다는 의미는 아니다.

| 로그 | CAN frames / ISO-TP payload | Native KWP positive | CDD 결과 |
|---|---|---|---|
| ComfortDiagData | 20 / 18 | 7 | decoded 6, diagnostics 1, no_match 4 |
| EngineDiagData | 36 / 32 | 5 | decoded 4, diagnostics 1, unverified 2, identified request 18 |
| DiagDataA | 18 / 16 | 7 | decoded 6, diagnostics 1, no_match 2 |

Comfort의 CDD 미정의 session/reset은 unsupported 요청 2건과 orphan 응답 2건, DiagDataA는 각각 1건이다. Engine의 미지원 DTC proxy 응답 2건은 ambiguous/unverified로 남긴다. 응답 없는 TesterPresent 18건 중 17건은 로그 시간으로 관측한 deadline 경과, 1건은 EOF incomplete다. native 식별값 16개와 DBC 메시지 74개를 비교하며, transport DBC의 signal 값은 0개다. 원본 파일과 정의·설정의 SHA-256을 전후 확인한다.

내부 profile의 과거 positive 집계 9/7/8과 달리 CDD 정의에 맞는 응답만 positive로 기록한다. 미정의 또는 미지원 메시지도 raw payload와 원본 위치는 유지한다. 종료 코드 3은 이 원본의 부분 품질을 나타낸다.

## 통합 분석과 replay

Comfort·Engine 통합 CLI는 ASC를 한 번 읽어 DBC frame, ISO-TP payload와 native KWP/CDD 결과를 기록한다. 각각 JSONL 50/95행, DBC frame 20/36개를 생성한다. DBC frame 위치와 진단 `data_locations`, 공통 `timeline_key` 및 독립 집계를 확인한다.

`scripts/verify_uds_cdd.py`는 udsoncan 1.26.1, can-isotp, cantools로 기존 UDS/CDD 30개 사례를 비교한다. `scripts/verify_replay_cdd.py`는 이 baseline과 KWP baseline을 사용해 총 26회 CLI 분석/replay 동등성, native negative 콘솔 표기, Comfort CDD 값과 실제 2배속 콘솔 출력 간격(예상 6.001268초)을 검증한다. CDD 없는 replay도 raw payload·DBC frame을 유지한다. `scripts/verify_kwp_raw.py`는 3개 로그의 CDD 없는 분석/replay 6회를 같은 독립 transport baseline과 비교한다. CDD를 제외한 기본 실행 파일에서도 6회가 통과했다.

디버그 단계 증거는 `artifacts/native_kwp_debug_2026-10-04_01/`, `artifacts/native_uds_debug_2026-10-04_01/`, `artifacts/native_replay_debug_2026-10-04_01/`에 있다. 배포 실행 파일의 같은 검증은 `artifacts/native_kwp_release_2026-10-04_01/`, `artifacts/native_uds_release_2026-10-04_01/`, `artifacts/native_raw_release_2026-10-04_01/`, `artifacts/native_replay_release_2026-10-04_01/`에 저장한다. 각 `results.json`에 명령·버전·원본/실행 파일 SHA와 비교 결과를 남긴다.

## Rust 검사

작은 authored CDD fixture는 `tests/fixtures/kwp-native.cdd`이며 Vector 원본을 복제하지 않는다. KWP 테스트는 CDD 없는 raw 경로, 제거된 내부 목록 밖의 SID, CDD 미정의 요청, 잘못된 echo/길이, native pending/negative, 중복 요청과 negative의 ambiguity, 미지원 response layout과 bounded timeout, CLI protocol 분리를 검사한다. 통합 CLI 테스트는 raw 분석/replay 동등성, live 배속·stop·파일 보호와 CDD 필드 출력도 검사한다. 기본 테스트 145개(Rust 1.88), CDD 포함 테스트 151개와 두 구성의 Clippy·format 검사가 통과했다. Clippy는 현재 도구체인에서 실행했으며 Rust 1.88 환경에는 Clippy component가 설치되어 있지 않다.

## 재현 명령

```powershell
.\scripts\build.ps1 -Cdd -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_kwp_cdd.py --exe .\dist\canlog.exe --samples .\can_example\vector_samples\2026-10-03\groups\CANoe_13.0.172\CAN\CANSystemDemo --artifacts .\artifacts\native-kwp-new-run
```

Python 패키지는 검증용이며 canlog 실행 시 필요하지 않다. artifacts 경로는 새 폴더를 지정한다. KWP scope, 엔진 실험적 profile과 raw 출력 예제는 [KWP 문서](kwp-cdd.md)에 있다.
