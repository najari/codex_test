# CANLOG-RS 설계서 — 보완판 v0.2

> Rust 기반 ASC / BLF / MF4 로그 분석·변환 CLI 및 Automotive Data Engine

## 개정 범위와 문서 상태

원안의 28개 장과 핵심 방향을 유지하되, 구현 시 해석이 갈릴 수 있는 데이터 계약·복구·저장·진단·검증 기준을 보완했다. 뒤에 변환 지원 범위, Application API, 운영 계약, 미확정 항목을 추가했다.

- **유지:** 원본 파일 보존, Rust 스트리밍 처리, 기존 DBC/CDD Engine의 Adapter 재사용, SQLite Catalog/Index/Cache, 공통 Application API.
- **수정:** 시간 모델, CAN/Error 모델, Reader/Writer 계약, 수동 ISO-TP 분석, UDS 다중 응답, SQLite 제약·캐시 버전, 질의의 시간 의미, 개발 단계별 완료 기준.
- **추가:** 변환 손실 보고, 지원 프로파일, 원본 변경 감지, 자원 상한, 취소·원자적 출력, 테스트 판정 기준, 외부 엔진 확인 목록.

이 문서는 목표 설계이며 구현 완료나 포맷 호환성을 의미하지 않는다. 현재 canlog 저장소에는 실행 코드가 없다. 사용자 지정 DBC/CDD 저장소의 소스와 테스트를 읽고 10·11장에 실제 API와 제약을 반영했다. canlog의 Rust API는 제안 계약이며, 외부 엔진 API는 확인한 commit을 기준으로 한다. 실차 로그와 진단 route는 별도로 필요하다. 호환성은 버전이 고정된 포맷 사양과 샘플 테스트로 입증한다.

## 1. 문서 목적

`canlog-rs`는 차량 네트워크 로그 파일인 **ASC, BLF, MF4(MDF4)** 를 읽고, 필터링·변환·분석·저장하는 Rust 기반 CLI 및 재사용 가능한 라이브러리 엔진을 목표로 한다.

기존에 구현된 **Rust DBC Engine**과 **Rust CDD Engine**을 재사용하며, 로그 엔진 내부에서 DBC/CDD 파서를 다시 구현하지 않는다. SQLite는 원본 로그를 대체하는 저장소가 아니라 **Workspace Catalog + Index + Cache + Analysis Result 저장소**로 사용한다.

주요 참고 구조는 다음과 같다.

- `python-can`: ASC/BLF Reader/Writer, 공통 Message/로그 스트림 추상화
- `asammdf`: MDF4/MF4 구조, channel/group, filter/cut/export, CAN/LIN bus logging
- 기존 Rust DBC Engine: CAN message/signal definition 및 encode/decode
- 기존 Rust CDD Engine: Diagnostic definition 및 UDS 데이터 해석

---

## 2. 핵심 설계 원칙

1. **원본 로그 파일은 원본 포맷으로 유지한다.**
2. ASC/BLF/MF4를 하나의 파일 포맷 구현에 종속시키지 않는다.
3. DBC/CDD Engine은 기존 Rust 구현을 Adapter를 통해 재사용한다.
4. ASC/BLF는 Frame-oriented, MF4는 Measurement + Bus Logging-oriented로 처리한다.
5. 대용량 로그는 전체 메모리 적재 없이 Streaming 처리한다.
6. SQLite는 Catalog/Index/Cache/Analysis에 사용하고 Raw Frame 전체 저장은 선택 기능으로 둔다.
7. ISO-TP와 UDS Transaction 계층을 로그 엔진과 CDD Engine 사이에 둔다.
8. CLI, 향후 GUI, Python Binding, MCP/AI가 동일한 Rust Application API를 사용하도록 한다.
9. 파일 포맷 계층과 분석/Query 계층을 분리한다.
10. DBC/CDD Engine의 내부 모델이 canlog 전체로 누출되지 않도록 Adapter 경계를 유지한다.

---

## 3. 전체 아키텍처

```text
                              ┌──────────────┐
                              │  canlog CLI  │
                              └──────┬───────┘
                                     │
                          Application / Query API
                                     │
            ┌────────────────────────┼────────────────────────┐
            │                        │                        │
            ▼                        ▼                        ▼
      Filter / Query             Analysis                Export
            │                        │                        │
            └────────────────────────┼────────────────────────┘
                                     │
                              Unified Model
                                     │
          ┌──────────────────────────┼──────────────────────────┐
          │                          │                          │
          ▼                          ▼                          ▼
      Log Engine               DBC Adapter                 ISO-TP
          │                          │                          │
   ┌──────┼──────┐                   ▼                          ▼
   ▼      ▼      ▼            Existing Rust               UDS Transaction
  ASC    BLF    MF4             DBC Engine                      │
                                                                 ▼
                                                           CDD Adapter
                                                                 │
                                                                 ▼
                                                          Existing Rust
                                                            CDD Engine
                                     │
                                     ▼
                              SQLite Workspace
                                     │
                ┌────────────────────┼────────────────────┐
                ▼                    ▼                    ▼
             Catalog               Index                Cache
                                                          │
                                            ┌─────────────┼─────────────┐
                                            ▼             ▼             ▼
                                         Signals         UDS        Analysis
```

---

## 4. 권장 Cargo Workspace

초기에는 crate 분리를 최소화하고 모듈로 경계를 검증한다. 독립 테스트, 의존성 격리, 선택 빌드가 필요한 시점에 분리한다. 처음부터 모든 crate를 빈 패키지로 만들지 않는다.

```text
automotive-tools/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── crates/
│   ├── canlog-core/          # Frame/Record/Time/Error/Reader 계약
│   ├── canlog-formats/       # asc/, blf/; 이후 mdf/ 분리 가능
│   ├── canlog-app/           # Application API, 실행 계획, filter/query/export
│   ├── canlog-storage/       # SQLite catalog/index/cache/migrations
│   ├── canlog-adapters/      # 기존 DBC/CDD Engine 연결
│   ├── canlog-diagnostics/   # isotp/, uds/; 필요 시 별도 crate
│   └── canlog-cli/           # CLI 인자 → Application API
├── tests/fixtures/           # 출처·라이선스·checksum manifest 포함
├── benches/
└── fuzz/
```

논리 컴포넌트 `ASC`, `BLF`, `MDF`, `Filter`, `Query`, `Analysis`, `Export`, `ISO-TP`는 원안대로 유지한다. crate 수와 논리 책임은 같을 필요가 없다.

의존 방향은 `CLI → app → formats/storage/adapters/diagnostics → core`로 둔다. `core`는 SQLite, CLI, 기존 DBC/CDD Engine에 의존하지 않는다. 포맷 Reader는 SQLite 없이도 사용 가능해야 한다. `app`이 인덱스와 Reader를 연결한다. GUI/Python/MCP도 `app`을 호출한다.

기존 엔진은 Git revision 또는 배포 버전을 고정한 dependency로 사용한다. 경로 의존성은 실제 소스가 있는 경우에만 설정한다. `existing/`에 엔진이 있다고 가정하거나 새 DBC/CDD parser를 대신 만들지 않는다.

후보 feature는 `asc`, `blf`, `sqlite`, `dbc`, `diagnostics`, `mdf-read`, `parquet`이다. 실제 외부 엔진 의존성을 확인한 후 정의하며, 최소 `core + asc + CLI` 빌드와 지원 feature 조합을 CI에서 검사한다. MSRV, Edition, SQLite 제공 방식(bundled/system)은 최초 구현 시 고정한다.

---

## 5. Unified Core Model

### 5.1 CAN Frame의 불변 조건

`CanFrame`은 유효한 CAN 데이터/Remote frame을 표현한다. Error frame은 별도 `CanErrorRecord`로 두어 유효하지 않은 CAN ID나 DLC를 임의 생성하지 않는다.

```rust
// 제안 형태. 모든 필드는 private로 두고 validated constructor를 제공한다.
pub struct CanFrame {
    timestamp: Timestamp,
    channel: ChannelKey,
    id: CanId,
    direction: Direction,
    kind: CanFrameKind,          // Data | Remote
    format: CanFormat,           // Classic | Fd
    raw_dlc: u8,                 // 로그에 기록된 DLC 코드
    payload: CanPayload,         // len <= 64; SmallVec/inline buffer는 benchmark 후 결정
    fd_flags: Option<FdFlags>,   // BRS, ESI
    provenance: RecordRef,
}

pub enum CanId { Standard(u16), Extended(u32) }
pub enum Direction { Rx, Tx, Unknown }
```

- Standard ID는 `0..=0x7ff`, Extended ID는 `0..=0x1fff_ffff`만 허용한다. 숫자가 같아도 ID 종류가 다르면 다른 메시지다.
- CAN FD DLC `0..=15`의 길이 매핑은 `0,1,2,3,4,5,6,7,8,12,16,20,24,32,48,64`이다. DLC 코드와 실제 payload 길이를 혼용하지 않는다.
- Classic data payload는 최대 8바이트다. 원시 DLC `9..=15`가 존재하는 표현은 최대 8바이트 길이와 원시 코드를 구분해 보존하며, 포맷이 이를 구별하지 못하면 손실 보고한다.
- Remote frame은 Classic에서만 지원하며 payload는 비어 있다. DLC는 요청한 길이를 나타낸다.
- FD에서 Remote frame은 허용하지 않는다. Classic에 BRS/ESI를 설정하지 않는다.
- 포맷이 payload보다 긴 선언 길이, 잘린 payload 등을 기록한 경우 정상 frame으로 조용히 보정하지 않는다. 구조화된 문제와 원본 위치를 남긴다.
- Error record에는 timestamp/channel, 포맷별 error 정보와 선택적 원시 표현을 둔다. 모든 포맷의 error 의미를 하나의 CAN frame으로 축약하지 않는다.

### 5.2 시간·채널·원본 식별

```rust
pub struct Timestamp {
    pub clock: ClockId,
    pub offset_ns: i64,
}

pub struct ChannelKey { pub source: SourceId, pub local: u32 }
pub struct RecordRef { pub source: SourceId, pub ordinal: u64 }
```

`offset_ns`는 해당 clock origin 기준의 정수 나노초다. Clock metadata에는 원본 단위/해상도, 상대시간 기준, 선택적 UTC origin, 시간대의 근거, 정렬 상태를 저장한다. UTC origin은 큰 epoch 값과 상대 offset을 구분해서 저장한다. 로그 헤더의 로컬 시각에 시간대가 없으면 UTC를 추측하지 않는다.

