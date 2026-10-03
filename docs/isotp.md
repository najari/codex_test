# 파일 기반 ISO-TP 분석

`canlog isotp`는 단일 ASC/BLF/JSONL 파일을 끝까지 읽어 명시한 물리 주소 경로의 payload를 재조립한다. DBC 없이 실행하며 CAN/FC를 송신하지 않는다. UDS transaction pairing과 CDD 의미 해석은 다음 단계다.

```powershell
.\dist\canlog.exe isotp .\can_example\asf\ComfortDiagData.asc --routes .\examples\isotp-physical.routes.json
.\dist\canlog.exe isotp .\can_example\asf\EngineDiagData.asc --routes .\examples\isotp-physical.routes.json -o .\engine-isotp.jsonl --report .\engine-isotp-report.json
```

새 출력 경로를 사용한다. 기존 payload 파일 교체는 `--overwrite`로 허용하며 report는 항상 새 경로를 요구한다. JSONL stdout은 실행 중 결과를 내보내므로 나중에 역행/오류가 발견될 수 있다. 전체 성공을 확인해야 하는 처리에는 파일 출력과 report를 사용한다.

## 경로 설정

[예제 설정](../examples/isotp-physical.routes.json)은 로컬 Vector 데모 로그에서 관측한 채널/숫자 ID에 사용자가 명시적으로 부여한 경로다. Comfort 데모는 channel 1, request `0x700`, response `0x600`; Engine 데모는 channel 2, request `0x200`, response `0x400`다. 다른 ECU에 적용하려면 실제 설정에 맞춰 수정한다. ECU 식별이나 DBC/CDD 연결을 자동으로 추론하지 않는다.

```json
{
  "schema_version": 1,
  "routes": [{
    "name": "ecu-a",
    "channel": 1,
    "kind": "physical",
    "profile": "classic",
    "addressing": "extended",
    "request": {"id": 1792, "extended": false, "address": 17},
    "response": {"id": 1536, "extended": false, "address": 34},
    "timeout_ns": 1000000000
  }]
}
```

ID는 JSON 정수이며 `extended`는 CAN ID 종류를 뜻한다. `addressing=extended`와 별개다. `normal`은 address를 생략한다. `extended`는 양쪽의 서로 다른 첫 데이터 바이트를, `mixed`는 양쪽의 동일한 address 바이트를 지정한다. 양방향 CAN ID/종류는 달라야 한다. 채널·ID 종류·주소 바이트가 겹쳐 어느 경로인지 결정할 수 없는 설정은 거부한다. route 이름은 고유하다. 출력의 request/response는 설정에서 정한 역할이고 로그의 Rx/Tx와 독립적이다.

이 버전은 Classic CAN에서 normal/extended/mixed 주소, 표준/확장 CAN ID, SF의 4-bit 길이, FF의 12-bit 길이, CF의 modulo-16 순번, FC의 CTS/WAIT/Overflow를 처리한다. 기능 주소/다중 responder, CAN FD, SF escape와 긴 FF escape, clock merge와 재개 checkpoint는 미지원이다. 설정의 미지원 profile/kind와 알 수 없는 필드는 거부한다. 선택된 경로에서 FD나 미지원 PCI를 만나면 위치를 포함한 issue와 불완전 결과를 남긴다. 주소 바이트/ID 종류가 다르면 경로 미선택으로 처리한다. 모든 주소 체계의 규범적 적합성을 판정하는 기능은 아니다.

## 조립과 품질

