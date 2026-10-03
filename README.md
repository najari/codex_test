# CANLOG

Rust 기반 파일 기록·재생 CLI와 재사용 가능한 라이브러리다. ASC·BLF·JSONL을 스트리밍으로 읽고, 새 로그 기록·변환·필터링과 시간 기반 재생을 제공한다. 현재는 파일 기반이며 물리 CAN 인터페이스 송수신은 포함하지 않는다.

## 빌드와 실행

Windows에는 Rust MSVC toolchain 1.88 이상, Visual Studio C++ Build Tools, Windows SDK가 필요하다. 실행 파일은 Python이나 CAN 장비 SDK 없이 실행한다.

```powershell
.\scripts\build.ps1 -Test -Release
.\dist\canlog.exe --help
.\dist\canlog.exe record --input .\can_example\asf\ComfortDiagData.asc -o .\recorded.blf --sync --allow-loss field:source-metadata --report .\record-report.json
.\dist\canlog.exe replay .\recorded.blf --speed 2
.\dist\canlog.exe replay .\recorded.blf --no-wait -o .\replayed.asc --report .\replay-report.json
```

빌드 스크립트는 현재 프로세스에서 Rust PATH와 Visual C++ 환경을 설정하고 release 실행 파일을 `dist/`에 복사한다. `Cargo.lock`을 사용하며 변경 사항을 자동 커밋하거나 push하지 않는다.

위 샘플의 `Don Nov ...` date는 지원하는 영어 날짜 문법으로 해석되지 않아 BLF origin을 unknown으로 기록한다. 예제는 이 metadata 손실을 명시 허용하며 종료 코드 3과 report를 남긴다. CAN frame의 timestamp와 payload는 유지한다.

## 명령

| 명령 | 기능 |
|---|---|
| `info INPUT` / `info INPUT --scan` | header metadata / 실제 프레임·issue 통계 |
| `stats INPUT` | 채널·ID 종류·방향·frame 종류별 통계 |
| `view INPUT` | JSONL 프레임 및 원본 위치, 기본 100개 |
| `convert INPUT OUTPUT` | 시간 대기 없는 ASC·BLF·JSONL·CSV 출력 |
| `filter INPUT -o OUTPUT` | ID·채널·방향·시간 구간 선택 후 기록 |
| `export INPUT -o OUTPUT --format jsonl` | 버전 있는 frame stream 출력; CSV도 지원 |
| `record --input INPUT -o OUTPUT` | 파일 또는 JSONL stdin을 새 로그로 기록 |
| `replay INPUT` | 기본 1배속·1회, JSONL stdout으로 재생 |

`INPUT=-`는 `--input-format jsonl`과 함께 사용한다. ASC·BLF·JSONL은 입력/출력, CSV는 출력만 지원한다. 인식되지 않는 header는 명시적 input format이 필요하다. 형식 지원 범위는 [지원 문서](docs/support.md)에 정리했다.

```powershell
.\dist\canlog.exe filter .\recorded.blf -o .\filtered.asc --id 0x600-0x700 --channel 1 --start 1s --end 2s
.\dist\canlog.exe view .\recorded.blf --unlimited
.\dist\canlog.exe replay .\recorded.blf --speed 2 --repeat 3 --repeat-gap 0.1s --sink console
.\dist\canlog.exe replay .\recorded.blf --control-stdin
```

ID는 decimal 또는 `0x` hexadecimal로 입력한다. `--id-kind standard|extended`로 같은 숫자의 서로 다른 ID 종류를 구분한다. 시간 구간은 `[start, end)`이고, 소수 초는 나노초 9자리까지 받는다. `--limit`은 선택된 프레임 수를 제한하며 `view --unlimited`와 함께 사용할 수 없다.

배속은 실행 대기 간격만 바꾼다. 새 파일의 timestamp는 원본 offset을 유지한다. 첫 선택 프레임은 즉시 재생하며, 반복은 선택 프레임의 시간 span과 `--repeat-gap`으로 회차 offset을 계산한다. 기본 gap은 0이고 반복 경계의 동일 timestamp를 허용한다.

`--control-stdin` 모드에서 한 줄씩 `pause`, `resume`, `stop`을 입력한다. pause 시간은 재생 deadline에 반영된다. 일반 실행은 Ctrl+C로 취소한다. 시간 역행은 기본 오류이며, `--on-regression immediate`를 명시하면 원본 순서에서 역행 프레임을 즉시 내보낸다.

## 미지원 기록과 출력 보호

기본값은 strict parsing, unsupported error, loss reject다. 시스템 변수·LIN·MOST·Ethernet·숫자 ID 없는 symbolic CAN 행과 CAN error는 issue로 보고한다. `--unsupported skip`은 읽기를 계속하는 선택이며 파일 재작성의 손실 허용과는 별개다.

