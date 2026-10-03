# CAN 파일 기록·재생 CLI 엔진 개발 계획

작성일: 2026-10-03

상태: 이 문서는 구현 전에 작성한 계획이다. 파일 기반 구현은 [README](../README.md), 실제 지원 범위는 [지원 문서](support.md), 실행 증거는 [검증 결과](validation.md)를 기준으로 확인한다.

## 1. 목표와 확정 범위

Rust 라이브러리와 Windows에서 실행 가능한 `canlog.exe`를 개발한다. 사용자가 선택한 **파일 기반부터 구현**을 1차 범위로 삼는다.

- **기록(record):** ASC·BLF 파일 또는 정의된 JSONL 프레임 스트림을 읽어 새 ASC·BLF 파일로 기록한다. 입력 프레임의 시간·채널·ID 종류·방향·DLC·payload·FD flags를 보존한다.
- **재생(replay):** 파일을 순차적으로 읽고 원본 시간 간격에 맞춰 콘솔/JSONL 스트림 또는 새 로그 파일로 내보낸다. 배속, 시간 구간, 채널/ID 선택, 반복, 취소를 지원한다.
- **검사·변환:** info/view/stats/filter/convert/export를 같은 엔진 위에 제공한다. 읽기·변환은 재생 대기 없이 처리한다.
- **검증:** 제공된 ASC 21개, BLF 22개를 전수 조사하고, 합성 경계 fixture 및 외부 reader로 지원 프로파일의 의미 보존을 확인한다.

1차 범위에서 recording은 파일/스트림을 새 로그로 저장하는 기능이다. CAN 장비 수신, 물리 버스 송신은 이후 transport adapter 단계다. 향후 장비 연결을 위해 FrameSource/FrameSink 경계를 둔다.

상위 설계는 [CANLOG-RS 설계서](canlog-rs-design.md)를 따른다. 설계서의 Workspace/SQLite, DBC/CDD, ISO-TP/UDS, MF4, GUI는 별도 후속 단계이며, 이번 계획으로 구현 완료를 선언하지 않는다. 이번 파일 기반 작업은 Foundation/ASC/BLF에 recording/replay 실행 경로를 추가하는 것이다.

## 2. 현재 저장소와 샘플에서 확인한 사실

- 저장소에는 설계 문서가 있으며 Cargo 프로젝트와 엔진 구현은 없다.
- 샘플 경로는 `can_example/asf/*.asc`, `can_example/blf/*.blf`다. 폴더 이름 `asf`를 임의 변경하지 않는다.
- 샘플 폴더는 현재 Git 미추적 상태다. 입력 샘플을 수정하지 않으며, 배포 권한을 확인하기 전 자동으로 Git에 추가하거나 외부로 업로드하지 않는다.
- ASC에는 hex/dec base, absolute/relative 시간 선언, 환경 변수·시스템 변수·측정 이벤트가 섞여 있다.
- `Logging_CAN1.asc`, `Logging_CAN2.asc`, `CANOE.ASC`에는 메시지 이름과 뒤쪽 `ID = ...` 필드가 함께 있는 CAN 행이 있다.
- `can2.asc`에는 `Stress2`처럼 이름만 있고 숫자 ID가 없는 CAN 행이 있다. DBC 등의 검증 가능한 매핑 없이 숫자 CAN ID를 만들어 내지 않는다.
- BLF에는 CAN 외에 LIN/MOST/Ethernet/A429 용도로 보이는 샘플도 있다. 파일 이름만으로 내용·지원 여부를 확정하지 않고 object를 조사한다.
- Rust stable MSVC toolchain은 설치되어 있지만 현재 셸 PATH에서 cargo/rustc가 발견되지 않았다. Visual Studio C++ 도구와 Windows SDK도 발견했다. 첫 단계에서 실제 빌드로 linker 환경을 검증한다.

위 목록은 일부 헤더/행과 파일 목록을 확인한 결과다. 전체 frame 수, FD/error 포함 여부, 손상 여부, 메모리 사용량은 아직 검증하지 않았다.

