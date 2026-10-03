# 현재 구현 범위와 후속 작업

2026-10-03 기준. 장기 설계는 목표 계약이며 이 문서는 현재 실행 코드와 검증을 기준으로 구분한다.

## 완료한 파일 기반 기능

- ASC/BLF/JSONL streaming 읽기·쓰기, raw CSV export, 필터/변환/통계, 파일 record/replay와 속도/반복/제어/취소.
- 지원 ASC/BLF profile의 native event/error/unknown 보존, symbolic ASC ID의 명시 mapping, 손실 정책과 안전한 file publication.
- SQLite sparse index, 원본/정책 identity 검증, 시간·채널·ID 검색.
- pinned Rust DBC parser/compiled codec, 채널별 assignment, typed raw/physical/label/inactive/error 해석.
- 여러 로그의 workspace manifest, stored DBC binding, content-verified relink, quota/LRU/checksum이 있는 영속 typed signal cache.
- DBC 재생 콘솔/JSONL 및 decode/workspace decode/replay의 [versioned 신호 CSV](signal-csv.md).
- 단일 파일 Classic CAN의 normal/extended/mixed 물리 경로 [수동 ISO-TP 재조립](isotp.md), 양방향 SF/FF/CF/FC·순번/timeout/gap/EOF 품질과 자원 상한.
- 물리 경로의 [UDS transaction과 선택적 pinned CDD facade](uds-cdd.md): 서비스 echo·positive/negative/pending, P2/P2*·관측 상한, ECU/variant 정의·원문·typed field·provenance·diagnostics.

- 기존 CANSystem ASC의 [KWP2000 물리 transaction·CDD 해석](kwp-cdd.md): CDD 엔진 기반 서비스 식별·요청/응답 해석, pending/negative, 실험적 legacy inline 8-bit static과 BCD. CDD 없으면 raw payload 출력. 응답 없는 TesterPresent·미정의 legacy 요청·빈 DTC response proxy는 부분 결과로 유지.

- ASC·DBC·CDD [통합 CLI 예제](../examples/asc-dbc-cdd/README.md): 한 scan의 DBC frame·transport·진단 결과, 공통 identity와 원본 위치, 단계별 품질·원본 보호.
- ASC·DBC·CDD [진단 replay](../examples/asc-dbc-cdd/replay.md): 단일 전체 scan의 배속·즉시 재생·콘솔 필드/JSONL·pause/resume/stop, 원본 시간과 진단 품질·입력 및 publication 보호.

## 다음 구현 순서

| 순서 | 작업 | 구현과 검증에 필요한 경계 |
|---|---|---|
| 1 | ISO-TP profile 확장 | 현재 Classic 물리 경로 구현을 기반으로 CAN FD/escape length, functional multi-responder, 결과 window/context와 clock mapping을 별도 검증 |
| 2 | UDS/KWP/CDD profile 확장 | 현재 물리 매칭/facade를 기반으로 functional responder별 상태, 추가 서비스, definition 기반 multi-DID, window/context, 별도 진단 export/cache 검증 |
| 3 | 분석 checkpoint와 안전한 재개 | identity 검증, 열린 ISO-TP/UDS부터 safe replay, stable result key/coverage/checkpoint의 같은 SQLite transaction, 강제 종료 후 동등성 |
| 4 | bounded query pages | source/query/settings identity에 묶인 continuation token, 동일 timestamp/역행에서 중복·누락/변경 검출, unknown total/has_more |
| 5 | MF4 Reader beta | 실제 MDF profile/spec/sample 기반 metadata·bus logging·measurement 구분, raw/physical/invalid/time·resource 상한, 독립 Reader 비교 |
| 6 | Parquet와 고급 분석/export | typed column/clock/unit metadata, bounded row groups, cross-signal 시간 정렬/join, explicit multi-clock merge와 external sort |
| 7 | Frontend/장비 adapter | 같은 backend를 쓰는 GUI/Python/MCP와 선택 장비의 실제 capture/transmit adapter |

ASC의 모든 dialect, 여러 trigger clock 병합, event/error의 교차 포맷 typed 변환은 별도 profile 확장이다. SQLite schema migration/WAL writer와 crash-safe scan resume도 현재 구현에 포함되지 않는다. 전체 MF4/진단/GUI/장비 송수신이 완료되었다고 해석하지 않는다.

실제 장비 interface는 아직 선택되지 않았으므로 우선순위 1~6은 파일 기반으로 진행할 수 있다. CDD 엔진 내부 parser를 canlog에서 새로 작성하지 않고 기존 adapter 원칙을 유지한다. 원본 sample·원본 Vector 설치 폴더는 수정하지 않는다.