```powershell
.\dist\canlog.exe stats .\can_example\blf\Easy.blf --unsupported skip --report .\easy-scan.json
.\dist\canlog.exe record --input .\can_example\blf\Easy.blf -o .\easy-can.blf --unsupported skip --allow-loss unsupported-record --report .\easy-record.json
```

허용된 부분 성공은 종료 코드 3이다. `--recover`도 손실을 허용하지 않으며, 구조 경계·I/O·자원 상한 오류는 복구하지 않는다. 허용할 손실은 다음 category를 반복 지정한다.

| Category | 의미 |
|---|---|
| `unsupported-record` | 미지원 event/object, unresolved ID, CAN error의 출력 제외 |
| `corrupted-region` | 복구 가능한 손상 행 또는 frame 제외 |
| `field:format-metadata` | frame duration, bit count, symbolic name 등의 부가 필드 제외 |
| `field:source-metadata` | JSONL/CSV의 로그 date, 재작성의 입력 provenance, 미해석/서브밀리초 date origin 제외 |
| `field:direction` | ASC/BLF에 표현할 수 없는 Unknown 방향을 명시 허용 후 Rx로 변환 |

기존 출력은 기본 보존한다. 교체는 `--overwrite`가 필요하고, 입력과 같은 파일·hardlink·symlink 실체는 교체할 수 없다. report는 기존 파일에 덮어쓰지 않으므로 새 경로를 지정한다. 출력은 임시 파일에 기록하고 `finish` 성공 후 게시한다. `--sync`는 게시 전에 파일 데이터를 sync한다. report는 별도로 원자적 저장되며 데이터 파일과 report를 한 트랜잭션으로 게시하는 기능은 아니다.

report에는 실제 손실·issue 종류·최대 100개의 원본 위치 예시·프레임 수·원본/출력 시간 범위·scan 완료 여부·finalized/durable/published가 있다. `frames_written`은 writer가 받아들인 수이며, flush/finish 성공과 파일 게시 여부는 별도 완료 필드로 확인한다. stdout 이미 출력된 내용은 실패 시 되돌릴 수 없고, 최종 실패 상태는 stderr와 report에 남는다. issue 예시·통계 key 수와 parser buffer에는 상한을 둔다.

종료 코드: `0` 성공, `1` 처리 실패, `2` 인자/설정 오류, `3` 허용된 부분 성공, `130` 취소.

## JSONL과 라이브러리

한 줄에 하나의 frame을 저장한다. 정수 timestamp를 사용하고 deserialization도 CAN 불변 조건을 검사한다.

```json
{"schema_version":1,"frame":{"timestamp_ns":100000001,"channel":1,"id":256,"extended":false,"direction":"rx","remote":false,"fd":false,"raw_dlc":2,"data":[170,187],"brs":false,"esi":false}}
```

`view`와 JSONL stdout replay는 선택적 `source_location`도 포함한다. 파일 기록 시 이 provenance를 전파하지 않으므로 입력에 해당 필드가 있다면 `field:source-metadata` 손실을 보고한다. 파일 JSONL export는 frame 의미를 내보내며 원본 ASC/BLF 전체를 보존하는 포맷이 아니다.

라이브러리는 `Frame`의 validated constructor, `LogReader`/`LogWriter`, `FrameRecord`/`Issue`/`Metadata`, cancellation과 Clock 기반 Scheduler, Application `validate`/`run`을 공개한다. Reader는 재생 시간 대기를 포함하지 않는다. CLI와 라이브러리는 동일한 포맷 구현을 사용한다.

## 검증 재현

```powershell
.\scripts\build.ps1 -Test -Release
python -m venv .\target\verify-env
.\target\verify-env\Scripts\python.exe -m pip install python-can==4.6.1
.\target\verify-env\Scripts\python.exe .\scripts\verify_samples.py --exe .\dist\canlog.exe
```

독립 비교용 Python은 CLI 런타임 의존성이 아니다. 스크립트는 로컬 샘플을 수정하지 않고 `artifacts/`에 checksum manifest·CAN 왕복·외부 비교·배속/제어·메모리 측정 결과를 생성한다. Windows benchmark는 peak working set을 사용한다. 로컬 샘플은 재배포 권한이 확인되지 않았으므로 Git/CI에서 제외하며, CI는 프로젝트 생성 fixture와 단위·통합 테스트를 실행한다.

[검증 결과](docs/validation.md), [원래 구현 계획](docs/implementation-plan.md), [장기 설계](docs/canlog-rs-design.md)를 참고한다.