## 3. 엔진 구조

초기에는 한 Cargo 패키지의 라이브러리와 CLI로 시작하고 책임을 모듈로 나눈다. 재사용과 선택 빌드가 실제로 필요해지면 crate를 분리한다.

```text
src/
  lib.rs              재사용 가능한 엔진 API
  core/               validated frame, time, channel, provenance, issue
  formats/asc/        ASC Reader/Writer
  formats/blf/        BLF header/container/object Reader/Writer
  formats/jsonl/      versioned frame stream Reader/Writer
  app/                filter, inspect, record, convert, export, reports
  playback/           scheduler, playback controls, cancellation
  output/             temp file, finalize, sync, atomic publication
  main.rs             CLI arguments → app API
tests/
  fixtures/           프로젝트에서 생성한 최소·경계 fixture
  integration/        CLI, round-trip, fault/cancel tests
scripts/              Windows build/test 및 로컬 샘플 검증
docs/                 사용법, 지원 범위, 검증 결과
```

처리 경로:

```text
파일/JSONL → Reader → validated record + issue → Filter
                                                    ├→ Recorder → Writer → finalize → publish
                                                    ├→ PlaybackScheduler → Sink
                                                    └→ info/stats/export
```

Reader에는 sleep, 배속, 송신을 넣지 않는다. Scheduler는 monotonic clock으로 시간 간격을 제어하고 Sink는 출력 방식을 담당한다. 파일 길이에 비례하는 전체 Vec와 무제한 queue를 기본 경로에 사용하지 않는다.

## 4. 데이터·시간·손실 계약

### CAN 모델

- Standard/Extended ID를 구분하고 범위를 검증한다. 같은 숫자의 Standard/Extended를 서로 다른 ID로 유지한다.
- Classic payload 최대 8 bytes, FD 최대 64 bytes. 원시 DLC 코드와 payload 길이를 별도 보존한다.
- Classic DLC 9–15, Remote, FD DLC 길이 매핑, BRS/ESI를 검증한다. FD Remote와 Classic의 FD flags는 거부한다.
- Error와 일반 CAN frame을 분리한다. 미지원 event/object와 손상은 issue 종류를 구분한다.
- source와 ordinal, ASC 행 위치 또는 BLF container/object 위치를 결과와 오류에 포함한다.
- JSONL에는 schema version과 정수 timestamp를 넣고, 역직렬화도 validated constructor를 통과시킨다.

### 시간

- timestamp는 checked integer nanoseconds로 처리하고 overflow를 거부한다. ASC decimal 시간은 불필요한 f64 왕복 없이 파싱한다.
- 상대시간 기준, header 선언, trigger block과 입력 순서를 profile로 명시한다. `timestamps relative`의 delta 해석은 producer fixture 및 독립 비교로 확정한다. 대조 reader가 같은 해석을 보장한다고 가정하지 않는다.
- 시간대가 없는 ASC date를 임의 UTC로 변환하지 않는다. 파일의 clock origin과 상대 offset을 분리한다.
- 시간 필터는 `[start, end)`다. 같은 timestamp에서는 원본 순서를 유지한다.
- 역행 timestamp를 정렬하거나 보정해 숨기지 않는다. replay 기본값은 역행을 위치와 함께 거부하며, 명시적 원본 순서 즉시 출력 정책을 선택할 수 있게 한다.

### 정보 손실과 성공 판정

- parse 기본값은 strict, unsupported 기본값은 error, 재작성 loss 기본값은 reject로 둔다.
- recover/skip과 loss 허용은 별도 옵션이다. skip 선택만으로 새 로그의 정보 손실을 허용하지 않는다.
- 미지원 record, 손상 구간, 원본 metadata, 포맷 전용 field, 시간 양자화를 보고한다. 허용할 손실 category를 명시해야 파일을 게시한다.
- accepted/flushed/finalized/durable/published를 구분한다. 성공 메시지는 finalize와 게시가 끝난 뒤 출력한다.
- 기존 출력은 기본 보존하고 `--overwrite`를 명시해야 교체한다. 입력과 출력이 같은 실체인 경우 hardlink/symlink까지 거부한다.
- 종료 코드: 0 완전 성공, 1 처리 실패, 2 인자/설정 오류, 3 명시 허용된 부분 성공, 130 취소. stdout 출력은 되돌릴 수 없으므로 실패 시 partial 상태를 stderr/report에 알린다.