- ASC의 absolute/relative 선언과 변종을 parser profile에서 해석한다. 원본 시간 표현을 보존할 필요가 있으면 별도 provenance metadata에 둔다.
- 정수 tick은 checked arithmetic으로 변환한다. MF4 등의 실수 시간은 유한값·범위 검사 후 명시한 반올림 규칙으로 ns로 변환하고 입력 해상도를 기록한다. ns 사용이 입력 정확도를 높이지는 않는다.
- 서로 다른 clock의 timestamp는 자동 비교하지 않는다. 명시적 origin/offset 또는 검증된 동기화 정보가 있어야 공통 timeline으로 변환한다. 드리프트 보정은 별도 선택 기능이다.
- 동일 clock에서도 입력은 역행하거나 같은 시간이 반복될 수 있다. `ordinal`은 원본 순서를 보존하며 시간 정렬을 보장하지 않는다.
- 채널 번호는 파일마다 의미가 다르다. 숫자 `1`만으로 여러 파일의 채널을 합치지 않는다. 포맷 채널 번호와 workspace 별칭을 매핑한다.
- 기간 필터는 기본 `[start, end)`이다. CLI 상대시간 기준은 선택된 입력 clock origin이며, UTC 필터는 별도 옵션으로 구분한다.

### 5.3 LogRecord와 SignalSample

```rust
pub enum LogRecord {
    Can(CanFrame),
    CanError(CanErrorRecord),
    Lin(LinFrame),
    Signal(SignalSample),
    Event(Event),
    Opaque(OpaqueRecord),
}

pub enum SignalValue {
    Signed(i64), Unsigned(u64), Float(f64), Bool(bool),
    Text(String), Bytes(Vec<u8>), Missing,
}
```

`SignalSample`에는 qualified signal key, timestamp, typed value, unit, validity/quality, 원본 record 참조를 둔다. MF4 raw value와 conversion 적용 physical value, DBC raw와 physical 값을 구분한다. 동일한 signal 이름을 채널·DB·메시지 구분 없이 key로 쓰지 않는다.

`Opaque`는 Reader가 경계를 확인한 미지원 데이터를 의미한다. 모든 포맷에서 재출력이 가능한 것은 아니며 크기 제한을 적용한다. ParseIssue는 정상 LogRecord와 별도로 전달한다.

---

## 6. Reader / Writer 추상화

### 6.1 순차 Reader와 seek capability

```rust
pub enum ReadItem {
    Record(LogRecord),
    Issue(ParseIssue),
}

pub trait LogReader {
    fn metadata(&self) -> &LogMetadata;
    fn capabilities(&self) -> ReaderCapabilities;
    fn next_item(&mut self) -> Result<Option<ReadItem>, CanLogError>;
    fn report(&self) -> &ReadReport;
}

pub trait IndexedReader: LogReader {
    fn seek_to(&mut self, anchor: &SeekAnchor) -> Result<(), CanLogError>;
}
```

`Ok(None)`은 EOF, `Err`는 처리를 중단하는 오류다. Recover 가능한 문제는 `Issue`로 반환하므로 무한 Err iterator와 조용한 누락을 방지한다. 필요 시 이 API 위에 iterator adapter를 제공한다. Metadata는 헤더 정보와 스캔 후 확정된 정보를 구분하며 frame count가 미확정이면 `Unknown`으로 표시한다.

ASC는 `BufRead` 기반 순차 읽기, BLF는 컨테이너 순차 읽기를 제공한다. MF4는 block link 탐색 때문에 기본 `Read + Seek` 입력이 필요할 수 있다. stdin을 무조건 지원한다고 약속하지 않는다. 임시 파일로 spool하는 선택 경로에는 디스크 상한을 적용한다.

Capabilities에는 포맷 버전/profile, record 종류, seek 가능 여부, 입력 순서 특성을 둔다. 사용자가 선택한 미지원 기능은 처리 시작 전에 오류로 알린다.

### 6.2 Writer와 finalize

```rust
pub trait LogWriter {
    fn capabilities(&self) -> WriterCapabilities;
    fn write(&mut self, record: &LogRecord) -> Result<(), CanLogError>;
    fn finish(self: Box<Self>) -> Result<WriteReport, CanLogError>;
}
```

`finish`는 header patch, flush, 포맷 검증을 완료하며 중복 호출을 허용하지 않는다. Drop은 오류를 보고할 수 없으므로 성공 판정에 사용하지 않는다. seek 기반 Writer와 streamable Writer를 구분한다. CLI 파일 출력은 임시 파일 → finish → 요청 시 sync → 원자적 게시를 담당하는 Application 계층으로 감싼다.

미지원 record/field 처리 규칙은 write 전에 결정한다. 기본은 손실 거부이며 명시적으로 허용한 손실만 `WriteReport`에 집계한다. `Csv/Jsonl/Parquet`는 typed export API를 사용하며, 이들을 원본 포맷 round-trip Writer와 동일한 의미로 취급하지 않는다.

### 6.3 포맷 감지

명시적 `--input-format` → magic/header 확인 → 확장자 hint → 구조 검증 순서로 처리한다. 확장자만 믿지 않는다. ASC처럼 magic이 약한 텍스트 포맷은 header/token probe가 필요하며 모호하면 명시적 format을 요구한다. Reader 선택과 포맷 버전 지원 판정은 별개다.

---

## 7. ASC Engine

ASC는 line-oriented streaming parser로 구현한다.

```text
ASC File
   │
   ▼
BufReader
   │
   ▼
Tokenizer
   │
   ├── Header
   ├── Classic CAN
   ├── CAN FD
   ├── Error
   └── Event
   │
   ▼
LogRecord
```

대용량 ASC를 전체 메모리에 올리지 않는다. Regex에 과도하게 의존하기보다 tokenizer/state parser를 권장한다.

---

## 8. BLF Engine

BLF는 다음 계층으로 분리한다.

```text
BLF File
   │
   ├── File Header
   │
   └── Log Container
           │
           ▼
       Decompress
           │
           ▼
        Objects
           │
    ┌──────┼─────────┐
    ▼      ▼         ▼
   CAN   CAN FD     Error
```

권장 모듈:

```text
canlog-blf/
├── header.rs
├── container.rs
├── object.rs
├── can.rs
├── canfd.rs
├── error.rs
├── reader.rs
└── writer.rs
```

### Streaming 원칙

```text
File
 ↓
BufReader
 ↓
Container
 ↓
Decompress
 ↓
Object Iterator
 ↓
CanFrame
 ↓
Filter
 ↓
Writer
```

메모리 사용량을 파일 크기가 아니라 설정된 Container/Batch 상한에 제한한다. container 선언 크기가 곧 허용 allocation 크기는 아니다. object carry를 포함한 총 budget을 적용한다.

BLF header/object version, object length/alignment, timestamp unit flag, 압축method를 검증한다. 대응 CAN/FD object variant를 명시한다. 알려진 object header로 안전하게 skip할 수 있는 미지원 type은 issue policy에 따라 처리한다. Writer는 지원 profile의 objects만 생성하며 header count/size/time 정보를 finish 시 확정한다. 실제 지원 producer/판본은 fixture로 확인한다.

---

## 9. MF4 / MDF4 Engine

MF4는 ASC/BLF와 별도의 모델로 접근한다.

```text
MDF4
 ├── Data Group
 ├── Channel Group
 ├── Channel
 ├── Conversion
 ├── Data / Compressed Data
 ├── Events
 └── Bus Logging
```

Low-level block parser와 High-level measurement API를 분리한다.

```text
CLI / Query
     │
     ▼
MDF High-Level API
     │
     ▼
MDF Domain Model
     │
     ▼
Block Parser
```

초기 구현 우선순위는 다음과 같이 한다.

1. MF4 metadata/channel 탐색
2. Measurement read
3. CAN Bus Logging extraction
4. Filtering/Cut
5. MF4 Writer

MF4 Writer는 Reader 및 Bus Logging extraction이 안정화된 이후 구현한다.

각 지원 MDF4 판본과 block/conversion/compression profile을 capabilities에 공개한다. metadata 탐색만 가능하면 measurement/bus extraction 지원으로 표시하지 않는다. 모든 MF4에 raw CAN bus record가 있다고 가정하지 않는다.

첫 profile은 실제 fixture가 있는 scalar measurement와 raw CAN bus logging으로 좁힌다. master channel/timebase, raw/physical conversion, invalidation, endianness/bit offset, sorted/unsorted records, data list/link와 압축 구성에 대한 지원/거부를 명시한다. array, VLSD, attachment, LIN 등은 후속 profile로 확장할 수 있다.

link cycle, block 범위·겹침·길이, record size overflow를 검증한다. metadata 크기와 link 탐색도 제한한다. selected channel read는 관련 master와 data를 함께 읽으며 전채널 적재를 전제로 하지 않는다. 서로 다른 group의 sample을 하나의 정렬 stream으로 만들 때에는 clock 확인과 bounded merge/reorder가 필요하다.

---

## 10. 기존 DBC Engine 연동 — 실제 engine API 기준

### 10.1 확인한 엔진과 재사용 범위

