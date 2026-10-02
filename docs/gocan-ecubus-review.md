# gocan · EcuBus-Pro 검토와 CANLOG-RS 반영안

검토일: 2026-10-02. README뿐 아니라 frame/capture/recorder, format reader/writer, 진단 codec, Trace IPC와 관련 테스트를 확인했다. 아래 판단은 확인한 commit 범위에 한정한다.

| 프로젝트 | 확인 revision | 성격 | CANLOG-RS에서의 활용 |
|---|---|---|---|
| [tomrford/gocan](https://github.com/tomrford/gocan) | `d35e092d7787dd02a1e89884af1e713d132ec72e` | Go CAN/CAN FD 통신·capture·protocol library | core 불변 조건, cursor/기록 lifecycle, codec 품질, 제한된 MF4 Writer 참고 |
| [ecubus/EcuBus-Pro](https://github.com/ecubus/EcuBus-Pro) | `86f6e1bab0de1ab7910554e50d06f23fed7cf073` | Electron/Vue/TypeScript 기반 ECU 개발·진단·시험 도구; manifest 버전 0.8.67 | Workspace/Trace UX, channel assignment, ASC/BLF parser 비교, GUI/CLI workflow 참고 |

**권고:** gocan을 주요 아키텍처·Writer·회귀 사례의 참고 프로젝트에 추가하고, EcuBus-Pro는 사용자 workflow와 ASC/BLF 비교 대상으로 추가한다. 현재 선택한 Rust DBC/CDD Engine은 유지한다. 두 프로젝트의 통신·시험 기능을 CANLOG-RS MVP에 모두 가져오지는 않는다.

## 1. gocan — 참고 우선순위가 높은 이유

### 1.1 Frame과 관측 event의 분리

`Frame`은 ID, 고정 64-byte owned data, on-wire DLC, flags를 보유한다. `DataLength`가 DLC→byte 길이를 계산하고 Remote는 payload 길이 0이다. CAN FD+Remote, Classic+BRS/ESI, ID 범위를 검증한다. Classic DLC 9〜15를 8-byte payload와 구분하는 설계도 확인된다.

`FrameEvent`는 frame에 BusID/time/direction을 결합한다. CAN error, controller state, receive overrun은 별도 Event이며 controller error count의 unavailable과 실제 0을 구별한다. 데이터와 관측 상태를 같은 frame으로 우겨 넣지 않는 점이 CANLOG-RS의 모델에 잘 맞는다.

**반영:** CanErrorRecord를 유지하고 `ControllerState`, `CaptureGap/ReceiveOverrun` event와 known/unknown 상태를 추가한다. gap은 UDS/validation 품질에 전파한다. unavailable counter를 0으로 저장하지 않는다.

근거: [frame.go](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/frame.go), [event.go](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/event.go).

### 1.2 Capture, Cursor와 retention

Capture는 append 순서의 frame/event를 chunk에 저장한다. Cursor는 timestamp나 per-signal offset이 아니라 capture 진행 위치다. 다른 generation, clear/prune으로 사라진 이력은 `ErrCursorOutOfRange`로 드러낸다. 읽기는 호출 시점의 snapshot을 사용하며 이후 append가 섞이지 않는다.

Prune은 소비자 cursor 중 가장 느린 위치를 기준으로 sealed chunk를 삭제한다. **Capture 자체는 retention policy가 없어서 호출자가 prune하지 않으면 계속 커진다.** 유용한 chunk 구조가 자동으로 bounded-memory를 보장하는 것은 아니다. Frames/Series의 전체 복사 API도 대용량 disk query의 기본 경로로 쓰면 안 된다.

gocan의 Cursor generation은 process-local이다. 그대로 영구 checkpoint로 직렬화해서 새 프로세스에서 재사용할 모델은 아니다. cursor interval `(start,end]`도 CANLOG-RS 시간 필터 `[start,end)`와 다른 계약이다.

**반영:** batch 진행 위치와 원본 RecordRef를 구별한다. 영구 checkpoint에는 source revision, parser/profile 버전, 재개 가능한 seek anchor, 마지막 확정 ordinal, 작업 설정 hash를 넣는다. 오래된 revision/anchor를 거부하며 live ring은 후속 기능으로 격리한다.

근거: [capture.go](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/capture.go), [capture_range.go](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/capture_range.go), [capture tests](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/capture_test.go).

### 1.3 accepted / flushed / finalized / durable

Recorder는 Writer에 전달한 Accepted와 Flush 성공 후의 Flushed를 구분한다. pruning은 Accepted가 아니라 Flushed 기준이다. Writer Close가 포맷을 완성하더라도 underlying file close/sync와 동일하지 않다고 명시한다. writer 실패나 stale cursor에서 자동 재시도로 손실을 숨기지 않는다.

**반영:** Application의 WriteReport/Progress에 완료 수준을 정의한다. accepted, flushed, finalized, durable, published는 서로 다르다. cache coverage는 transaction commit 후 complete이고 파일 출력은 finish/publish 후에 성공이다. checkpoint가 있다는 사실만으로 BLF/MF4 중단 파일의 재개 가능성을 주장하지 않는다.

근거: [recorder.go](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/recorder/recorder.go), [recorder tests](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/recorder/recorder_test.go).

### 1.4 MF4는 raw CAN Writer profile로 읽어야 한다

확인한 `mf4` package는 MDF 4.10의 raw CAN/CAN FD Writer다. DBC attachment, timestamped event marker, 선택적 Deflate, application metadata, bus name을 제공한다. frame buffer는 64 KiB이며 seekable 빈 출력이 필요하다. decoded measurement channel Writer, 기존 파일 append, interrupted file recovery는 지원하지 않는다고 명시한다.

시간은 상대 float64 seconds로 저장한다. package 문서는 같은 time master의 equal timestamp를 허용하는 정책에 strict ordering 관련 호환성 제한이 있음을 직접 밝힌다. 이 내용을 사양 적합성 검증 완료로 읽으면 안 된다. event marker와 frame stream의 개별 순서가 원본 전체 interleave 순서를 복원해 주는 것도 아니다.

**반영:** MF4 전체 Writer 대신 `MdfCanBusWriterProfile`을 별도 범위로 정의한다. attachment/event/history/metadata 정책과 출력 precision을 보고하고 외부 reader 검증을 추가한다. 전체 measurement Writer보다 앞서 작은 실험을 할 수 있지만, 기본 MF4 Reader의 우선순위를 뒤집을 근거는 아니다.

근거: [mf4/writer.go](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/mf4/writer.go), [MF4 tests](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/mf4/writer_test.go), [참조 revision lock](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/.repos/.lock).

확인한 tree의 ASC도 Writer 중심이며 BLF/MF4의 일반 Reader는 확인되지 않았다. gocan을 ASC/BLF/MF4 분석 엔진 전체의 대체재로 평가하지 않는다.

### 1.5 ISO-TP/UDS와 DBC/CDD 비교 가치

ISO-TP physical link는 normal addressing, Classic/FD 송수신, FC/timeout/padding 등을 구현한다. functional 경로는 single-frame 송신 전용이며 응답은 physical 주소로 처리한다. UDS Client는 요청을 보내고 P2/P2*로 기다린다. **이는 passive offline reassembly/matching과 다른 실행 모델**이다. 송신·FC 생성·wall-clock deadline을 canlog에 그대로 이식하지 않는다.

DBC의 `ErrInactiveSignal`은 multiplex 비활성 상태를 오류/손상과 구분한다. shared scalar codec은 identity scaling에서 정수 타입을 유지한다. CDD는 ECU/variant를 선택하며 index가 해당 document에 국한됨을 명시한다. `base` 이름만으로 variant inheritance를 임의 추론하지 않는다. duplicate reference도 ambiguous로 남긴다.

**반영:** raw/physical/label/validity를 별도 필드로 둔다. inactive signal은 malformed나 관측 누락과 구분한다. DBC/CDD 경계와 scalar 경계 사례를 Rust engine의 differential fixture 후보로 삼는다. 각 구현의 지원 profile·길이·enum 정책이 다르므로 값이 같아야 하는 범위를 먼저 정한다.

근거: [ISO-TP](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/isotp/isotp.go), [functional](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/isotp/functional.go), [UDS](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/uds/uds.go), [DBC codec](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/dbc/codec.go), [scalar](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/internal/scalar/scalar.go), [CDD document](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/cdd/document.go).

## 2. EcuBus-Pro — 제품 workflow와 ASC/BLF 참고

### 2.1 GUI/CLI와 Trace UX

Main process가 transport/diagnostics/IPC를, Vue renderer가 UI를 담당한다. CLI의 seq/test/build 기능과 GUI가 같은 프로젝트·script workflow를 사용한다. GUI/CLI/Python을 Application API 위에 두려는 CANLOG-RS 방향을 뒷받침한다. Electron IPC 자체를 domain API로 복제할 이유는 없다.

Trace는 BLF/ASC drag-and-drop, log channel→project device→DBC mapping, frame 상세와 signal 상세, 상대시각/UTC/Δt, ID/DLC/LEN, column 설정을 제공한다. UI상의 bounded live trace와 별도 logger도 구분한다.

**반영:** 향후 GUI의 source/channel assignment, frame provenance와 decoded detail, time 기준 표시를 설계한다. device binding 없이도 offline 분석할 수 있도록 workspace의 논리 channel에 매핑한다. UI에 보이는 행만 export하는지 전체 query 결과인지 명시한다.

근거: [Trace 문서](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/docs/en/um/trace/trace.md), [Trace UI](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/renderer/src/views/uds/trace.vue), [CLI 문서](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/docs/en/um/cli/cli.md).

### 2.2 페이지 UI와 대용량 처리는 다르다

`ipc-trace-file-open`은 Reader의 모든 frame을 `frames[]`에 넣고 session에 보관한다. `ipc-trace-file-page`는 그 배열을 slice한다. UI pagination은 존재하지만 **이 경로의 backend memory는 전체 frame 수에 비례한다.** 작은 로그에는 가능한 선택이지만 수십 GB 목표의 canlog에는 맞지 않는다.

실시간 Trace 50,000행 제한과 file session 전량 적재는 서로 다른 경로다. Reader 자체에 stream이 있다는 사실만으로 최종 Application의 bounded memory가 보장되지는 않는다.

**반영:** `query_page`는 source revision/query hash에 묶인 continuation token, file index seek, bounded decoded page cache를 사용한다. 확정되지 않은 총건수는 unknown으로 반환한다. 모든 결과를 먼저 Vec에 모으거나 `OFFSET`마다 처음부터 스캔하는 API를 기본으로 하지 않는다.

근거: [Trace file IPC](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/ipc/uds.ts)의 `ipc-trace-file-open/page/close`.

### 2.3 Reader와 재생 스케줄러를 분리해야 한다

ASC/BLF Reader의 `readFrame`은 replay speed, pause/resume, wall-clock delay를 포함한다. replay 제품에서는 편리하지만 분석/index/convert Reader에 이 책임을 넣으면 배치 작업이 재생 timing과 얽힌다.

**반영:** LogReader는 데이터를 읽고 오류·provenance를 반환한다. 화면 replay가 필요할 때만 별도 PlaybackScheduler가 timestamp→wall clock, speed/pause/cancel을 담당한다. 기본 Application 기능은 입력 frame을 송신하지 않는다.

README/일부 replay 문서는 ASC만 지원한다고 설명하지만, 확인한 코드와 replay tests에는 ASC/BLF Reader가 있다. 지원 범위는 문서 한 줄 대신 코드/profile/fixture로 판정한다. 이번 검토한 replay 처리 경로는 offline 로그 injection이며 일반 hardware replay가 검증됐다고 주장하지 않는다.

근거: [Replay API](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/replay/index.ts), [ASC Reader](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/replay/ascReader.ts), [BLF Reader](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/replay/blfReader.ts), [Replay 문서](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/docs/en/um/replay/replay.md).

### 2.4 ASC/BLF 호환성 비교의 제한

BLF Reader는 CAN_MESSAGE/CAN_MESSAGE2/CAN_ERROR_EXT/CAN_FD_MESSAGE/CAN_FD_MESSAGE_64 경로와 cross-container carry를 포함한다. Writer는 container와 zlib를 사용한다. 이는 구현 비교와 fixture 선정에 유용하다. 단, 다음 차이를 시험 판정에 반영해야 한다.

| 항목 | 확인한 동작 | CANLOG-RS 결정 |
|---|---|---|
| Timestamp | ns flag의 정수를 `tsRaw / 1000n`으로 µs number에 변환 | 원시 해상도·정수 ns를 유지하고 비교 시 상대 구현의 양자화를 명시 |
| 원시 DLC | CAN parse 반환 frame에 raw DLC가 없음 | raw DLC와 payload length를 구분해서 유지 |
| FD ESI | replay msgType은 ID kind/BRS/FD/Remote만 포함 | ESI를 독립 필드로 보존 |
| Error | `id=0`, empty payload, isError로 표시 | 별도 CanErrorRecord에 보존 |
| 미지원/손상 | 일부 경로에서 null/return으로 넘김 | ParseIssue/Unsupported와 report로 표시 |
| 압축 상한 | 선언 uncompressed size를 검사하지만 inflateSync에 실제 출력 상한을 전달하지 않음 | 선언과 실제 확장량을 각각 제한·검증 |

Timestamp/DLC/ESI는 source의 parse 함수만 분리한 작은 Node 검사에서 재현했다. 1234ns timestamp가 1µs가 되고, Classic DLC=9 frame의 payload 8bytes는 남지만 raw DLC가 없으며, FD EDL+BRS+ESI 입력에서 ESI가 반환되지 않았다. 함수 본문은 변경하지 않았고, app imports/type-only imports와 enum constants만 실행용으로 분리했다. 이 검사는 Electron 앱 전체 테스트가 아니다.

압축 상한은 정적 검토 사항이다. 잘못된 declared size가 실제 inflate allocation 상한이 될 수 없다는 점을 확인했으며 공격 재현·peak memory benchmark는 수행하지 않았다. writer의 압축 queue와 stream write의 backpressure/error 전달도 canlog에서 독립 검증해야 한다. UI 또는 writer의 작은 buffer 크기만으로 전체 queue budget을 보장하지 않는다.

근거: 위 [BLF Reader](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/replay/blfReader.ts), [BLF Writer](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/transport/blf.ts), [frame 타입](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/share/can.ts).

### 2.5 CDD와 플랫폼 범위

CDD IPC는 별도 Python cddparse script를 호출하고 JSON 결과를 읽는다. CANLOG-RS의 기존 Rust CDD Engine을 이를 위해 대체하지 않는다. 사용자 입장에서 필요한 CDD import/tester 설정과 field 표시를 참고한다.

루트 manifest의 vendor 설정은 Windows가 여러 vendor, Linux/macOS는 simulate/slcan으로 되어 있다. cross-platform UI와 모든 OS의 모든 장치 지원은 다른 주장이다. offline canlog MVP에는 vendor SDK를 요구하지 않는다. 검토한 경로에서 MF4 일반 Reader/Writer, SQLite 대용량 분석 계층, Parquet export는 확인하지 못했다.

근거: [CDD IPC](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/src/main/ipc/cdd.ts), [package.json](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/package.json).

## 3. 비교와 우선순위

| 관심 영역 | gocan | EcuBus-Pro | 적용 우선순위 |
|---|---|---|---|
| Core data/event 모델 | owned frame·검증·event 구분 | replay/UI용 message | gocan 우선, canlog time/provenance 모델 유지 |
| 대용량 offline 로그 | capture retention은 별도 정책 필요 | Trace file은 전량 적재 | 둘을 그대로 복제하지 않고 index/stream 유지 |
| ASC/BLF | ASC Writer; BLF 일반 Reader 미확인 | ASC/BLF Reader/Writer 경로 존재 | EcuBus 비교 fixture 추가 |
| MF4 | 제한된 MDF 4.10 raw CAN Writer | 검토 범위에서 미확인 | gocan Writer profile 연구 |
| ISO-TP/UDS | active protocol library | active diagnostics/test workflow | edge cases 참고, passive engine 별도 |
| DBC/CDD | 의미 codec·선택/품질 사례 | import/GUI/tester workflow | Rust engines 유지, differential 비교 후보 |
| GUI/automation | library 중심 | Trace/graph/sequence/CLI | EcuBus 제품 흐름 참고 |

**현재 우선순위:** cursor/완료 단계/event 품질/Reader 분리 → ASC/BLF fixture와 손실 검증 → index 기반 query_page → MF4 raw CAN Writer의 제한된 연구 → GUI/실시간/active protocol 후속 확장.

J1939, XCP, DoIP, LIN diagnostics, 자동 송신/HIL은 참고 목록에는 넣되 MVP 완료 조건을 확대하지 않는다.

## 4. 검증 범위와 재사용 조건

- gocan: README, core/capture/recorder, ASC/MF4 Writer, ISO-TP/UDS, DBC/CDD/scalar, 관련 tests/CI를 정적으로 검토했다. Go SDK가 확인되지 않아 `go test -race ./...`를 실행하지 않았다. `/usr/bin/go`는 Go 언어 compiler가 아니다.
- EcuBus-Pro: 위 format/Trace/replay/IPC/manifest/docs/tests를 정적으로 검토하고 BLF parse 함수의 3개 제한을 isolated Node 검사로 재현했다. Electron/native/Python 환경 설치, 전체 Vitest, hardware tests는 실행하지 않았다.
- 두 upstream source는 수정하지 않았다. fixture의 배포 권한과 기대 동작을 검토하기 전 대량 복사하지 않는다. 외부 구현이 한 번 읽었다는 사실을 포맷 완전 적합성으로 표현하지 않는다.
- gocan은 [MIT](https://github.com/tomrford/gocan/blob/d35e092d7787dd02a1e89884af1e713d132ec72e/LICENSE), EcuBus-Pro는 [Apache-2.0](https://github.com/ecubus/EcuBus-Pro/blob/86f6e1bab0de1ab7910554e50d06f23fed7cf073/license.txt)다. 코드 이식 시 해당 notice/라이선스 조건을 따르고 vendor SDK와 sample은 별도로 확인한다. 이번 변경은 구현 복사가 아닌 설계 참고다.

변경 설계는 [CANLOG-RS 설계서](canlog-rs-design.md)의 34장에 반영했다. 기존 10·11장의 Rust engine 연동과 26장의 MVP 범위는 유지한다.