## 5. CLI 계획

다음은 **계획된 문법**이며 현재 실행 가능한 명령이 아니다. 구현 과정에서 세부 옵션을 확정하고 help와 문서를 함께 갱신한다.

| 명령 | 동작 |
|---|---|
| `canlog info INPUT [--scan]` | header metadata와 실제 scan 결과를 구분해 표시 |
| `canlog view INPUT [--limit 100]` | 프레임과 원본 위치 확인; 전체 출력은 명시 옵션 |
| `canlog stats INPUT` | 채널/ID/format/direction별 수, 시간 범위, issue 수 |
| `canlog convert INPUT OUTPUT` | 지원 프로파일 간 시간 대기 없는 변환 |
| `canlog filter INPUT -o OUTPUT ...` | 채널, ID 종류/범위, 방향, 시간 구간 선택 |
| `canlog export INPUT -o OUTPUT --format jsonl\|csv` | schema가 명시된 분석용 export |
| `canlog record --input INPUT -o OUTPUT` | 파일 또는 JSONL stdin을 새 ASC/BLF 로그로 기록 |
| `canlog replay INPUT --speed 1 --sink jsonl` | 원본 간격으로 재생; 기본 반복 1회 |
| `canlog replay INPUT --speed 2 --repeat 3 -o OUTPUT` | 2배속·3회 재생을 새 로그로 기록 |
| `canlog replay INPUT --no-wait ...` | 시간 대기 없이 재생 경로 검증/출력 |

공통 옵션은 `--input-format`, `--channel`, `--id`, `--id-kind`, `--direction`, `--start`, `--end`, `--limit`, `--unsupported error|skip`, `--recover`, 반복 가능한 `--allow-loss`, `--report`, `--overwrite`, 자원 상한으로 구성한다. 필요한 명령에만 옵션을 노출한다.

예정 사용 예:

```powershell
.\canlog.exe info .\can_example\asf\ComfortDiagData.asc --scan
.\canlog.exe record --input .\can_example\asf\ComfortDiagData.asc -o .\recorded.blf
.\canlog.exe replay .\recorded.blf --speed 2 --sink jsonl
.\canlog.exe replay .\recorded.blf --no-wait -o .\replayed.asc
```

CLI replay에서는 stdin control 모드를 명시 선택해 pause/resume/stop을 받도록 한다. JSONL 입력과 control stdin이 충돌하는 조합은 실행 전에 거부한다. 비대화식 재생에는 Ctrl+C 취소를 제공한다.

반복 재생은 각 회차를 같은 입력 clock으로 읽되 출력 timeline을 별도로 계산한다. 첫 프레임은 즉시 출력하고, 반복 경계의 간격/회차 offset 정책을 명시한다. 원본 timestamp와 출력 timestamp를 구분하며, file Sink의 timeline 변경도 report에 남긴다. 배속은 실행 대기 간격을 바꾸며 원본 timestamp를 암묵적으로 바꾸지 않는다.

## 6. 구현 단계와 완료 기준

### 1단계 — 프로젝트·샘플 조사·Core

1. Cargo package, lockfile, MSRV/edition, build script, 최소 CI를 구성한다.
2. 실제 Rust hello/build/test로 Windows linker와 SDK 환경을 확인한다.
3. 샘플 manifest를 만든다: 상대 경로, 크기, SHA-256, producer/header/profile, records/issues와 확인 수준. 출처·라이선스 미확인은 그대로 적는다.
4. ASC의 전체 행 유형과 BLF object type/version/compression을 bounded scan으로 분류한다.
5. validated CAN/time/provenance, Reader/Writer, cancellation과 report API를 구현한다.