- 저장소: [najari/candb-csharp-clone](https://github.com/najari/candb-csharp-clone), 확인 commit `da64ad9ccf10237fce0993d1b83f460416d3da53`.
- 사용 package: **`candb-engine` 0.1.0**, 경로 `engine/`, Edition 2024. Rust library가 공개되어 있다.
- 사용 영역: `parser::Document`, `model::{Database, Message, Signal}`, `codec::{Codec, CompiledMessage, Scratch}`. C# GUI와 editor의 JSON request/undo/project 기능은 로그 처리 경로에 넣지 않는다.
- parser와 codec을 그대로 사용한다. canlog에 DBC lexer, bit decode, multiplex evaluator를 복제하지 않는다.

```text
DBC bytes
 → Document::from_bytes(bytes, encoding_hint)
 → parse warnings / Database validation / assignment resolution
 → Message별 CompiledMessage::compile(message)
 → CanFrame payload + worker별 Scratch
 → engine decode result
 → canlog의 typed SignalSample + quality/provenance
```

### 10.2 ID와 길이의 정확한 매핑

engine의 `Message.raw_id`와 `Codec::message`는 **bit 31이 Extended 표시인 DBC raw ID**를 사용한다. 반면 canlog의 `CanId`는 bus ID와 ID 종류를 분리한다.

```rust
// CanId 자체는 앞서 검증된 값이어야 한다.
fn dbc_raw_id(id: CanId) -> u32 {
    match id {
        CanId::Standard(v) => u32::from(v),
        CanId::Extended(v) => v | 0x8000_0000,
    }
}
```

engine의 `Message.dlc`는 codec에서 **기대 payload byte 수**로 사용된다. CAN FD의 4-bit DLC 코드를 그대로 넘기지 않는다. `CompiledMessage::decode`와 `decode_physical`은 입력 byte 수가 이 길이와 다르면 오류를 반환한다. canlog adapter는 임의 padding/truncation 없이 이 실패를 전달한다. Remote/Error record는 DBC payload decode에 넣지 않는다.

Database는 `frame_format`과 `VFrameFormat` 해석을 제공한다. canlog core의 format/ID enum으로 변환하는 작업은 adapter에 둔다. engine에 큰 J1939 payload를 다루는 경로가 있다는 이유로 이를 단일 CAN frame에 허용하지 않는다. J1939 transport 분석은 별도 후속 기능이다.

### 10.3 두 decode 경로

| engine API | 실제 결과 | canlog 활용 |
|---|---|---|
| `CompiledMessage::decode(data, scratch)` | `Vec<serde_json::Value>`; name/raw/physical/unit/description. raw는 문자열, signal 오류는 row로 표현 | raw 정수·오류·enum 설명을 보존하는 기본 정확성 경로 |
| `CompiledMessage::decode_physical(data, scratch, out)` | 재사용 가능한 `(signal index, f64)` 목록 | physical 숫자만 필요한 plot/aggregation의 선택 경로 |
| `Codec::new(&Database)` / `Codec::decode` | ID lookup, lazy compilation, mutable scratch 관리 | 단일 worker의 초기 구현 경로 |

`decode_physical`은 비활성 multiplex signal과 decode 불가능한 signal을 출력하지 않는다. 따라서 누락만 보고 inactive/error를 구분할 수 없으며 모든 원시 정수의 정밀도를 보장하지 않는다. exact raw export, 품질 보고, validation의 기본 경로를 이 API 하나로 대체하지 않는다. NaN/Infinity도 canlog quality/serialization 정책에 맞게 처리한다.

기본 adapter는 JSON row를 domain typed result로 변환한다. 정수 signal의 raw 문자열은 원래 signal의 signedness를 이용해 i64/u64로 읽는다. Float signal은 원시 bit와 물리값을 구분한다. engine의 physical 값은 f64 연산 결과이므로 exact decimal이라고 표현하지 않는다.

JSON row 경로에서 duplicate signal name이 존재하면 key 충돌을 피하도록 definition ordinal을 포함하거나 DB assignment를 거부한다. 정상 정의와 결과 row의 대응을 테스트로 보장하고, ambiguity를 조용히 해결하지 않는다.

후속 성능 개선으로 engine에 `typed decode + per-signal status` API를 추가하는 것이 바람직하다. **이것은 외부 엔진의 별도 변경 제안**이며 현재 canlog 설계 보완에서 구현하지 않는다. 그 전에도 기존 decode API로 정확성 경로를 만들 수 있다.

### 10.4 Cache와 worker ownership

원안의 frame별 DBC lookup/parsing을 피한다는 원칙은 유지하되 engine이 이미 precompiled codec을 제공하므로 그 기능을 다시 만들지 않는다. 동일 raw ID의 duplicate message는 `Codec::new`가 첫 항목을 선택하므로 adapter가 사전에 검출하고 binding 오류로 보고한다.

adapter 준비 시 DB revision/channel/ID별 immutable compiled definitions를 만들고 worker별 `Scratch`를 둔다. `CompiledMessage`의 Send/Sync trait는 연동 테스트에서 확인한 뒤 공유한다. GUI용 `Engine` 전체나 SQLite Project를 decode worker 사이에서 공유하지 않는다. borrowed `Codec<'a>`를 자기참조 struct로 억지로 넣지 말고, batch 동안 DB를 borrow하거나 owned compiled handle 구조를 사용한다.

engine의 `Engine::source_hash()`는 DefaultHasher 기반 UI fingerprint다. 장기 cache identity로 그대로 쓰지 않는다. adapter가 DBC 원본 bytes의 SHA-256과 실제 engine commit/profile을 별도로 저장한다.

### 10.5 확인 수준과 integration gate

소스에 compiled/reference decode 비교, encode/decode, multiplex, FD/wide signal, physical path 테스트가 존재한다. 이번 보완에서는 이를 읽었으며 실행하지 않았다. canlog adapter 완료 gate는 해당 engine tests와 실제 adapter fixtures를 같은 pinned revision에서 실행하는 것이다. 특히 Standard/Extended 동일 숫자, FD DLC/byte 길이, short payload, invalid signal, duplicate definition, u64 raw 보존을 포함한다.

---

## 11. 기존 CDD Engine 연동 — 개발 중인 실제 API 기준

### 11.1 Facade와 maturity

- 저장소: [najari/cdd-rust-engine](https://github.com/najari/cdd-rust-engine), 확인 commit `0bd598b645814b178f9a4b313caba26cf873833e`.
- 사용할 facade: **`cdd-api` 0.0.1의 `CddEngine`**. Rust 연동에서는 cdd-cli subprocess나 C FFI를 우선하지 않는다.
- 내부 계층: cdd-xml → cdd-core → cdd-codec → cdd-api, 별도 cdd-store/index. canlog은 이 구조를 재구현하지 않는다.
- 현재 `CddEngine::open`은 `OpenOptions.allow_experimental = true`가 아니면 `ProfileRequired`로 실패한다. profile은 `uds-candela-2x`, maturity는 **Experimental**이다. canlog 설정에 명시적 opt-in을 두며 기본값으로 몰래 활성화하지 않는다.

현재 소스에는 identify/decode/encode, typed field와 request context, NRC, index/search, bit/container/state/DTC 관련 코드·테스트가 있다. 이 사실이 모든 제조사 CDD와 UDS 서비스 지원 또는 실차 검증 완료를 의미하지 않는다. profile maturity와 load issues를 결과·export·validation report에 그대로 남긴다. 수치 PASS와 profile 검증 수준은 별도 축으로 표시한다.

### 11.2 실제 호출 흐름

```text
CDD bytes + experimental opt-in
 → CddEngine::open(bytes, &OpenOptions)
 → load_issues / fingerprint
 → select_ecu → select_variant → context

완료된 요청 ISO-TP payload
 → identify(context, Direction::Request, payload)
 → Unique인 경우 decode_message(service, Request, ...)
 → request_context(service, request_payload)

시간/connection 기준으로 매칭된 응답 payload
 → decode_response(&RequestContext, response_payload, &DecodeOptions)
 → canlog typed diagnostic fields + provenance/issues
```

`identify`는 payload가 어떤 CDD message definition인지 판정한다. **로그상 어떤 요청에 대한 응답인지 연결하는 Transaction Matcher와는 책임이 다르다.** ISO-TP와 시간/connection 기반 request-response pairing은 canlog에 둔다. service/DID별 데이터 구조와 meaning decode는 CDD Engine에 맡긴다.

`IdentifyResult`의 `Unique`, `NoMatch`, `Ambiguous`, `Unverified`를 보존한다. `decode_identified`는 unique한 verified match에만 성공하므로 adapter가 ambiguity를 첫 후보로 바꾸지 않는다. variant가 미정이면 `identify_across_variants`로 후보를 탐색할 수 있지만 자동 확정은 하지 않는다. ECU/variant assignment는 workspace에 저장한다.

원안의 `service(sid)`, `did(u16)`, `decode_response(data)`는 실제 엔진 연동 계약으로 부족하다. 엔진의 context/key와 request-aware response decode를 사용한다. 동일 SID/DID라도 ECU/variant/service definition에 따라 의미가 달라질 수 있다.

### 11.3 Context와 cache

`RequestContext`는 document fingerprint, context/service/request key, request payload, response-layout fingerprint를 가진다. JSON serialization API가 있으며 process-local SnapshotId는 직렬화하지 않는다. canlog persistence에서는 raw arena index를 영구 ID로 쓰지 않는다.

`DocumentFingerprint`의 원본 SHA-256, profile fingerprint, parser semver를 cache key에 포함하고, 개발 중 같은 semver의 변경을 구별하도록 pinned commit도 포함한다. 현재 response context 해석은 key와 response layout을 확인하지만 canlog의 원본 DB 동일성 규칙을 대신 보장한다고 가정하지 않는다. 다른 DB revision으로 context를 재사용하려면 명시적인 재바인딩/호환성 검증이 필요하다.

CDD Engine의 definition/search index는 engine 소유의 별도 cache directory를 사용한다. canlog workspace.db는 assignment, transport sessions, transactions, analysis 결과를 관리한다. engine의 XML/model/search schema를 workspace DB에 복제하거나 직접 수정하지 않는다. cache directory와 runtime index handles도 소유권을 분리한다.

### 11.4 결과와 오류의 변환

`DecodedField`에는 key/raw/physical/label/unit/wire spans가 있다. adapter가 이를 canlog typed field로 매핑하고 `DecodedMessage.diagnostics/provenance`를 보존한다. 전체 decode 성공과 field validity를 혼동하지 않는다.

`DecodeOptions`의 trailing bytes 정책과 reserved bits 설정을 canlog 설정·cache key에 포함한다. 기본 trailing 정책은 Error이며 CDD layout이 맞지 않는 payload를 조용히 잘라서 성공시키지 않는다. unknown/unsupported/malformed payload는 raw와 engine error code/issues를 남기고 정상 decode와 구분한다.

session/security의 현재 상태가 캡처로 확정되지 않으면 unknown이다. engine의 declared service conditions를 읽을 수 있다는 이유로 실제 ECU의 state를 추측하지 않는다. `0x78` 누적, suppress-positive-response, 다중 응답은 canlog transaction 계층에서 평가한다.

### 11.5 Toolchain과 남은 검증

현재 repo는 `rust-toolchain.toml`과 workspace rust-version에 **1.98.1**을 지정한다. canlog와 통합할 compiler가 해당 pin을 만족하고 실제 배포되어 있는지 확인해야 한다. Edition 2024인 DBC와 Edition 2021인 CDD는 함께 dependency로 사용할 수 있지만, MSRV·native SQLite dependency compatibility는 실제 Cargo build로 검증한다. 확인한 manifest에서는 두 engine이 모두 rusqlite 0.40/bundled를 사용한다. canlog도 이 조합에 맞춰 시작하되 lockfile에서 libsqlite3-sys 버전과 `links` 충돌 여부를 실제 build로 확인한다.

이번 작업은 소스/API/테스트의 정적 검토다. 현재 cloud machine에는 Rust toolchain이 없어 engine build/test는 실행하지 않았다. 설계 검토를 위한 임의 toolchain 설치·코드 수정은 하지 않았다. Integration gate에서 pinned compiler·실제 CDD samples로 request context round-trip, positive/negative, ambiguity, profile guard, cache mismatch, unsupported layout을 실행 검증한다.

---

## 12. ISO-TP 계층

ISO-TP 계층은 **오프라인 수동 분석기**다. CAN을 송신하거나 FC를 생성하지 않는다. DBC decode와 독립적으로 원본 CAN stream을 입력받는다.

### 12.1 연결 설정과 상태

Connection은 source/channel, physical/functional addressing, request/response ID 종류와 값, addressing mode(normal/extended/mixed), 필요 시 address byte로 식별한다. 단순 `(request_id, response_id)`만으로 모든 연결을 구분하지 않는다. CAN FD와 PCI의 지원 조합도 profile에 선언한다.

방향별 reassembly 상태는 `Idle → Receiving → Complete`이며 `Incomplete/Aborted/ProtocolViolation`을 별도로 기록한다. SF/FF/CF/FC, CF sequence의 modulo-16 증가, 중복/누락, 새 FF로 인한 중단, 마지막 CF의 선언 길이와 padding 처리를 정의한다. 완성된 payload만 CDD 기본 decode에 전달한다.

- 타이머는 로그 timestamp로 평가한다. 실행 중 wall clock을 쓰지 않는다.
- EOF에서 열린 세션을 incomplete로 flush한다. 역행 시간과 ParseIssue gap은 품질 상태에 반영한다.
- 실제 캡처 유실과 송신 측 위반을 구분할 수 없으면 원인을 단정하지 않는다.
- FF 길이, 열린 연결 수, 세션별 buffer, 총 buffer, timeout 상한을 설정한다. 과도한 선언 길이는 allocation 전에 거부한다.
- FF/CF의 FD 확장, SF escape length, 긴 FF length, 주소 모드는 지원되는 ISO-TP 판본/profile과 fixture가 있을 때 활성화한다. Classic용 길이 규칙을 FD에 그대로 적용하지 않는다.

### 12.2 FC, BS, STmin 분석

FC에는 Continue/Wait/Overflow 상태가 있다. 관측된 FC에 대해서만 BS/STmin 및 delay를 계산한다. STmin은 유효한 ms/100µs 표현과 reserved 값을 구분한다. FC 방향은 데이터 전송 방향의 반대이며 address 설정을 반영한다.

FC가 캡처되지 않았다는 이유만으로 payload 조립을 항상 실패시키지 않는다. `fc_observation=missing`으로 남기고 가능한 payload를 복원하되 프로토콜 준수 판정은 `unknown`으로 둔다. 관측 timestamp 해상도보다 작은 시간 위반은 확정하지 않는다.

Result에는 connection key, payload length, first/last frame time, 관련 record refs, FC 관측, sequence issue, completeness, capture gap을 둔다. 선택적 ID heuristic은 후보 발견 기능이며 확정 assignment로 저장하지 않는다.

시간/ID prefilter로 필요한 CF나 FC를 제거하지 않는다. 최종 결과 window와 별도로 bounded context를 읽고 세션 조립 후 결과를 필터링한다. context 시작 전에 FF가 있으면 incomplete 결과를 허용해야 한다.

---

## 13. UDS Transaction Engine

ISO-TP 완료 payload → 최소 UDS envelope 분류 → Transaction Matcher → 선택적 CDD decode 순서로 처리한다. CDD가 없어도 raw payload, SID, negative response, 응답 상태를 분석할 수 있다. 제조사 데이터 의미는 CDD adapter에 맡긴다.

### 13.1 요청과 응답 연결

- 매칭 key는 connection, ECU 대상, service별 discriminator(DID/subfunction/routine/block sequence 등), 시간 window를 포함한다.
- `response SID = request SID + 0x40`만으로 매칭하지 않는다. `0x7f, request SID, NRC` 구조와 service별 echo 필드를 확인한다.
- `0x22`는 여러 DID를 한 요청에 포함할 수 있다. `0x19` 등 가변 구조를 단일 `identifier` 열로 축약하지 않는다.
- `0x78 ResponsePending`은 중간 응답으로 누적하고 configured P2* 관측 window를 적용한다. 같은 요청의 최종 응답을 계속 찾는다. P2/P2*는 설정 또는 근거가 확인된 데이터에서 가져오며 무제한 연장하지 않는다.
- suppress-positive-response가 적용되는 서비스는 부재를 무조건 실패로 판단하지 않는다. subfunction flag를 적용 가능한 서비스에서만 해석한다.
- functional request는 여러 ECU 응답을 받을 수 있어 request 1개에 response N개 구조가 필요하다. 응답이 없는 경우에도 ECU 실패를 단정하지 않는다.
- outstanding 요청, 반복 요청, 관측되지 않은 요청, 애매한 후보는 `ambiguous/orphan`으로 남긴다. 임의로 가장 최근 요청에 붙이지 않는다.

### 13.2 결과와 latency

Transaction 결과는 `Positive`, `Negative`, `PendingOnly`, `NoResponseObserved`, `SuppressedExpected`, `Ambiguous`, `Incomplete`로 구분한다. 분석 window와 capture 품질도 기록한다. 로그만으로 ECU 통신 장애를 확정하는 상태명은 사용하지 않는다.

Latency는 요청 payload 마지막 frame → 응답 payload 첫 frame 시간을 기본으로 하고, 요청 시작 → 최종 응답 완료 시간도 별도 저장한다. 단일 `duration_ns`의 기준을 숨기지 않는다. 음수 latency는 timestamp 문제로 보고한다.

초기 서비스 범위: `0x10/11/14/19/22/27/2E/31/34/36/37`의 envelope/matching 및 제공된 CDD definition의 decode. SID 목록이 곧 모든 서비스 변종과 제조사 확장을 지원한다는 의미는 아니다. 지원 service profile과 fixture coverage를 공개한다.

---

## 14. SQLite의 역할

SQLite는 다음 데이터를 담당한다.

```text
SQLite Workspace
│
├── File Catalog
├── Channel Metadata
├── CAN ID Statistics
├── Time/Container Index
├── DBC/CDD Assignment
├── Lazy Signal Cache
├── ISO-TP Session
├── UDS Transaction
└── Analysis Result
```

원칙적으로 대용량 Raw Frame 전체를 SQLite에 복제하지 않는다.

```text
30 GB vehicle.blf
        │
        ├── Raw Data ─────► vehicle.blf
        │
        └── Metadata/Index ► workspace.db
```

---

## 15. SQLite 주요 Schema와 일관성

다음은 v1 기반 schema의 실행 가능한 초안이다. Signal definition, typed payload detail, MDF seek anchor 등은 독립 migration으로 확장한다. `source_revision_id`를 통해 원본 content revision과 분석 결과를 연결한다.

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE log_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    format TEXT NOT NULL
);

CREATE TABLE source_revisions (
    id INTEGER PRIMARY KEY,
    log_file_id INTEGER NOT NULL REFERENCES log_files(id) ON DELETE CASCADE,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
    mtime_ns INTEGER,
    content_hash TEXT,
    fingerprint_kind TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    UNIQUE(log_file_id, id)
);

CREATE TABLE clocks (
    id INTEGER PRIMARY KEY,
    source_revision_id INTEGER NOT NULL REFERENCES source_revisions(id) ON DELETE CASCADE,
    local_clock_key TEXT NOT NULL,
    origin_utc_seconds INTEGER,
    origin_subsecond_ns INTEGER CHECK(origin_subsecond_ns BETWEEN 0 AND 999999999),
    resolution_ns INTEGER CHECK(resolution_ns > 0),
    ordering TEXT NOT NULL CHECK(ordering IN ('unknown','nondecreasing','unordered')),
    UNIQUE(source_revision_id, local_clock_key),
    UNIQUE(source_revision_id, id),
    CHECK((origin_utc_seconds IS NULL) = (origin_subsecond_ns IS NULL))
);

CREATE TABLE channels (
    id INTEGER PRIMARY KEY,
    source_revision_id INTEGER NOT NULL REFERENCES source_revisions(id) ON DELETE CASCADE,
    local_index INTEGER NOT NULL CHECK(local_index >= 0),
    name TEXT,
    bus_type TEXT NOT NULL,
    UNIQUE(source_revision_id, local_index),
    UNIQUE(source_revision_id, id)
);

CREATE TABLE can_messages (
    source_revision_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    clock_id INTEGER NOT NULL,
    can_id INTEGER NOT NULL,
    extended INTEGER NOT NULL CHECK(extended IN (0,1)),
    frame_count INTEGER NOT NULL CHECK(frame_count >= 0),
    first_time_ns INTEGER NOT NULL,
    last_time_ns INTEGER NOT NULL,
    PRIMARY KEY(source_revision_id, channel_id, clock_id, can_id, extended),
    FOREIGN KEY(source_revision_id, channel_id) REFERENCES channels(source_revision_id, id) ON DELETE CASCADE,
    FOREIGN KEY(source_revision_id, clock_id) REFERENCES clocks(source_revision_id, id) ON DELETE CASCADE,
    CHECK(can_id >= 0 AND ((extended = 0 AND can_id <= 2047) OR (extended = 1 AND can_id <= 536870911))),
    CHECK(first_time_ns <= last_time_ns)
);

CREATE TABLE blf_containers (
    id INTEGER PRIMARY KEY,
    source_revision_id INTEGER NOT NULL REFERENCES source_revisions(id) ON DELETE CASCADE,
    clock_id INTEGER NOT NULL,
    file_offset INTEGER NOT NULL CHECK(file_offset >= 0),
    compressed_size INTEGER NOT NULL CHECK(compressed_size >= 0),
    uncompressed_size INTEGER NOT NULL CHECK(uncompressed_size >= 0),
    anchor_file_offset INTEGER NOT NULL CHECK(anchor_file_offset >= 0),
    min_time_ns INTEGER,
    max_time_ns INTEGER,
    first_sequence INTEGER,
    last_sequence INTEGER,
    UNIQUE(source_revision_id, file_offset),
    FOREIGN KEY(source_revision_id, clock_id) REFERENCES clocks(source_revision_id, id) ON DELETE CASCADE,
    CHECK(anchor_file_offset <= file_offset),
    CHECK((min_time_ns IS NULL AND max_time_ns IS NULL) OR
          (min_time_ns IS NOT NULL AND max_time_ns IS NOT NULL AND min_time_ns <= max_time_ns)),
    CHECK((first_sequence IS NULL AND last_sequence IS NULL) OR
          (first_sequence IS NOT NULL AND last_sequence IS NOT NULL AND first_sequence <= last_sequence))
);
CREATE INDEX blf_time_idx ON blf_containers(source_revision_id, clock_id, min_time_ns, max_time_ns);

CREATE TABLE databases (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    db_type TEXT NOT NULL CHECK(db_type IN ('dbc','cdd')),
    content_hash TEXT NOT NULL,
    engine_version TEXT NOT NULL,
    UNIQUE(db_type, content_hash, engine_version)
);

CREATE TABLE log_database_map (
    source_revision_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    database_id INTEGER NOT NULL REFERENCES databases(id),
    role TEXT NOT NULL CHECK(role IN ('dbc','cdd')),
    binding_key TEXT NOT NULL, -- DBC 선택 규칙/ECU/profile의 정규화 key
    PRIMARY KEY(source_revision_id, channel_id, role, binding_key),
    FOREIGN KEY(source_revision_id, channel_id) REFERENCES channels(source_revision_id, id) ON DELETE CASCADE
);

CREATE TABLE cache_entries (
    id INTEGER PRIMARY KEY,
    source_revision_id INTEGER NOT NULL REFERENCES source_revisions(id) ON DELETE CASCADE,
    semantic_key TEXT NOT NULL, -- signal/DB hash/engine/settings/profile 포함
    UNIQUE(source_revision_id, semantic_key),
    UNIQUE(source_revision_id, id)
);

CREATE TABLE cache_coverage (
    cache_entry_id INTEGER NOT NULL REFERENCES cache_entries(id) ON DELETE CASCADE,
    clock_id INTEGER NOT NULL REFERENCES clocks(id) ON DELETE CASCADE,
    start_ns INTEGER NOT NULL,
    end_ns INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('complete','partial','building')),
    PRIMARY KEY(cache_entry_id, clock_id, start_ns, end_ns),
    CHECK(start_ns < end_ns)
);

