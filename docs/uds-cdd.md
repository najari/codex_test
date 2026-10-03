# 파일 기반 UDS transaction과 선택적 CDD 해석

`uds`는 단일 로그를 파일 순서대로 읽고, 명시한 Classic CAN 물리 경로의 ISO-TP payload에서 요청·응답을 연결한다. 실제 송신이나 ECU 상태 변경은 하지 않는다. CAN 경로 설정은 [isotp.md](isotp.md)를 따른다.

## 바로 실행하기

프로젝트 생성 로그에는 DefaultSession, VIN 요청·pending·positive와 negative 응답이 있다. CDD 없이도 요청 header·NRC·원문·시간·위치와 매칭 상태를 확인할 수 있다.

```powershell
.\dist\canlog.exe uds .\examples\uds-demo.asc --routes .\examples\uds-physical.routes.json --policy .\examples\uds-physical.policy.json
```

사용자가 제공한 cantools CDD의 실제 UDS 정의를 연결하려면 CDD 기능을 포함해 빌드한다. 기본 빌드는 Rust 1.88 이상이며, pinned CDD 엔진을 포함하는 빌드는 Rust 1.98.1 이상이 필요하다.

```powershell
.\scripts\build.ps1 -Cdd -Test -Release
.\dist\canlog.exe cdd-info "C:\Users\admin\claude\cdd_rust_engine\examples\cantools\cdd\example-diddatarefs.cdd" --allow-experimental
.\dist\canlog.exe uds .\examples\uds-session.asc --routes .\examples\uds-physical.routes.json --policy .\examples\uds-cantools.policy.json
```

예제 CDD policy는 위 로컬 파일, ECU `SBS_Test`, variant `Base_Variant`를 명시한다. 다른 설치 경로에서는 policy의 `cdd.path`를 변경한다. 상대 CDD 경로는 **policy 파일이 있는 폴더 기준**이다. `cdd-info`의 ECU·variant qualifier를 확인하고 해당 로그를 설명하는 정의를 직접 배정한다. 같은 폴더에 있다는 이유로 로그와 CDD의 관계를 추정하지 않는다.

`-o output.jsonl --report report.json`으로 결과를 저장할 수 있다. 기본값은 기존 출력을 보호하며 교체에는 `--overwrite`가 필요하다. report는 새 경로여야 한다. source·routes·policy·ID map·CDD와 같은 파일 또는 hardlink 실체에는 출력/report를 저장할 수 없다. 분석 전후 내용 hash를 확인하고, 파일 결과는 완료 후 게시한다. stdout은 실패 시 이미 출력된 내용을 되돌릴 수 없다. 결과와 report는 별도 원자적 파일로 저장한다.

`uds`에도 `--dbc CHANNEL=PATH`를 반복 지정해 DBC 프레임과 CDD transaction을 한 JSONL에 기록할 수 있다. CDD assignment가 없는 단일 route policy에는 `--cdd PATH --ecu QUAL --variant QUAL --allow-experimental`을 직접 지정한다. 각 단계의 상태와 원본 frame 위치를 유지한다. 기존 KWP 로그를 사용하는 실제 [ASC·DBC·CDD 통합 예제](../examples/asc-dbc-cdd/README.md)를 참고한다.

## 매칭 계약

UDS policy schema 1은 각 transport route에 `protocol: "uds2013"`, `p2_ns`, `p2_star_ns`, `transaction_max_duration_ns`를 필수 지정한다. 기존 KWP 데모 로그에 UDS를 자동 적용하지 않는다. DiagnosticSessionControl의 응답에 있는 timing 값을 다음 요청의 policy로 자동 적용하지 않는다.

| SID | 지원하는 header/echo profile |
|---|---|
| `10`, `11` | subfunction, UDS 2013 세션 timing/Reset 응답 길이 |
| `14` | 3-byte DTC group; positive SID |
| `19` | subfunction 01/02, status mask, count/list 응답 구조 |
| `22`, `2E` | DID echo; 읽기 multi-DID positive는 경계 미확정으로 ambiguous |
| `27` | security subfunction echo; seed/key 데이터는 CDD 없으면 opaque |
| `31` | control subfunction와 routine ID echo |
| `34` | ALFID 요청 길이, LFI positive 구조 |
| `36` | block sequence counter echo; 상위 bit도 counter 그대로 유지 |
| `37` | positive SID와 선택 record |
| `3E` | subfunction 00와 suppress-positive bit |