완료 기준: 잘못된 ID/DLC/flags/payload/시간 overflow를 거부하는 Core 테스트와 최소 CLI 처리 성공. 각 샘플의 지원·부분 지원·거부 사유를 조사 결과로 남긴다.

### 2단계 — ASC 읽기·쓰기와 CLI

1. hex/dec, Classic/FD/Remote, 숫자 ID와 검증 가능한 `ID = ...` 행을 처리한다.
2. symbolic-only CAN 행을 unresolved-ID issue로 보고한다. 사용자가 제공한 명시 매핑을 후속 확장할 수 있는 경계를 둔다.
3. header/time/trigger profile, 오류·event 정책, ASC Writer를 구현한다.
4. info/view/stats/filter/convert 및 JSONL/CSV export를 연결한다.
5. 최대 행/토큰 길이, 예외·I/O 실패, 임시 출력 정리를 검증한다.

완료 기준: 지원 ASC fixture의 frame semantics가 round-trip에서 일치하고 외부 reader로 생성 파일을 읽을 수 있다. 미지원/손상이 조용히 누락되지 않으며, 실패 시 기존 출력이 유지된다.

### 3단계 — BLF 읽기·쓰기

1. file/object header, v1/v2 timestamp, channel, 압축 method를 검증한다.
2. 초기 CAN object 후보는 CAN_MESSAGE(1), CAN_MESSAGE2(86), CAN_FD_MESSAGE(100), CAN_FD_MESSAGE_64(101)다. CAN error object는 독립 record로 취급하고 보존 가능 범위를 명시한다.
3. uncompressed/zlib container, container를 가로지르는 object carry를 구현한다.
4. 선언 크기와 실제 압축 해제 크기, object 길이·padding·남은 EOF를 검증한다. 알려진 경계 없이 magic 검색만으로 손상 복구하지 않는다.
5. Classic/FD Writer의 header count/size/time 및 finalize를 구현한다.

완료 기준: 지원 profile의 ASC↔BLF semantic round-trip와 독립 reader 비교 통과. 잘린 object, cross-container object, 잘못된 크기, 과도한 압축 해제, 미지원 object가 명시적으로 처리된다. BLF 전체 포맷 호환성으로 확대 표현하지 않는다.

### 4단계 — Recorder와 안전한 출력

1. Reader 또는 JSONL stdin을 받는 Recorder를 구현한다.
2. synchronous streaming/backpressure와 bounded writer buffer를 적용한다.
3. output temp → finish/flush → 선택 sync → atomic publish 순서를 구현한다.
4. 새 출력/overwrite, 동일 입력 실체, 권한·disk-write failure, 취소 경로를 검증한다.
5. 기록 중 수와 최종 기록 수·손실·완료 수준을 report로 제공한다.

완료 기준: 원본/기존 출력 보호와 CLI 기록 성공을 end-to-end로 확인한다. malformed JSONL, 쓰기 실패, 취소에서 성공 상태나 완성된 파일을 잘못 게시하지 않는다.

### 5단계 — PlaybackScheduler와 Replay

1. monotonic deadline과 입력 상대 시간으로 scheduling한다. 프레임마다 sleep을 누적해 drift가 커지는 구조를 피한다.
2. 배속 값의 finite/positive 검증, no-wait, selection, repeat 정책을 구현한다.
3. pause/resume 시 paused duration을 deadline에 반영한다. 취소는 긴 시간 간격에서도 빠르게 반응한다.
4. Sink를 console/JSONL/file로 연결하고 stdout/stderr를 분리한다.
5. 늦은 sink, 동일 timestamp, 역행 timestamp, repeat 경계, 0-frame 입력을 검증한다.

완료 기준: fake clock 테스트로 정확한 deadline/순서/pause 정책을 확인한다. 실제 CLI에서 1배속·2배속·no-wait의 프레임 수/순서/실행시간을 확인하고 허용 오차와 플랫폼을 기록한다. 원본에 긴 무프레임 구간이 있어도 취소가 지연되지 않는다.

### 6단계 — 샘플 전수 검증·패키징