CREATE TABLE signal_cache (
    cache_entry_id INTEGER NOT NULL REFERENCES cache_entries(id) ON DELETE CASCADE,
    clock_id INTEGER NOT NULL REFERENCES clocks(id) ON DELETE CASCADE,
    timestamp_ns INTEGER NOT NULL,
    source_ordinal INTEGER NOT NULL CHECK(source_ordinal >= 0),
    sample_ordinal INTEGER NOT NULL CHECK(sample_ordinal >= 0),
    value_kind TEXT NOT NULL CHECK(value_kind IN ('i64','u64','f64','bool','text','bytes','missing')),
    value_blob BLOB NOT NULL, -- versioned tagged codec; u64를 REAL로 변환하지 않음
    quality TEXT NOT NULL,
    PRIMARY KEY(cache_entry_id, source_ordinal, sample_ordinal)
);
CREATE INDEX signal_time_idx ON signal_cache(cache_entry_id, clock_id, timestamp_ns);

CREATE TABLE uds_transactions (
    id INTEGER PRIMARY KEY,
    source_revision_id INTEGER NOT NULL REFERENCES source_revisions(id) ON DELETE CASCADE,
    clock_id INTEGER NOT NULL,
    analysis_key TEXT NOT NULL, -- CDD/route/timeout/matcher version等
    connection_key TEXT NOT NULL,
    request_first_ns INTEGER,
    request_last_ns INTEGER,
    service_id INTEGER CHECK(service_id BETWEEN 0 AND 255),
    status TEXT NOT NULL,
    request_payload BLOB,
    FOREIGN KEY(source_revision_id, clock_id) REFERENCES clocks(source_revision_id, id) ON DELETE CASCADE
);

