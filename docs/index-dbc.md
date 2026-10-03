# SQLite 검색 인덱스와 DBC 해석

단일 ASC/BLF 파일의 sparse SQLite 검색 인덱스와 실제 `candb-engine` adapter를 제공한다.
이 문서는 첫 구현 범위이며 전체 Workspace/진단 설계의 완료 선언은 아니다.

## 사용법

출력 부모 폴더를 먼저 만든다. 파일과 report는 기본적으로 기존 파일을 덮어쓰지 않는다.

```powershell
.\scripts\build.ps1 -Test -Release
New-Item -ItemType Directory -Force .\artifacts\my-test
.\dist\canlog.exe index build .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc -o .\artifacts\my-test\motorola.sqlite --stride 128
.\dist\canlog.exe index info .\artifacts\my-test\motorola.sqlite
.\dist\canlog.exe index query .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --index .\artifacts\my-test\motorola.sqlite --start 0.1 --end 0.2 -o .\artifacts\my-test\frames.jsonl --report .\artifacts\my-test\query-report.json
.\dist\canlog.exe decode .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' -o .\artifacts\my-test\signals.jsonl --report .\artifacts\my-test\decode-report.json
.\dist\canlog.exe decode .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' --index .\artifacts\my-test\motorola.sqlite --start 0.1 --end 0.2
```

build는 전체 scan을 요구하며 필터·limit·native 보존·stdin·JSONL 인덱싱을 거부한다.
`--stride`는 semantic reader item 수로 1~1,000,000, 기본 1,024개다.
이벤트/손상 행은 기존 `--unsupported skip` / `--recover`로 명시적으로 처리한다.
치명적인 header·경계·resource limit 오류와 복구할 수 없는 상대 시간은 계속 거부한다.
query/decode는 기존 시간·채널·ID·ID 종류·방향 필터를 지원하며 `[start,end)`와 원본 순서를 유지한다.
query JSONL은 `view`와 같은 `schema_version/frame/source_location` 구조다.
output 생략 시 stdout을 사용한다. `--limit` 때문에 남은 선택 범위를 확인하지 못하면 `selection_complete=false`다.

`replay INPUT --dbc CHANNEL=PATH`도 같은 compiled decoder를 사용한다. 시간 대기·속도·반복·pause/resume/stop은 기존 replay scheduler를 따른다. JSONL은 `decode`와 같은 typed `decoded_frame`이며 `--sink console`은 메시지와 신호값을 표시한다. DBC 재생의 파일 출력은 JSONL만 지원한다. 사용법과 샘플 검증은 [replay-dbc-validation.md](replay-dbc-validation.md)에 있다.

## 인덱스 계약

- SQLite에는 `manifest`, `chunks`, `issues`만 저장한다. raw payload를 복제하지 않는다.
- manifest는 절대 원본 경로/크기/SHA-256, parser identity/limits/ID map hash, metadata, frame/issue/chunk 수와 전체 scan 완료 상태를 저장한다.
- ASC anchor는 byte offset, line/ordinal, base, time mode, 누적 시간, trigger 수와 metadata다. 무시된 `Start of measurement`도 시간 상태에 반영한다.
- BLF는 carry가 없는 안전한 container 위치와 container/ordinal/padding 상태, 이후 재독 item 수를 저장한다. 분할 객체는 이전 안전 anchor부터 읽으며 인접 chunk는 실행 중인 reader를 계속 사용한다.
- chunk의 시간 min/max로 후보를 고른 뒤 같은 parser와 Filter를 적용한다. 시간 좌표가 없는 issue의 chunk는 모든 시간 질의에 포함한다.
- 새 `.partial` DB에 bounded transaction으로 기록하고 전체 scan/재해시/연결 종료 후 generation 파일을 원자적으로 publish한다. 실패·일반 cancellation은 임시 파일을 정리하고 이전 index를 보존한다. 강제 종료는 미게시 `.partial`을 남길 수 있다.
- application ID와 schema version을 검사한다. 알 수 없는 schema, 달라진 원본/limits/ID map/parser는 기존 파일을 보존하고 재생성을 요구한다.
- input, ID map, DBC, index와 output/report의 충돌 및 hardlink를 보호한다. 원본 옆에 임의 sidecar를 만들지 않는다.