route·SID·해당 echo가 일치하며 시간 범위가 유효한 요청이 하나일 때만 매칭한다. 반복 요청/동일 SID의 negative·pending에서 후보가 여러 개면 후보 key와 원문을 남기고 ambiguous로 닫는다. 다른 DID·routine·counter 응답을 최신 요청에 임의로 붙이지 않는다. 서비스 전체의 ECU 상태 전이, 모든 subfunction, 제조사 data record의 의미를 검증하는 기능은 아니다.

P2는 요청의 마지막 transport frame부터 응답의 첫 transport frame까지 관측한다. NRC `78`은 transaction을 열어 두고 P2*를 pending 응답 완료부터 다시 계산한다. 전체 관측 상한은 요청 완료부터 고정되어 pending이 계속 와도 늘어나지 않는다. P2/P2* 내에 시작한 FF는 재조립 완료까지 기다리되 전체 상한을 넘지 않는다. 동일 timestamp에서는 원본 record 순서까지 검사한다. latency는 `request_last_to_response_first_ns`와 `request_first_to_response_last_ns`를 구분한다.

| 상태 | 의미 |
|---|---|
| `positive`, `negative` | 고유한 요청에 연결된 완료 응답; negative도 분석은 완료 |
| `pending` | NRC 78 중간 관측; 같은 transaction key로 후속 결과 연결 |
| `pending_only` | pending 이후 응답 deadline/관측 상한/count 상한 도달 |
| `no_response_observed` | 로그가 deadline을 지나 진행했지만 매칭 응답 없음 |
| `suppressed_expected` | suppress-positive 요청의 deadline을 관측했고 응답 없음 |
| `ambiguous`, `orphan` | 후보가 여러 개/미확정 또는 적합한 요청 없음 |
| `incomplete` | capture gap·불완전 transport·EOF·수신 중 관측 상한 등으로 단정 불가 |
| `malformed`, `unsupported`, `resource_limit` | header/지원 profile/자원 경계 문제 |

EOF만으로 ECU timeout이나 suppress 성공을 단정하지 않는다. `--unsupported skip`/recover로 제외한 기록은 capture gap으로 남기고 열린 문맥을 보수적으로 닫는다. 미해석 요청과 자원 제한으로 놓친 요청도 해당 route의 열린 문맥을 중단한다.

## CDD facade와 provenance