CREATE TABLE uds_responses (
    id INTEGER PRIMARY KEY,
    transaction_id INTEGER NOT NULL REFERENCES uds_transactions(id) ON DELETE CASCADE,
    responder_key TEXT NOT NULL,
    first_ns INTEGER NOT NULL,
    last_ns INTEGER NOT NULL,
    response_type TEXT NOT NULL,
    nrc INTEGER CHECK(nrc BETWEEN 0 AND 255),
    payload BLOB NOT NULL,
    CHECK(first_ns <= last_ns)
);
CREATE INDEX uds_time_idx ON uds_transactions(source_revision_id, clock_id, request_first_ns);
```

`first_time_ns/last_time_ns`는 관측 순서상의 첫/마지막 시각이 아니라 최소/최대 시각이다. 입력 시간이 역행해도 후보 검색에서 누락하지 않는다.

### Index 계약

- BLF object는 container 경계를 넘을 수 있다. Reader는 이전 container의 잔여 bytes를 carry해서 복원한다.
- container의 physical offset과 object의 logical offset은 구분한다. seek anchor는 carry 없이 재개할 수 있는 경계 또는 앞쪽에서 다시 읽을 시작점이다. 임의 container의 단독 해제로 모든 object를 읽을 수 있다고 가정하지 않는다.
- min/max time은 후보를 선택하는 거친 인덱스다. frame filter가 최종 판정하고 미확정 구간은 안전하게 스캔한다.
- CAN ID 통계만으로 ID별 container 위치를 알 수 없다. v1은 시간 후보를 읽어 ID로 필터링한다. 추가 posting/bloom filter를 도입하면 잘못된 제외가 없도록 검증한다.
- ASC anchor는 행 경계 offset과 header/parser state, MF4 anchor는 DG/CG/data block/record 위치를 포함한다. 공통 timestamp만으로 seek를 재현할 수는 없다.

### Cache·원본 변경·DB 운영

Cache key에는 원본 revision, DB content hash, channel/ECU assignment, engine/decoder 버전, signal identity, decode 설정을 포함한다. mtime/size는 저렴한 변경 감지 수단이며 content identity가 아니다. 빠른 fingerprint를 쓰면 보장 수준을 표시하고, 정확한 동일성이 필요한 작업에서는 full hash를 확인한다.

같은 path의 파일이 바뀌면 새 revision을 만들고 과거 index/cache를 현행 결과에 섞지 않는다. 스캔 전후 stat을 비교해 처리 중 변경을 감지하면 complete로 확정하지 않는다. 변경 중인 로그를 tail하는 기능은 후기에 명시적으로 추가한다.

Coverage에는 `[start,end)`, clock, 성공 상태를 저장한다. 결과가 비어도 complete coverage는 기록할 수 있다. rows가 있다는 이유만으로 전구간 cache가 완성됐다고 판단하지 않는다. 중단된 building/partial을 자동 승격하지 않고, signal rows와 대응 coverage를 같은 transaction에서 확정한다.

SQLite 연결마다 FK를 활성화한다. local writable DB에서는 WAL/busy timeout, 단일 writer queue, bounded batch transaction을 기본으로 한다. network filesystem/read-only에서는 WAL을 무조건 요구하지 않는다. schema version, migration, backup/checkpoint 절차를 정의하고 오래된 binary가 새로운 schema에 쓰지 못하게 한다.

cache sample의 clock이 cache entry의 원본 revision에 속하는지, assignment role과 DB type이 일치하는지, semantic key가 정규화됐는지는 Application 계층에서 검증한다. 필요하면 후속 migration에서 composite FK/trigger로 강화한다. 위 SQL만으로 모든 domain 규칙을 보장한다고 주장하지 않는다.

Cache에는 size quota/LRU 등 삭제 정책을 둔다. 재생성 가능한 cache를 삭제할 때 assignment/rule/catalog를 지우지 않는다. 진단 raw payload를 영구 저장하면 저장 범위·삭제 정책을 별도로 제공한다.

---

## 16. Storage Mode

mode는 저장 기능을 허용하는 범위이지 import 시 모든 cache/analysis를 사전 생성한다는 의미가 아니다. `INDEXED`도 signal decode와 diagnostics는 요청 시 계산한다. `FULL`은 size quota와 명시 선택이 필요하며 원본 보존의 대체물이 아니다.


### LIGHT

```text
Metadata
Channels
CAN ID Statistics
BLF/MF4 Container Metadata
DBC/CDD Assignment
```

### INDEXED — 기본 권장

```text
LIGHT
+
Time/Container Index
Signal Cache
UDS Transaction
Analysis Result
```

### FULL — 선택 기능

```text
INDEXED
+
Raw CAN Frames
+
선택/전체 Decoded Signals
```

`FULL`은 작은 로그, CI, 테스트 자동화 등에 제한적으로 사용한다.

---

## 17. Filter Engine

`matches(record) -> bool`은 raw record predicate에 한정한다. Signal decode와 diagnostics는 상태, 의존 관계, 실패를 가지므로 별도 실행 단계로 처리한다.

```text
Source selection / 시간 후보 seek
 → 안전한 raw predicate pushdown
 → 필요한 DBC decode / ISO-TP reassembly / UDS matching
 → derived predicate
 → projection / aggregation / export