stale 검출을 위해 질의 전후 전체 원본을 SHA-256으로 읽는다. 전체 파일 I/O를 제거하는 기능은 아니며 선택 범위 밖의 parsing/decompression/decode를 줄인다.
새 generation은 DELETE journal을 사용한다. 기존 workspace의 WAL writer, migration, resume/crash checkpoint 복구는 아직 제공하지 않는다.

## DBC 계약

Git dependency와 lockfile은 `najari/candb-csharp-clone`의 `c51f18848dc4e32afd96b1a6e5bc975da70c6828`을 고정한다.
기존 `da64ad9`에서 업데이트한 API·결과·캐시 호환성 검증은 [engine-update-validation.md](engine-update-validation.md)에 정리했다.
`Document::from_bytes`, `analysis::check`, `CompiledMessage::compile/decode`, `Scratch`를 재사용하며 parser/bit decoder/multiplex evaluator를 복제하지 않는다.
Python/cantools는 독립 검증용이고 실행 파일의 의존성이 아니다.

`--dbc CHANNEL=PATH`를 반복해 지정한다. 한 채널에 여러 DB를 지정하면 raw ID가 서로 달라야 한다.
자동 matching/첫 후보 선택은 하지 않는다. 최대 assignment 16개, 각 DBC 32 MiB다.
parse findings, error급 validation, 중복 message/signal, invalid ID, 단일 CAN을 초과하는 message를 거부한다.
warning은 assignment report에 보존한다. 독립 신호 pseudo-message `0xC0000000`은 제외한다.

- Standard/Extended는 DBC bit 31로 변환하고 같은 숫자 두 종류를 구분한다. payload byte 수와 FD wire DLC를 혼동하지 않으며 padding/truncation하지 않는다.
- 명시적 CAN FD message는 Classic에 적용하지 않는다. 일반 0~8-byte DBC는 FD에도 적용할 수 있다.
- frame 상태: `decoded`, `remote`, `no_database`, `no_message`, `format_mismatch`, `length_mismatch`, `signal_error`. Remote payload는 decode하지 않는다.
- signal 상태: `valid`, `inactive`, `decode_error`, `non_finite`. inactive multiplex를 0으로 채우지 않는다.
- key는 DBC SHA-256/채널/raw message ID/definition ordinal이다. message/name/unit/enum 설명과 frame의 원본 위치를 유지한다.
- raw는 `signed` i64 / `unsigned` u64 / `float_bits`다. `raw_text`도 제공하므로 JavaScript number로 최대 u64를 반올림할 필요가 없다.
- physical은 f64 계산값이다. NaN/Infinity는 JSON null과 `non_finite`로 표현한다.
- 말미에 DB hash를 재확인한다. 변경되면 file output을 publish하지 않는다.
- selected issue 또는 미해석/오류 frame은 partial/종료 코드 3, strict 오류는 1, cancellation은 130이다. 이미 쓴 stdout은 rollback할 수 없다.

decode JSONL은 `decoded_frame` envelope이며 일반 frame JSONL 입력과 구별된다.
persistent typed cache와 workspace binding manifest는 [workspace](workspace.md)에서 제공한다.
signal CSV/Parquet export는 후속 범위다.

## 독립 검증

```powershell
.\target\verify-env\Scripts\python.exe .\scripts\verify_index_decode.py --manifest .\can_example\vector_samples\2026-10-03\manifest.json --manifest .\can_example\web_downloads\2026-10-03\manifest.json --out .\artifacts\index-decode-rerun
.\scripts\build.ps1 -TestEngine -EngineSamples C:\Users\admin\candb_csharp_clone\samples
```

Python 3.11 이상과 `cantools==43.0.2`를 사용한다. 결과는 존재하지 않는 새 폴더에 저장한다.
source hash와 full/indexed frame·위치·선택 issue 수·상태를 비교하고 Motorola/Model3/CANoe easy를 독립 decode한다.
easy의 `ENVVAR_DATA_`는 cantools oracle의 in-memory 문자열에서만 제외하며 원본 DBC는 수정하지 않는다.
upstream tests에 필요한 Vector corpus는 Git에 포함되어 있지 않다. `-EngineSamples`는 pinned source를 `target/dbc-engine-source`에 복사하고 로컬 corpus의 DBC만 추가한다. 외부 engine 및 원본 sample은 수정하지 않는다.

multi-source workspace/assignment 저장과 persistent cache는 [workspace](workspace.md)를 따른다.
source 이동 재연결은 `workspace relink`로 제공한다.
후속 단계: ISO-TP/UDS/CDD, MF4, GUI, 실제 CAN transport.