1. 로컬 43개 샘플을 조사/처리하고 결과를 표로 저장한다. unsupported 전용 파일도 오류/부분 결과가 기대와 일치하는지 확인한다.
2. 프로젝트 생성 fixture로 Standard/Extended, Rx/Tx, zero payload, Remote, raw DLC 9–15, FD 12–64 bytes, BRS/ESI, 동일/역행 시간과 손상 사례를 채운다.
3. 버전을 고정한 python-can을 독립 비교용으로만 사용한다. Rust 실행 파일에 Python 의존성을 넣지 않는다. 상대 reader가 놓치는 raw DLC/시간 정밀도/unknown 의미는 별도 기대값으로 검증한다.
4. 반복 생성한 로그의 1배/10배 크기로 peak RSS·처리량·첫 프레임 지연을 측정하고 자원 상한을 확인한다. 수치 목표는 측정 근거와 함께 확정한다.
5. release build, help, 사용 예, 지원/loss matrix, 샘플 검증 report를 제공한다.

완료 기준: 테스트·독립 비교·실행 데모가 실제 실행 결과로 남고, release CLI로 ASC→BLF 기록→timed replay→ASC/JSONL 출력이 성공한다. 남은 미지원 format/profile을 문서에 표시한다.

## 7. 검증 매트릭스

| 영역 | 검증 대상 | 증거 |
|---|---|---|
| Core | ID 종류/범위, DLC/length, flags, overflow | constructor 및 경계 테스트 |
| ASC | base/time dialect, symbolic ID, Classic/FD/Remote, unsupported | 합성 fixture + 제공 샘플 + 독립 비교 |
| BLF | header/version/unit, 압축, carry, CAN/FD, unknown | 경계 fixture + 제공 샘플 + 독립 비교 |
| Record | 파일/JSONL 입력, finalize, 출력 보호 | CLI integration과 실패 주입 |
| Replay | 배속, pause/resume, repeat, filter, cancel, drift | fake clock + 실제 timed CLI 실행 |
| Round-trip | timestamp/channel/id-kind/direction/DLC/data/flags | 정규화 record stream 비교, loss report |
| Resources | 긴 행, 큰 선언 크기, 압축 확장, 10배 입력 | limit 오류 + peak RSS 측정 |
| Delivery | release exe, help와 문서의 실제 명령 | Windows release 실행과 데모 report |

CI에 재배포 권한이 확인되지 않은 로컬 샘플을 올리지 않는다. CI는 프로젝트 생성 fixture를 사용하고, 사용자 샘플 검증은 로컬 script로 재현한다.

## 8. 산출물과 작업 순서

산출물은 Rust library/CLI 소스, `Cargo.lock`, Windows build/test script, 생성 fixture, 샘플 manifest/검증 report, release 실행 파일, README/지원 범위 문서다.

첫 구현은 **환경 빌드 확인 → 샘플 manifest/프로파일 조사 → Core → ASC vertical slice** 순서로 진행한다. 이후 BLF, Recorder, Replay를 순차 연결하고 최종 단계에서 전체 recording/replay 경로를 검증한다. 계획서 작성만으로 엔진 목표를 완료 처리하지 않는다.

## 9. 참고와 확인 수준

- [CANLOG-RS 설계서](canlog-rs-design.md): 모델·손실 정책·완료 단계·Reader/Scheduler 분리 기준.
- [gocan/EcuBus 검토](gocan-ecubus-review.md): 비교 사례와 지원 범위 주의점.
- [python-can ASC 구현 문서](https://python-can.readthedocs.io/en/stable/_modules/can/io/asc.html): dialect와 독립 비교 후보.
- [python-can BLF 구현 문서](https://python-can.readthedocs.io/en/stable/_modules/can/io/blf.html): object/container 구조와 독립 비교 후보.

외부 구현은 비교 대상이며 포맷 적합성의 유일한 판정 기준으로 삼지 않는다. 필요 시 고정 revision의 라이선스를 확인한 뒤 참조하고, 실제로 검증된 producer/profile만 지원 목록에 올린다.
