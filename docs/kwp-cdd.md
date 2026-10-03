# 기존 Vector ASC·DBC·CDD로 KWP2000 분석

`kwp`는 명시한 Classic CAN 물리 경로에서 ISO-TP 요청·응답을 수동 분석한다. `protocol: "kwp2000_vector"` policy를 필수 지정한다. UDS policy와 혼용하거나 K-line, CAN FD, functional addressing을 자동 적용하지 않는다. 실제 ECU로 송신하지 않는다.

## 기존 샘플 실행

Vector CANSystemDemo 설정에서 확인한 연결은 다음과 같다. 샘플 파일은 Git에 복제하지 않았으며, 앞서 정리한 로컬 `can_example/vector_samples` 폴더가 필요하다.

| ASC | DBC | CDD | 물리 경로 |
|---|---|---|---|
| `ComfortDiagData.asc`, `DiagDataA.asc` | `Comfort.dbc` | `CANSystemDoor.cdd` | channel 1, request `700`, response `600` |
| `EngineDiagData.asc` | `PowerTrain.dbc` | `CANSystem.cdd` | channel 2, request `200`, response `400` |

두 CDD의 ECU는 `Any_ECU_example`, variant는 `COMMON_DIAGNOSTICS`다. 경로와 CDD assignment는 각각 `examples/isotp-physical.routes.json`, `examples/kwp-cansystem.policy.json`에 있다. 상대 CDD path는 policy 파일의 폴더를 기준으로 해석한다. 설치 경로가 다르면 policy의 `cdd.path`를 변경한다.

기존 샘플을 한 CLI에 ASC·DBC·CDD로 지정하는 전체 명령과 실행 스크립트는 [통합 예제](../examples/asc-dbc-cdd/README.md)에 있다.

시간에 맞춰 재생하려면 [ASC·DBC·CDD replay 예제](../examples/asc-dbc-cdd/replay.md)를 사용한다. `replay`에 `--protocol kwp2000-vector`, routes/policy와 DBC·CDD를 지정하며 콘솔 진단 필드 또는 JSONL을 출력한다. 배속은 벽시계 대기에만 적용하고 원본 시간·진단 품질은 유지한다. 진단 replay는 현재 전체 로그 1회다.

```powershell
.\scripts\build.ps1 -Cdd -Release
.\examples\asc-dbc-cdd\run.ps1 -Sample comfort
.\examples\asc-dbc-cdd\run.ps1 -Sample engine
```

`kwp`와 `uds`에서 `--dbc CHANNEL=PATH`를 반복 지정할 수 있다. 같은 source를 한 번 읽으며 모든 CAN frame의 DBC 결과와 진단 결과를 함께 출력한다. 이 DBC의 `Diag_Request`/`Diag_Response`에는 신호가 없으므로 DBC 메시지 이름 확인과 CDD 진단 필드 해석을 구분한다.

`--cdd PATH --ecu QUALIFIER --variant QUALIFIER --allow-experimental`은 CDD assignment가 없는 단일 route policy에서 사용할 수 있다. 여러 route에는 기존 policy의 CDD assignments를 지정한다. CLI CDD와 policy CDD를 동시에 지정하면 거부하며 자동으로 덮어쓰지 않는다. CLI DBC/CDD path는 현재 작업 폴더 기준이다.

출력의 `decoded_frame.record.location`과 payload/transaction의 `data_locations`가 같은 원본 위치를 가리킨다. 공통 `timeline_key`는 DBC·CDD·route·policy·원본 identity에 묶인다. report는 `dbc_counts`, `kwp_counts`/`uds_counts`, `cdd_counts`를 각각 유지하며 어느 단계가 partial인지 구분한다. DBC도 분석 전후 SHA를 확인하고 출력/report가 DBC 실체를 덮어쓰지 않도록 보호한다.

`-o output.jsonl --report report.json`을 추가하면 결과와 품질 집계를 저장한다. 기존 출력은 기본 보호하며 `--overwrite`로 데이터 출력만 교체할 수 있다. report는 새 파일이어야 한다. 원본·정책·CDD를 출력으로 덮어쓸 수 없다. 실제 샘플의 미정의 요청·경고·응답 누락 때문에 **종료 코드 3인 부분 성공이 정상적인 관측 결과**다.

## CDD가 없는 raw 출력

CDD assignment가 없는 경로는 서비스나 요청·응답 관계를 추측하지 않는다. ISO-TP `payload`의 `data_hex`와 원본 위치를 그대로 출력하며, DBC를 지정하면 `decoded_frame`도 함께 출력한다. `kwp_transaction`/`kwp_pending`은 CDD를 지정한 경로에서만 생성한다. 기본 빌드에서도 CDD 없는 raw 출력을 사용할 수 있다.

```powershell
.\dist\canlog.exe kwp "$s\asc\Logging\ComfortDiagData.asc" `
  --routes .\examples\asc-dbc-cdd\comfort.routes.json `
  --policy .\examples\asc-dbc-cdd\comfort.policy.json
```

진단 replay에서도 같은 routes/policy에 `--protocol kwp2000-vector`를 지정하고 CDD 옵션을 생략하면 raw payload를 재생한다.

## 지원 경계와 CDD 엔진

canlog의 내부 KWP SID 목록, 헤더 길이, echo, DTC count 및 TesterPresent mode 판별 코드는 제거했다. 별도 [CDD 엔진 master의 업데이트](https://github.com/najari/cdd-rust-engine/commit/ecd4a6a42792636a8386439653950e8693818d02)를 Cargo Git revision으로 고정한다. 빌드 시 Cargo가 가져와 컴파일하며 실행 시 GitHub나 Python에 접속하지 않는다. 기본 빌드는 Rust 1.88, `-Cdd` 빌드는 Rust 1.98.1 이상이 필요하다.

CDD가 지정되면 엔진의 `identify`, `request_context`, `decode_response`가 서비스 식별과 응답 검증을 결정한다. 서비스 SID와 key도 엔진 모델에서 얻는다. 실험적 공통 profile은 `uds-candela-2x`이며 provenance의 `protocol`이 `kwp2000`인지 함께 확인한다. legacy inline 8-bit static과 BCD를 포함한 native field key, wire span, provenance, diagnostics를 유지한다.

CDD에서 유일하게 식별되지 않는 요청은 `kwp_issue`로 raw payload와 native identification을 남긴다. 응답도 해당 native 서비스 정의에 맞아야 transaction으로 연결한다. prefix만 검증 가능한 응답은 ambiguous/unverified, 매칭되지 않는 응답은 orphan으로 남긴다. 미정의 요청을 처리할 별도 KWP fallback은 없다. 같은 SID의 negative 응답 등 여러 요청에 대응할 수 있는 경우 FIFO로 연결하지 않는다.

canlog는 ISO-TP, 재생 시간 제어와 P2/P2*, 전체 관측·자원 상한, transport gap/EOF 상태만 관리한다. NRC pending 값은 엔진의 decoded negative 결과에서 얻는다. KWP mode byte를 UDS suppress bit로 해석하거나 CDD 밖의 TesterPresent suppression 규칙을 적용하지 않는다. EOF만으로 timeout 또는 성공을 확정하지 않는다. UDS의 기존 헤더 profile은 유지한다.

새 분석 identity는 `canlog-kwp2000-cdd-physical-v2`다. 이전 내부 profile이 CDD 없는 요청도 positive로 세던 결과와 집계가 달라질 수 있다. Vector 샘플의 미정의 session/reset 및 미지원 DTC proxy는 부분 결과로 남는다. [실샘플 검증](kwp-cdd-validation.md)을 참고한다.