- FF/CF 상태는 route·channel·방향별로 분리한다. 새 FF/SF는 기존 세션을 aborted로 끝낸다. CF의 순번 누락/중복은 확인한 prefix만 incomplete로 남기고 후속 CF를 앞 세션에 붙이지 않는다. 원인이 송신 오류인지 수집 유실인지 단정하지 않는다.
- FF는 8-byte CAN 데이터가 필요하며 SF로 보낼 수 있는 길이의 FF는 거부한다. 비최종 CF도 8-byte 데이터가 필요하다. 마지막 CF는 선언 길이까지만 사용한다. 기본적으로 나머지 padding 값을 무시한다. route에 `padding_byte`를 지정하면 SF/최종 CF/FC의 남은 바이트가 그 값인지 확인한다. padding 길이 자체를 강제하지 않는다.
- `timeout_ns`는 FF/CF 또는 그 세션의 반대 방향 FC가 마지막으로 관측된 이후의 **로그 시간 비활성 상한**이다. 새로운 레코드의 시간이 deadline보다 클 때 incomplete로 끝낸다. 정확히 deadline인 레코드는 허용한다. wall clock/P2/P2*/규범적인 전체 N_* 타이머를 대신하지 않는다. EOF에는 `end_of_file`로 열린 세션을 끝낸다.
- 관련 없는 채널/ID의 프레임과 시간이 알려진 issue도 원본 clock의 순서를 검사한다. 동일 timestamp는 원본 순서로 처리한다. 역행은 실행 실패이고 파일을 publish하지 않는다. 정렬하거나 UTC를 추정하지 않는다.
- `--unsupported skip`/`--recover`로 넘어간 reader issue는 보수적으로 capture gap으로 기록한다. 알려진 채널의 열린 세션 또는 채널 미상일 때 모든 세션을 incomplete로 끝낸다. 정상 bus 데이터가 빠졌다고 확정하는 의미는 아니다.
- CF/FC 문맥을 잃지 않도록 `--id`, `--channel`, `--id-kind`, `--direction`, `--start`, `--end`, `--limit` 필터를 거부한다. 필요한 경로는 routes로 지정한다. 입력 stdin/native preservation도 미지원이다.

FC는 반대 방향 세션에 연결한다. FC가 없어도 올바른 CF로 payload를 완성할 수 있으며 `fc_observation=missing`을 남긴다. FC status/BS/STmin 원본 바이트, 유효한 STmin ns 또는 reserved/null, CF 최소 간격, BS 초과·WAIT 뒤 CF·STmin보다 짧은 간격의 관측 횟수를 기록한다. 시각 해상도와 수집 누락 때문에 `protocol_compliance`는 항상 `unknown`이며 관측 횟수만으로 위반을 확정하지 않는다. BS/STmin의 세션 설정은 CTS에서 갱신하고 Overflow는 aborted로 끝낸다.

## 출력과 identity

JSONL schema 1의 `payload`, `flow_control`, `protocol_issue`, `capture_gap` 행을 출력한다. payload는 `complete/incomplete/aborted` 상태, 선언/관측 길이, 대문자 hex, 첫/마지막 시간·위치, data frame 위치 목록, FC 관측을 포함한다. incomplete/aborted의 `data_hex`는 검증한 prefix이므로 완성된 진단 응답으로 처리하지 않는다. FC 행의 direction은 FC를 송신한 endpoint 역할이고 세션 관측은 반대 방향 payload에 붙는다.

`timeline_key`는 원본 SHA-256, routes 파일 SHA-256, parser/ID map 정책, 초기 clock metadata, 단일 파일 순서 정책과 analyzer 버전을 포함한다. `result_key`는 timeline과 결과 내용을 결합한 SHA-256이다. 같은 입력/경로/정책/파일 경로는 재실행 시 동일 key를 얻는다. source metadata에는 경로가 포함되어 이동 후에는 key가 달라질 수 있다. hash 자체가 진단 cache/checkpoint의 구현을 뜻하지 않는다. 이 버전은 ISO-TP 결과를 SQLite에 저장하지 않는다.

report에는 처리한/경로에 맞는 프레임 수, 결과/이유별 counts, issue 예시 최대 100개, 생략 수, `scan_complete`, `published`, source/routes identity와 사용한 설정이 있다. metadata의 origin을 변환하거나 epoch로 추정하지 않는다. `scan_complete`는 EOF 후 identity 검사를 통과한 경우만 true다. `status=complete`는 scan과 payload 조립에서 기록된 오류가 없다는 뜻이며 프로토콜 준수나 요청/응답 거래 완료의 판정이 아니다. 완성된 payload에 FC 관측이 없거나 시간 관측 경고가 있어도 독립적으로 기록한다. 미선택 프레임은 `frames_examined - frames_matched`로 확인한다.