```

CAN ID/종류, channel, 방향, Classic/FD, data pattern, 시간은 raw 조건이다. Signal, DID, NRC, routine은 derived 조건이다. Unknown direction을 Rx/Tx에 일치시키지 않는다. Data pattern은 payload 길이가 부족한 경우의 판정을 정의한다.

ISO-TP 입력에서 CF/FC를 제거하거나 signal join에 필요한 message를 제외하는 최적화는 하지 않는다. planner가 의존 message/connection과 context window를 결정한다. 결과가 frame, signal row, transaction 중 무엇인지 command/schema에 명시한다.

```bash
canlog filter input.blf --id 0x100-0x1FF --channel 1 --start 10s --end 30s -o filtered.blf
```

ID 종류를 생략하면 Standard/Extended의 해당 숫자 범위에 모두 일치시키는 방안을 기본으로 한다. 구별하려면 `--id-kind standard|extended`를 사용한다. 최종 CLI 구현 시 이 동작을 help와 테스트로 고정한다.

---

## 18. Expression / Query Engine

식은 임의 SQL/Rust/Python 코드로 실행하지 않고 작은 typed AST로 parse한다. 식별자, 비교, 논리, 선택한 시간 연산을 단계적으로 추가한다. 식 깊이, token 수, 결과 행 수, join window에 상한을 둔다.

### 이름 해석과 결측

짧은 signal 이름은 한 가지 definition으로 해석될 때만 허용한다. 모호하면 channel/database/message/signal의 qualified key를 요구한다. 필요한 unit/type/DB assignment가 없으면 실행 전에 오류로 알린다.

Missing/Invalid/NaN은 unknown을 포함하는 삼값 논리로 다룬다. 일반 where는 true만 채택하며 `is_missing` 등으로 결측을 선택할 수 있게 한다. raw/physical 비교를 구분하고 다른 단위는 변환 관계가 정의된 경우에만 비교한다.

### Signal 간 시간 정렬

다른 message의 signal은 동시에 갱신되지 않는다. `VehicleSpeed > 100 && BatteryVoltage < 11.5`에 무기한 latest 값을 암묵적으로 사용하지 않는다. v1은 같은 message 안의 조건과 단일 signal stream부터 지원한다.

cross-message query에는 evaluation 기준(signal 업데이트/grid/event), join 방식(exact/previous/nearest), 최대 sample age, 미래 sample 허용 여부를 명시한다. 선형 보간은 연속량에 한해 선택적으로 사용하고 enum/boolean/diagnostic에는 적용하지 않는다. 결합한 sample의 실제 시각과 age를 출력한다.

### Temporal Query

공통 clock으로 대응할 수 있는 입력만 결합한다. ±window의 경계 포함 여부, 복수 후보, tie, join cardinality를 정의한다. unordered 입력에는 bounded reorder 또는 external sort를 명시적으로 선택한다. 유한 reorder window로 보장할 수 없는 입력은 거부하거나 품질 저하를 보고한다.

결과에는 source refs, clock mapping, DB/engine 버전, query text/AST version, 결측·제외·복구 건수를 포함한다. 같은 원본 revision과 설정에서 재현 가능한 결과를 만든다.

---

## 19. DBC + CDD Cross Validation

비교에는 signal/DID의 완전한 식별, ECU/connection, 단위와 시간 정렬이 필요하다. 진단 DID의 실제 측정 시각은 응답 수신 시각과 다를 수 있다. ECU 시각을 모르면 응답 시작/완료 시각을 proxy timestamp로 사용했다고 기록한다.

```yaml
schema_version: 1
name: HV Voltage Cross Check
diagnostic:
  connection: powertrain_ecu
  service: 0x22
  did: 0x4901
  field: voltage
  timestamp: response_first_frame
signal:
  database: EPICD
  channel: CAN1
  message: HVBatteryStatus
  name: HV_BatteryVoltage
alignment:
  method: previous
  max_age_ms: 100
units:
  target: V
tolerance:
  absolute: 1.0
  relative: 0.0
```

판정식은 `abs(a-b) <= absolute + relative * abs(reference)`이고 이 예시의 reference는 DBC 값이다. sample 부재, invalid decode, 단위 불명, clock 대응 불가는 INCONCLUSIVE이며 PASS가 아니다. Report에 PASS/FAIL/INCONCLUSIVE 건수, sample 시각/age, 차이, source refs를 넣는다. 임계값은 차량·신호 사양으로 정하며 예시 수치를 일반적인 정답으로 사용하지 않는다.

```bash
canlog validate drive.blf --workspace EPICD --rule voltage-check.yaml
```

---

## 20. CLI Command Specification

```text
canlog
├── info
├── view
├── channels
├── convert
├── filter
├── cut
├── merge
├── decode
├── diag
├── extract
├── export
├── query
├── index
├── stats
├── validate
├── verify
└── workspace
```

각 command는 지원 phase/profile에서만 활성화한다. `info`는 헤더만으로 아는 값과 `--scan`으로 확정한 통계를 구분한다. header의 count를 실측값처럼 출력하지 않는다. `view/query`는 default limit을 명시하고, unlimited output은 명시 옵션으로 선택한다.

대용량 결과는 stdout, progress/warnings는 stderr에 쓴다. 공통 옵션 후보는 `--input-format`, `--workspace`, `--strict|--recover`, `--unsupported error|skip`, `--loss-policy reject|allow`, `--report <path>`, `--memory-budget`, `--jobs`, `--limit`, `--overwrite`이다. 최종 인자 이름은 구현 시 고정한다.

### info

```bash
canlog info vehicle.blf
```

예상 출력:

```text
Format          BLF
File Size       1.42 GB
Start           14:23:01.102
End             14:31:52.431
Duration        8m 51.329s
CAN FD          yes
Channels        3
Frames          2,416,546
```

### convert

```bash
canlog convert input.asc output.blf
canlog convert input.blf output.asc
canlog convert input.blf output.mf4
```

MF4 → BLF/ASC는 지원되는 Raw CAN Bus Logging을 명시 선택해서 추출한다. measurement·event·metadata가 함께 있으면 파일 전체의 lossless 변환으로 부르지 않는다. Measurement Signal만 존재하는 MF4에서 원본 CAN Frame을 임의 복원하지 않는다. 29장의 loss policy를 적용한다.

### diag

```bash
canlog diag stats drive.blf
```

예상 출력:

```text
UDS Transactions      8,421
0x10 SessionControl     121
0x11 ECUReset            32
0x14 ClearDTC            17
0x19 ReadDTC            822
0x22 ReadDID          6,992
0x31 RoutineControl     302
0x34 RequestDownload     12
0x36 TransferData       118
0x37 TransferExit         5
Negative Responses      27
```

---

## 21. Workspace 개념

사용자가 매번 DBC/CDD 옵션을 지정하지 않도록 Project/Workspace를 제공한다.

```bash
canlog workspace create EPICD
canlog workspace add EPICD drive.blf
canlog workspace add EPICD EPICD.dbc
canlog workspace add EPICD EPICD.cdd
```

논리 구조:

```text
EPICD/
├── canlog.toml
├── workspace.db
├── cache/
├── logs/
└── databases/
```

원본 파일은 default로 복사하지 않고 path/reference를 관리한다. portable workspace는 명시적으로 복사하며 checksum으로 동일성을 확인한다. manifest의 상대 path는 manifest 디렉터리 기준이다. 절대 path·파일 이동·접근 불가 시 재연결 흐름을 제공한다. 원본 폴더에 임의 sidecar를 쓰지 않는다.

assignment의 우선순위는 명시 CLI override → workspace binding → 자동 후보 제안 순서로 둔다. 자동 추정은 확정 binding을 덮어쓰지 않는다. schema version을 manifest에 추가하고 ECU별 진단 route와 DBC별 channel을 명확히 지정한다. TOML/SQLite 간 중복 정보는 manifest가 사용자 설정의 원본, SQLite가 파생 catalog/index라는 책임을 둔다.

### Manifest 예 — 개념 예시; 실제 binding schema는 구현 시 확정

```toml
schema_version = 1

[workspace]
name = "EPICD"

[[logs]]
path = "D:/logs/drive.blf"

[[dbc]]
path = "D:/database/EPICD.dbc"
channel = 1

[[cdd]]
path = "D:/database/EPICD.cdd"

[diagnostic]
protocol = "isotp"
request_id = 0x7E0
response_id = 0x7E8

[storage]
mode = "indexed"
database = "workspace.db"
```

---

## 22. Export

초기 필수는 JSONL과 CSV, 이후 Parquet와 bounded JSON document, 명시 SQLite export로 확장한다. SQLite workspace 내부 schema를 곧 외부 export 계약으로 노출하지 않는다.

- frame schema: source/ordinal/clock/time/channel/ID kind/DLC/payload/flags/quality.
- signal schema: qualified key/time/raw 또는 physical typed value/unit/validity/source refs.
- diagnostic schema: transaction/response/connection/status/times/payload/decoded fields.

JSON/JSONL은 ns와 u64가 JavaScript의 안전 정수 범위를 넘을 수 있으므로 버전이 있는 schema에서 큰 정수를 decimal string으로 encode한다. payload는 hex/base64 중 고정 형식을 사용하고 NaN/Infinity는 JSON 숫자로 내보내지 않는다. CSV는 escaping/null/encoding/time unit과 metadata sidecar를 정의한다. Parquet는 integer/float/binary column의 타입과 unit/clock metadata를 보존하고 row group을 bounded batch로 작성한다.

대용량 Signal 분석에는 CSV보다 Parquet export를 적극 고려한다.

```text
BLF/MF4
   ↓
DBC Decode
   ↓
Signal Stream
   ↓
Parquet
   ↓