선택적 Cargo feature `cdd`는 [cdd-rust-engine](https://github.com/najari/cdd-rust-engine)의 commit `9207dfc845d5d256eb479073824bad234b27f79b`를 Git dependency로 고정한다. DBC 엔진과 같은 방식으로 빌드 시 Cargo가 cache/fetch하고 실제 Rust facade·codec을 컴파일한다. canlog에 CDD XML parser의 일부를 복제하지 않는다. 생성한 exe는 실행 시 GitHub나 Python에 접속하지 않는다. 원본 CDD는 runtime 입력 파일이다.

해당 엔진의 profile maturity는 **experimental**이다. route의 `cdd.allow_experimental: true` 또는 `cdd-info --allow-experimental`을 명시해야 한다. `cdd-info`는 Unknown 프로토콜도 검사하지만 UDS route 배정은 모델이 Uds일 때만 허용한다.

매칭된 transaction의 요청을 ECU·variant context에서 식별하고 Unique definition에 한해 요청을 decode한 뒤 document fingerprint가 있는 RequestContext로 응답을 decode한다. `no_match`·`ambiguous`·`unverified`·`decode_error`를 유지한다. 매칭 불확실/불완전 transaction에는 `not_decoded`를 붙이며 추측해서 필드를 해석하지 않는다. multi-DID 응답은 CDD가 있어도 현재 matcher에서 ambiguous로 남는다.

CDD native JSON schema 2의 field key, 정확한 raw 정수 문자열, physical 값, label/unit, wire span, definition/protocol NRC 해석과 diagnostics를 보존한다. canlog 관측은 document SHA-256·engine revision을 포함하며 요청 context와 native provenance에는 profile fingerprint·parser version이 있다. session/security의 실제 ECU 상태는 unknown으로 유지한다. CDD 서비스 정의에 없는 NRC는 `decoded_with_diagnostics`로 남겨 종료 코드 3으로 보고한다.

UDS 출력에는 기존 ISO-TP `payload`/`flow_control`/`protocol_issue`/`capture_gap`과 추가 `uds_transaction`/`uds_pending`/`uds_issue`가 함께 나온다. schema version은 1이며 각 row에 analyzer·timeline/result key가 있다. report는 transport count와 `uds_counts`, `cdd_counts`, CDD load diagnostics를 별도로 집계한다. row의 status와 report의 scan/published/partial 상태를 함께 확인한다.

## 상한과 현재 제한

policy 256 KiB, bindings 64개, outstanding 기본 64/최대 128, 요청 payload buffer 기본 256 KiB/최대 1 MiB이다. pending 예시는 transaction당 기본 16/최대 64, 관측 횟수는 기본 256/최대 65536이다. P2/P2* 최대 60초, 전체 관측 최대 600초이며 arithmetic overflow도 거부한다. ISO-TP의 payload/session 상한도 그대로 적용된다.

CDD assignments 최대 8개, 파일당 8 MiB/전체 32 MiB, ECU·variant qualifier 128 bytes, load issue 예시 100개이다. `cdd-info`의 ECU·각 variant·service·DID 예시는 각각 128개까지 출력하고 생략 수를 남긴다. CDD parse 내부는 외부 엔진에 맡기며 cancellation은 호출 전후에 검사한다.

CAN FD/escape length, functional multi-responder, 여러 clock/source 병합, query window, 영속 diagnostic cache/checkpoint/resume, 실시간 송수신은 후속 작업이다. CDD 필드를 DBC 신호 CSV에 섞어 export하거나 workspace에 저장하는 기능도 아직 없다.

## 독립 검증 재현

빌드·실제 corpus·독립 비교 기록은 [uds-cdd-validation.md](uds-cdd-validation.md)에 있다.

```powershell
.\target\verify-env\Scripts\python.exe -m pip install udsoncan==1.26.1 can-isotp==2.0.7 python-can==4.6.1 cantools==43.0.2
.\target\verify-env\Scripts\python.exe .\scripts\verify_uds_cdd.py --exe .\dist\canlog.exe --artifacts .\artifacts\uds_cdd_new_run --cdd-samples C:\Users\admin\claude\cdd_rust_engine\examples\cantools --vector-cdd C:\Users\admin\claude\cdd_rust_engine\examples\vector_example\UDS-ExampleEcu-5.1.0.cdd
```

독립 Python 도구는 테스트에만 사용한다. 결과의 `passed`, 명령 exit code, source/binary SHA와 실제 비교 항목은 artifacts의 `results.json`에 남는다. 원본 CDD를 수정하지 않는다.

제공 corpus 결과: `example-diddatarefs.cdd`는 Uds/`SBS_Test`/`Base_Variant`이며 세션 P2=50, P2Ex raw=500/physical=5000을 cantools와 비교한다. `example.cdd`·`le-example.cdd`·`invalid-bo-example.cdd`의 모델 protocol은 Unknown이며 UDS 배정은 거부한다. invalid byte order `4321`은 엔진의 `CDD-BIT-004`와 독립 cantools의 오류를 비교한다. cantools의 진단 identifier 목록에는 여러 서비스의 subfunction도 포함되므로 그 개수를 엔진의 UDS DID 개수와 동일하다고 가정하지 않는다. session definition은 충돌하는 identifier dictionary 대신 이름·identifier로 선택한다.

추가 Vector UDS CDD에서는 VIN과 세션 timing, NRC 78/31의 protocol fallback 및 서비스 정의 경고를 확인한다. 이 검증은 사용한 definition/profile에 대한 비교이며 모든 CDD 버전의 적합성 인증은 아니다. timing과 서비스 echo 근거는 [udsoncan client 문서](https://udsoncan.readthedocs.io/en/latest/udsoncan/client.html)와 [서비스 문서](https://udsoncan.readthedocs.io/en/latest/udsoncan/services.html)를 참고한다.
