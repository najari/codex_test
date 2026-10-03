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

```powershell
.\scripts\build.ps1 -Cdd -Release
.\examples\asc-dbc-cdd\run.ps1 -Sample comfort
.\examples\asc-dbc-cdd\run.ps1 -Sample engine
```

`kwp`와 `uds`에서 `--dbc CHANNEL=PATH`를 반복 지정할 수 있다. 같은 source를 한 번 읽으며 모든 CAN frame의 DBC 결과와 진단 결과를 함께 출력한다. 이 DBC의 `Diag_Request`/`Diag_Response`에는 신호가 없으므로 DBC 메시지 이름 확인과 CDD 진단 필드 해석을 구분한다.

`--cdd PATH --ecu QUALIFIER --variant QUALIFIER --allow-experimental`은 CDD assignment가 없는 단일 route policy에서 사용할 수 있다. 여러 route에는 기존 policy의 CDD assignments를 지정한다. CLI CDD와 policy CDD를 동시에 지정하면 거부하며 자동으로 덮어쓰지 않는다. CLI DBC/CDD path는 현재 작업 폴더 기준이다.

출력의 `decoded_frame.record.location`과 payload/transaction의 `data_locations`가 같은 원본 위치를 가리킨다. 공통 `timeline_key`는 DBC·CDD·route·policy·원본 identity에 묶인다. report는 `dbc_counts`, `kwp_counts`/`uds_counts`, `cdd_counts`를 각각 유지하며 어느 단계가 partial인지 구분한다. DBC도 분석 전후 SHA를 확인하고 출력/report가 DBC 실체를 덮어쓰지 않도록 보호한다.

`-o output.jsonl --report report.json`을 추가하면 결과와 품질 집계를 저장한다. 기존 출력은 기본 보호하며 `--overwrite`로 데이터 출력만 교체할 수 있다. report는 새 파일이어야 한다. 원본·정책·CDD를 출력으로 덮어쓸 수 없다. 실제 샘플의 미정의 요청·경고·응답 누락 때문에 **종료 코드 3인 부분 성공이 정상적인 관측 결과**다.

## 지원 경계

| SID | 이 profile의 요청과 positive 응답 |
|---|---|
| `10` | full DiagnosticMode byte echo; `81`을 suppress bit로 해석하지 않음 |
| `11` | full ResetMode echo; legacy reset record는 opaque |
| `14` | 2-byte DTC group echo |
| `18` | 요청 mode 02/03 + 2-byte group; 응답은 count + 2-byte DTC/1-byte status 목록, mode echo 없음 |
| `1A`, `21` | 1-byte local identifier echo; 데이터는 CDD 또는 opaque |
| `3B` | local identifier + 요청 데이터, 응답 local identifier echo |
| `20` | SID-only StopDiagnosticSession |
| `3E` | 요청 01은 `7E` 응답, 요청 02는 positive 응답을 기대하지 않음; UDS `7E 00`을 적용하지 않음 |

negative는 `7F SID NRC`, NRC `78`은 pending이다. echo가 없는 DTC/TesterPresent 또는 같은 SID의 negative에 후보가 여러 개면 ambiguous다. P2/P2*, 고정 전체 관측 상한, 자원 상한, transport gap와 EOF 처리는 [UDS 매칭 계약](uds-cdd.md)을 공유한다. EOF만으로 timeout 또는 suppress 성공을 확정하지 않는다. 제조사별 추가 서비스나 전체 KWP 규격의 지원을 의미하지 않는다.

KWP framing 설명은 [NI Automotive Diagnostic Command Set 매뉴얼](https://download.ni.com/support/manuals/372139d.pdf)을 참고했고, 위 mode·echo profile은 제공된 CDD의 프로토콜 정의와 실제 ASC로 검증했다.

## CDD 엔진과 검증

CDD parser를 canlog에 복제하지 않는다. 별도 [CDD 엔진의 KWP commit](https://github.com/najari/cdd-rust-engine/commit/9207dfc845d5d256eb479073824bad234b27f79b)을 Cargo Git revision으로 고정한다. 이 commit은 `codex/kwp2000-codec` 브랜치에 게시되어 있으며 upstream `master`에 병합된 상태는 아니다. 임시 local path patch 없이 빌드한다. 기본 빌드는 Rust 1.88, `-Cdd` 빌드는 Rust 1.98.1 이상이 필요하다. 실행 시 GitHub/Python 접속은 없다.

엔진은 실험적 `kwp-candela-2x` profile로 CDD가 선언한 SID·static·datatype·NRC를 해석한다. legacy `STATICCOMP bl="8"`에 `dtref`가 없는 경우만 unsigned 1-byte static을 지원한다. invalid `dtref`, wider inline width, 미충족 proxy와 지원 밖 component를 추측하지 않는다. BCD 값은 decimal raw 정수로 출력하며 native field key·wire span·provenance·diagnostics를 보존한다.

검증: [KWP·CDD 실샘플 결과](kwp-cdd-validation.md). 기존 ASC 3개에서 74 CAN frames, 66 ISO-TP payload, 24 positive transaction을 비교했다. Engine의 TesterPresent 요청 18개 중 17개는 응답 deadline 경과, 1개는 EOF incomplete다. Engine DTC response의 빈 CDD proxy 2개는 decode error로 남는다. Comfort의 legacy session/reset 요청 등도 미정의 상태를 유지한다.