Python / Data Analytics / AI
```

---

## 23. 성능·순서·자원 설계

streaming, lazy decode, bounded decompression, 후보 seek와 batch 처리를 기본으로 한다. streaming이라는 이름만으로 메모리 상한을 보장한 것으로 간주하지 않는다.

### 자원 상한과 병렬 처리

- 초기 평가용 memory budget 예시는 256 MiB다. worker/queue/reorder 크기를 총예산 안에서 정하고 권장값은 실측으로 확정한다.
- ASC line/token 길이, BLF object/container와 압축 해제 bytes, MF4 block/link depth/record 크기, ISO-TP payload/connection/buffer, query join 상태, export batch, cache quota에 상한을 둔다.
- 압축 해제량의 절대 상한은 필수다. 압축비 제한은 정상적인 고압축 입력도 고려해 profile에 설정한다.
- DBC compiled definitions와 worker별 Scratch를 사용한다. Send/Sync와 실제 engine 동작을 검증한 뒤 병렬화를 활성화한다.
- Reader/worker/Writer 사이에 bounded queue와 backpressure를 둔다. reorder buffer도 제한하며 CPU 수만으로 병렬도를 정하지 않는다.
- ordinal로 입력 순서를 복원하는 것과 timestamp 정렬은 다르다. ISO-TP/UDS 상태는 connection별 순서를 유지한다.

### Merge / Cut

merge는 clock/channel mapping을 명시한다. 입력이 공통 timeline에서 정렬됐으면 k-way merge를 사용하고 같은 시각은 source order→ordinal로 결정한다. 비정렬 입력은 external sort와 임시 디스크 예산을 선택하거나 거부한다. UTC를 자동 추정하지 않는다.

cut은 `[start,end)`이며 기본적으로 원본 timestamp를 유지한다. `--rebase-time`으로 바꾸면 원본 origin과 변환 mapping을 기록한다. 진단 cut의 분석 context와 출력 범위는 다르다. 잘라낸 출력만으로 ISO-TP가 완결되지 않으면 보고한다.

### Benchmark 완료 기준

hardware/OS/compiler/engine 버전, 입력 checksum/compression/record 종류, warm/cold cache와 건수를 기록한다. throughput, peak RSS, 첫 record까지 시간, seek latency, cache 크기를 측정한다.

대표 fixture의 10배 규모에서도 RSS가 설정 budget을 지키는지 검증한다. 단일 record/container가 상한을 넘으면 allocation 전에 거부한다. 고정 속도 목표는 baseline 측정 뒤 정하고 미측정 frames/s를 성능 보장으로 쓰지 않는다.

---

## 24. Error / Recovery / Cancel

Error에는 kind와 source, format/profile, physical/logical 위치, ordinal, block/object type, 원인 chain을 넣는다. 시간이 알려졌으면 함께 기록한다. decode 실패와 파일 구조 손상을 구분한다.

### 독립된 policy

1. Parse policy: 기본 strict는 손상 시 중단하며 recover는 검증 가능한 경계에서 재개한다.
2. Unsupported policy: error는 미지원 record에서 중단하며 skip은 위치·건수를 보고하고 제외한다.

정상적인 unknown object와 손상은 다른 문제다. recover여도 I/O 권한, 자원 상한, 쓰기 실패, 신뢰할 재동기화 경계 부재는 중단 사유다.

ASC는 다음 행, BLF는 검증된 object/container 경계, MF4는 검증 가능한 block link를 사용한다. magic byte만으로 정상 경계를 판정하지 않고 길이/alignment/link/file 범위를 확인한다. 재동기화 탐색량과 시간도 제한한다.

Report는 정상/미지원/제외/손상/복구/incomplete를 구분한다. 모르는 건수는 unknown이다. gap을 진단·query에 전달하고 누락 구간을 근거로 PASS를 만들지 않는다.

### 출력과 중단

입력과 출력이 symlink/hardlink를 포함해 같은 실체면 거부한다. 기존 출력은 기본적으로 보존하고 명시 overwrite만 허용한다. 같은 filesystem의 고유 temp에 쓴 뒤 finish 성공 후 게시한다. 기존 파일을 덮지 않는 게시와 명시 교체를 분리하고 Windows 차이도 테스트한다.

SIGINT/CancelToken, disk full, SQLite busy, 중간 I/O 실패는 temp와 partial cache를 정리하며 complete로 표시하지 않는다. 부분 결과 보존은 명시 policy가 필요하고 sidecar에도 partial을 기록한다. stdout은 rollback할 수 없으므로 final status와 stderr report로 알린다.

종료 코드안: `0` 완전 성공, `1` 실행/I/O/parse 실패, `2` 인자/설정 오류, `3` 허용된 부분 성공·손실, `4` validation FAIL, `5` validation INCONCLUSIVE. POSIX SIGINT는 `130`을 사용한다. partial을 성공 코드 0으로 숨기지 않는다.

---

## 25. Test Strategy와 판정 기준

### Fixture와 비교 대상

fixture manifest에는 producer/format version/profile, 생성 방법, license/checksum, 기대 records/issues/limitations를 둔다. python-can/asammdf의 버전을 고정하되 하나의 구현을 유일한 정답으로 취급하지 않는다. 불일치는 사양, producer fixture, 다른 reader를 통해 원인을 분류한다.

round-trip은 지원 profile의 semantic equality를 검증한다. byte 보존은 별도 계약이다. 정수 timestamp는 정확히 비교하고 출력 양자화가 있으면 입출력 해상도로 허용 오차를 정해 보고한다. 넓은 epsilon 하나를 모든 포맷에 적용하지 않는다.

| 계층 | 필수 검증 |
|---|---|
| Core/Time | Standard/Extended 경계, DLC/payload, Remote, overflow/NaN, clock 차이, 동시각/역행 |
| ASC | header/base/time mode/dialect, Classic/FD/error/event, 긴 행, 잘린 마지막 행 |
| BLF | version/object variants, 압축·비압축/padding, container 경계 통과, unknown/truncation/해제 상한 |
| MF4 | 지원 profile/link cycle, sorted/unsorted·압축, master clock/conversion/invalid sample/CAN extraction |
| DBC adapter | endian/signed/scaling/multiplex, ID bit31, FD byte 길이, duplicates, raw u64/quality, DB 변경 |
| ISO-TP | SF/FF/CF/FC, sequence wrap, 누락/중복/FC 부재/timeout/EOF, addressing/FD PCI/자원 상한 |
| UDS/CDD | pending→final, negative, multi-DID, functional 다중 응답, suppressed/orphan/ambiguous, context/profile guard |
| SQLite | FK/unique/check/migration, partial coverage, 동시각 sample, 원본·assignment 변경, busy/rollback |
| Query/Export | 타입/결측/unit/sample age/context, 큰 정수, loss rejection, 원자적 출력/cancel/disk full |
| CLI/App | 종료 코드, stdout/stderr/report, 실제 file 기반 end-to-end |

Property tests는 core 불변 조건, 적용 가능한 decode round-trip, time conversion, index와 full scan 결과 일치를 다룬다. 같은 query/policy가 index 유무에 관계없이 같은 records/issues를 반환해야 한다.

ASC, BLF, MF4, ISO-TP, query parser를 fuzz 대상으로 한다. panic, 무한 loop, 자원 상한 위반, 검증 전 allocation을 찾고 crash corpus를 회귀 fixture로 보존한다.

### CI와 실행 증거

Rust format/lint/unit/integration, supported feature matrix, Linux/Windows file 동작을 검사한다. 엔진이 없는 CI에서는 integration을 skip으로 표시하고 mock 성공을 실제 연동 완료로 부르지 않는다. 대용량 benchmark와 장시간 fuzz는 별도 job으로 운영할 수 있다.

phase마다 실행 건수, passed/failed/skipped, fixture profile, 미지원 범위를 기록한다. zero-test 성공과 자체 Writer→자체 Reader 성공만으로 호환성을 주장하지 않는다. 외부 엔진 테스트도 이번 정적 검토와 실제 실행 결과를 구분한다.

---

## 26. 단계별 구현 계획과 완료 조건

ASC/BLF의 올바른 streaming 변환부터 완성한다. MF4 전체 지원, 일반 시간 join, GUI는 MVP 조건이 아니다. 예정 CLI 기능을 구현된 것으로 표시하지 않는다.

| Phase | 구현 범위 | 완료 조건 |
|---|---|---|
| 1 Foundation | Core/time/channel/provenance, error/policy, Reader/Writer, CLI 골격 | 불변 조건·overflow·capability 검증, 최소 fixture의 CLI 처리 |
| 2 ASC vertical slice | Reader/Writer, info/view/filter/convert/stats, CSV/JSONL/report | 선택 dialect의 Classic/FD 외부 비교, 원자적 출력, bounded memory |
| 3 BLF | 지원 object/profile, container/carry, Reader/Writer | ASC↔BLF 의미 비교, 경계/손상/압축 상한, unknown report |
| 4 Workspace/Index | manifest/revision/SQLite, ASC/BLF seek | index/full scan 일치, 원본 변경 감지, 재시작/migration, partial 재개 |
| 5 DBC | 실제 candb-engine adapter, assignment, typed decode/cache | engine commit 고정, 실제 DBC/multiplex/FD/u64/quality, cache 무효화 |
| 6 ISO-TP/UDS/CDD | route, reassembly/matching, 실제 cdd-api adapter | incomplete/다중응답/pending/ambiguity, request context, experimental provenance |
| 7 MF4 Reader | 제한 profile metadata/measurement/CAN bus | capability 목록, 선택 channel의 bounded read, raw/physical/invalid/time |
| 8 Query/Analysis | cross-signal alignment, temporal join, validation | clock/unit/context/INCONCLUSIVE, 결정적 결과 |
| 9 Advanced Export | Parquet, merge/cut, 선택 MF4 Writer | 외부 Reader 검증, 손실 matrix, external sort/disk 상한 |
| 10 Frontends | GUI/Python/MCP, 안정 Application API | CLI와 같은 typed 결과, API version/cancel/progress |

**MVP는 Phase 1〜3**이다. 선택 ASC dialect·BLF profile의 Classic/FD 읽기·쓰기, 기본 CLI, CSV/JSONL/report, 의미 비교와 자원 상한을 포함한다. error/event/unknown의 완전 보존은 profile별로 판정한다. **후속 실용판은 Phase 4〜6**으로 Workspace/DBC/진단을 완성하고 MF4는 독립 reader beta에서 확대한다.

Phase 1부터 이미 확인한 engine API의 adapter fixture와 toolchain/dependency 조합을 검증한다. 부족한 엔진 API는 별도 개선 제안으로 다루며 canlog에서 parser/codec을 복제하지 않는다.

---

## 27. 최종 Application 확장 구조

CLI를 최종 제품 계층으로 만들지 않고 Rust Application API 위의 하나의 Frontend로 둔다.

```text
                    ┌──────── CLI
                    │
                    ├──────── GUI
                    │
Application API ────┼──────── Python Binding
                    │
                    └──────── MCP / AI Agent
                         │
                         ▼
               Query / Analysis Engine
                         │
          ┌──────────────┼──────────────┐
          ▼              ▼              ▼
       Log Engine     DBC/CDD        SQLite
```

이를 통해 향후 GUI, 자동화 서버, AutoTest Lab, AI 분석 기능에서 동일한 backend를 재사용할 수 있다.

---

## 28. 최종 권장안

`canlog-rs`는 단순 ASC/BLF/MF4 converter가 아니라 다음 네 가지 기능을 하나의 Rust backend로 통합하는 방향이 적합하다.

```text
1. Automotive Log Engine
   ASC / BLF / MF4

2. Vehicle Data Decode
   Existing DBC Engine

3. Diagnostic Analysis
   ISO-TP + Existing CDD Engine