종료 코드는 complete `0`, 불완전/aborted/프로토콜 issue/수집 gap 등 partial `3`, 설정·파싱·역행·I/O 실패 `1`, 취소 `130`이다. partial 파일도 publish하며 각 행의 상태를 확인해야 한다. 원본, routes, ID map을 출력/report 경로 또는 hardlink로 덮어쓸 수 없다. 실행 전후 source/routes/map hash를 검사하고 임시 파일을 원자적으로 publish한다. payload와 report는 별도 파일 publication이며 함께 묶인 transaction은 아니다.

## 자원과 독립 검증

routes 파일은 256 KiB, 1~64개 route, 이름 128 UTF-8 bytes다. `max_payload_bytes` 기본/최대 4095, `max_total_payload_bytes` 기본 262144/최대 1048576, `max_sessions` 기본 64/최대 128, route `timeout_ns` 기본 1초/최대 60초다. FF allocation 전에 선언 길이·전체 예약 payload·세션 상한을 검사한다. data 위치는 유효한 조립 프레임만 추가하여 Classic profile의 최대 길이로 제한되며, FC 위치 예시는 세션당 16개와 생략 횟수로 제한한다. 이 buffer 상한은 payload 예약량이며 allocator·위치·serializer의 전체 메모리 상한과 같지 않다. 원본 frame stream과 결과는 누적 저장하지 않는다.

`scripts/verify_isotp.py`는 test-only python-can 4.6.1/can-isotp 2.0.7을 사용한다. 외부 ISO-TP 송신기가 만든 Classic 프레임을 외부 listen-only 수신기와 실제 canlog CLI로 각각 재조립한다. normal/extended/mixed × 11/29-bit ID × padding 유무 × 길이 1/6/7/8/12/100/120/255/4095와 로컬 Comfort/Engine ASC·변환 BLF를 비교한다. 독립 수신기는 FC를 송신하지 않았는지도 확인한다. 이 oracle은 양질의 payload 조립 비교이며 타이머·오류·padding 정책의 모든 적합성 판정을 대신하지 않는다. 경계·gap·역행·순번 오류·취소·경로 보호는 Rust/CLI 테스트로 확인한다. Python은 canlog 실행 의존성이 아니다.

2026-10-03 실행 검증: 기존 기능을 포함한 Rust/CLI 125 tests, Clippy `-D warnings`, fmt/diff check, Rust 1.88 all-targets, Release build를 통과했다. Release 실행 파일로 12개 CLI 명령을 실행해 실제 로그의 payload 결과 100개(ASC/BLF 각각 포함)와 생성 데이터 108개, 총 208 payload 결과가 외부 receiver와 일치했다. Comfort ASC의 20 frames는 18 complete payload + 1 FC 관측이다. Engine ASC의 36 frames는 32 complete payload + 2 FC 관측이다. ASC→BLF로 변환해 같은 payload를 각각 재확인했고 입력과 설정 SHA-256가 변하지 않았다. 로컬 실행 증거는 `artifacts/isotp_release_2026-10-03_01/results.json`, 사용한 binary SHA-256는 `d511200ed6b698c4637220fca0c4f16100f45bef0bc9f9b875279ef5966aaa46`다.

기술 근거: [Linux ISO-TP 문서](https://docs.kernel.org/networking/iso15765-2.html), [can-isotp implementation API](https://can-isotp.readthedocs.io/en/latest/isotp/implementation.html), [addressing API](https://can-isotp.readthedocs.io/en/latest/isotp/addressing.html). 이 제한된 profile을 ISO 15765-2의 모든 판본에 대한 공식 적합성 인증으로 해석하지 않는다.