4. Data Workspace
   SQLite Index / Cache / Query / Analysis
```

특히 **DBC/CDD Engine을 기존 구현 그대로 독립 유지하고 Adapter를 통해 연결하는 것**, 그리고 **SQLite를 원본 데이터 저장소가 아닌 Workspace/Index/Cache로 사용하는 것**이 핵심 설계 결정이다.

이 구조는 이후 CANoe/CANalyzer 스타일 Trace 분석, Signal Plot, Diagnostic 분석, 자동 Validation, GUI, Python API, MCP/AI 연동까지 확장할 수 있는 기반이 된다.
## 29. 변환 지원 범위와 정보 손실 계약

포맷 이름만으로 지원 여부를 표현하지 않는다. Reader/Writer 각각 판본, producer dialect, record 종류, 필드 보존 범위를 가진다.

| 경로 | 지원 조건 | 보고할 제한 |
|---|---|---|
| ASC ↔ BLF | 양쪽 profile이 CAN/FD record와 flags를 지원 | timestamp quantization, DLC/direction/error/event 표현 차이, 원본 header/주석 |
| BLF → MF4 | MF4 Writer의 선택 bus logging profile 구현 후 | 비CAN object와 metadata 보존 여부, channel/time mapping |
| MF4 → ASC/BLF | 지원 raw CAN bus logging subset을 선택 | measurement/conversion/event/attachment는 이동되지 않을 수 있음 |
| MF4 measurement → CAN | 기본 미지원 | 원본 frame 복원 불가. DBC encoding은 별도 합성 작업이며 실측 원본이 아님 |
| Frame/Signal/UDS → CSV/JSONL/Parquet | versioned export schema | 분석용 projection이며 원본 포맷 round-trip 보장 없음 |
| 같은 포맷 read/write | support profile의 semantic subset | unknown object, vendor extension, metadata의 byte equality 보장 없음 |

기본 `loss-policy=reject`다. metadata scan으로 미리 알 수 있는 손실은 preflight에서 거부한다. 처리 중 발견한 손실도 file publish 전에 실패시킨다. `allow`는 사용자가 선택한 범위의 손실만 허용하며 partial exit와 report를 남긴다. 누락을 숨긴 성공 결과를 만들지 않는다.

Report에는 tool/profile version, input identity, emitted/dropped/unsupported/recovered counts, field losses, time rounding bounds, clock/channel mapping, partial 여부를 넣는다. 원본 byte copy와 semantic conversion은 다른 기능이다.

---

## 30. Application API와 실행 계획

```text
CLI / GUI / Python / MCP
    → Request(typed options, sources, workspace, policies)
    → Inspect / Plan(capabilities, bindings, dependencies, resource estimate)
    → Execute(reader → decode/diagnostics → query → sink)
    → Result + Quality/Loss Report
```

`app`에는 `inspect`, `convert`, `filter`, `decode`, `diagnostics`, `query`, `index`, `validate`의 typed request/response를 둔다. CLI가 parser/SQLite를 직접 조합하지 않는다. core는 동기 순차 I/O로 시작하고 async wrapper는 필요할 때 추가한다.

Request에는 cancel/progress hook, memory/disk budget, parse/unsupported/loss policies, output schema version을 둔다. Progress는 bytes/records와 known/unknown total을 구분하며 제한된 주기로 전달한다.

Plan은 signal 조건에 필요한 CAN IDs, ISO-TP context, DB binding, clock conversion, sort/spool 필요 여부를 설명 가능한 형태로 반환한다. estimate가 불확실하면 unknown으로 표시한다. API result에서 engine 내부 타입, SQL row, CLI 문자열을 누출하지 않는다.

---

## 31. 운영·보안·재현성

- 로그/DBC/CDD/rule은 신뢰하지 않는 입력으로 취급한다. 길이·offset·link를 검증하고 panic과 과도한 allocation을 방지한다.
- CDD XML의 외부 entity/network resolution, nesting/size 제한은 엔진 설정과 테스트로 확인한다. canlog가 별도 XML parser를 추가해서 우회하지 않는다.
- 기본 workflow는 오프라인이다. 로그 분석을 위해 CAN 송신, ECU 접속, 외부 업로드를 자동 수행하지 않는다.
- payload에는 VIN/진단정보가 포함될 수 있다. debug/error 출력은 byte dump를 기본으로 하지 않고, report/export의 포함 필드를 설정할 수 있게 한다.
- 결과 metadata에는 binary/engine/schema/profile versions, 원본/DB identity, assignments, time mapping, query/rule과 policies를 기록한다. 재현을 위해 기밀 payload 전체를 복제할 필요는 없다.
- 두 engine의 확인한 manifest에는 license 선언이 없으며 별도 LICENSE도 발견되지 않았다. 공개 GitHub 접근이 곧 배포 허가를 의미하지 않으므로 소유자 정책에 맞는 라이선스를 확정한다. 참고 구현 코드를 이식할 때도 라이선스와 배포 조건을 확인한다.

---

## 32. 확정된 연동 결정과 남은 확인 항목

### 지금 확정할 수 있는 결정

1. DBC는 `candb-csharp-clone/engine`의 `candb-engine` library를 사용한다. C# GUI는 연동 대상에서 제외한다.
2. DBC parser와 precompiled codec을 재사용한다. adapter는 ID/길이/typed result/assignment 경계만 담당한다.
3. DBC exact raw/quality 경로와 physical-only 고속 경로를 구분한다. 후자는 전자의 대체물이 아니다.
4. CDD는 `cdd-api::CddEngine`을 사용하며 context-aware identify/decode/request-context를 재사용한다.
5. ISO-TP transport와 로그 transaction matching은 canlog 책임, CDD message identification/meaning decode는 CDD Engine 책임이다.
6. Experimental CDD opt-in과 provenance, DB/engine revision별 cache invalidation을 공통 계약에 넣는다.

### 구현 전에 확인할 항목

| 항목 | 현재 근거 / 필요한 확인 | 영향 |
|---|---|---|
| DBC API | package/API 확인 완료. thread traits, adapter row 대응과 실제 테스트 실행은 필요 | worker ownership, typed result와 오류 보존 |
| CDD API | facade/context/experimental guard 확인 완료. pinned build와 실차 CDD 검증은 필요 | experimental 지원 범위, response matching/decode |
| Toolchain/deps | CDD pin 1.98.1, DBC Edition 2024. 설치 가능 compiler와 SQLite native deps 조합 확인 | reproducible Cargo integration |
| Engine 버전 관리 | 확인 commit 고정. 개발 중 CDD upgrade policy와 schema 변경 검증 필요 | context/cache compatibility |
| 실제 로그 | producer/version, 익명화 샘플, 손상/unknown fixtures 필요 | MVP dialect와 object/block profile |
| MDF4 사양 | 적용 판본과 접근 가능한 사양/fixture 확인 | bus layout, conversion/compression 지원 |
| 진단 route | ECU/channel/ID/address mode/functional responder 집합 필요 | ISO-TP key와 UDS matching |
| 시간/단위 | timezone/clock sync, signal/DID unit 확인 | merge/query/cross validation |
| 배포/라이선스 | Linux/Windows, data 규모, engine license 정책 확인 | file atomicity, 자원 budget, 배포 |

권장 순서는 **엔진 commit 고정 및 adapter fixture 준비 → Core/ASC vertical slice → BLF support profile → Workspace/DBC → 진단/CDD → MF4**다. 첫 구현은 선택한 profile을 정확히 읽고, 제한된 자원으로 처리하며, 정보 손실과 불완전성을 보고할 수 있어야 한다.

---

## 33. 이번 개정의 코드 근거와 검증 수준

2026-10-02에 두 저장소의 아래 commit을 HTTPS read-only로 조회하고 별도 임시 디렉터리에서 검토했다. 참조 저장소를 canlog checkout의 member로 추가하거나 수정하지 않았다.

- [DBC manifest](https://github.com/najari/candb-csharp-clone/blob/da64ad9ccf10237fce0993d1b83f460416d3da53/engine/Cargo.toml), [parser](https://github.com/najari/candb-csharp-clone/blob/da64ad9ccf10237fce0993d1b83f460416d3da53/engine/src/parser.rs), [model](https://github.com/najari/candb-csharp-clone/blob/da64ad9ccf10237fce0993d1b83f460416d3da53/engine/src/model.rs), [codec](https://github.com/najari/candb-csharp-clone/blob/da64ad9ccf10237fce0993d1b83f460416d3da53/engine/src/codec.rs), [codec tests](https://github.com/najari/candb-csharp-clone/blob/da64ad9ccf10237fce0993d1b83f460416d3da53/engine/tests/codec.rs).
- [CDD facade](https://github.com/najari/cdd-rust-engine/blob/0bd598b645814b178f9a4b313caba26cf873833e/crates/cdd-api/src/engine.rs), [request context](https://github.com/najari/cdd-rust-engine/blob/0bd598b645814b178f9a4b313caba26cf873833e/crates/cdd-api/src/context.rs), [profile](https://github.com/najari/cdd-rust-engine/blob/0bd598b645814b178f9a4b313caba26cf873833e/crates/cdd-core/src/profile.rs), [decode types](https://github.com/najari/cdd-rust-engine/blob/0bd598b645814b178f9a4b313caba26cf873833e/crates/cdd-codec/src/decode.rs), [decode tests](https://github.com/najari/cdd-rust-engine/blob/0bd598b645814b178f9a4b313caba26cf873833e/crates/cdd-api/tests/decode.rs), [toolchain](https://github.com/najari/cdd-rust-engine/blob/0bd598b645814b178f9a4b313caba26cf873833e/rust-toolchain.toml).

검토 범위는 API/구현/테스트의 정적 리뷰이며 Rust build나 canlog 통합 테스트의 실행 성공을 의미하지 않는다. 문서 내 SQL은 SQLite에서 schema 생성과 대표 제약을 확인하되, 완성된 storage 구현으로 표현하지 않는다.

문서 검증 결과: 33개 장의 순서, code fence, TOML 예제 파싱을 확인했다. SQLite에서 schema 생성, 정상 데이터 삽입, 동일 timestamp의 복수 sample, 최대 u64의 BLOB 보존, 잘못된 ID/FK/coverage/anchor/origin 및 중복 sample 거부 7개 사례가 통과했다. 실제 로그 변환·engine 통합 테스트는 아직 실행하지 않았다.
