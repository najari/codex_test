# 다중 파일 workspace와 신호 캐시

workspace는 여러 ASC·BLF의 경로, 선택적 ID map, 로그별 채널→DBC 연결을 저장한다.
각 로그의 원래 시간축과 순서를 유지하며, 서로 다른 로그의 clock을 합치거나 동기화하지 않는다.
로그와 DBC 원본은 복사하거나 수정하지 않는다. 실행 파일은 Python 없이 동작한다.

## 실제 샘플로 시작하기

프로젝트 루트의 PowerShell에서 실행한다. workspace 폴더는 새 경로여야 하며 부모 폴더는 존재해야 한다.

```powershell
New-Item -ItemType Directory -Force .\artifacts | Out-Null
.\dist\canlog.exe workspace create .\artifacts\sample_workspace --name "CAN samples"
.\dist\canlog.exe workspace add .\artifacts\sample_workspace .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --name motorola
.\dist\canlog.exe workspace bind .\artifacts\sample_workspace motorola --dbc "1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc"
.\dist\canlog.exe workspace index .\artifacts\sample_workspace motorola --stride 128
.\dist\canlog.exe workspace decode .\artifacts\sample_workspace motorola -o .\artifacts\cold.jsonl --report .\artifacts\cold-report.json
.\dist\canlog.exe workspace decode .\artifacts\sample_workspace motorola -o .\artifacts\warm.jsonl --report .\artifacts\warm-report.json
.\dist\canlog.exe workspace cache .\artifacts\sample_workspace info
```

위 샘플은 50개 frame이다. 첫 해석은 cache misses 50개, 두 번째는 hits 50개이며 JSONL은 같다.
출력과 report는 새 경로를 사용한다. 기존 JSONL 교체에는 `--overwrite`가 필요하며 report는 덮어쓰지 않는다.

BLF도 같은 workspace에 등록할 수 있다. CANoe demo의 easy DBC는 채널 1·2 모두에 연결한다.

```powershell
.\dist\canlog.exe workspace add .\artifacts\sample_workspace .\can_example\web_downloads\2026-10-03\py_canoe_demo\demo_log.blf --name easy
.\dist\canlog.exe workspace bind .\artifacts\sample_workspace easy --dbc "1=.\can_example\web_downloads\2026-10-03\py_canoe_demo\easy.dbc" --dbc "2=.\can_example\web_downloads\2026-10-03\py_canoe_demo\easy.dbc"
.\dist\canlog.exe workspace index .\artifacts\sample_workspace easy --unsupported skip
.\dist\canlog.exe workspace decode .\artifacts\sample_workspace easy --unsupported skip -o .\artifacts\easy-signals.jsonl
```

이 로그에는 unsupported record 403개가 있다. 선택 frame 1,132개를 해석해도 issue를 report에 유지하고 종료 코드 3을 반환한다.

## 명령과 연결 규칙

| 명령 | 동작 |
|---|---|
| `workspace create ROOT [--name NAME]` | 새 manifest·SQLite·indexes 폴더 생성 |
| `workspace info ROOT` | 현재 설정 JSON 조회 |
| `workspace add ROOT INPUT --name LOG [--id-map PATH]` | ASC·BLF 등록; 중복 이름/동일 파일 실체 거부 |
| `workspace relink ROOT LOG NEW_INPUT [--expected-sha256 HASH]` | 내용이 같은 파일의 새 경로로 재연결 |
| `workspace bind ROOT LOG --dbc CHANNEL=PATH ...` | 해당 로그의 저장된 연결 전체 교체; 실제 DBC 검증 |
| `workspace index ROOT LOG [--stride N]` | 전체 scan 인덱스 생성 또는 동일 원본·설정의 기존 인덱스 검증 |
| `workspace query ROOT LOG [FILTERS]` | 원본 CAN JSONL; 일치하는 인덱스가 있으면 사용 |
| `workspace decode ROOT LOG [FILTERS]` | 저장된 DBC 연결로 typed 신호 JSONL, 캐시 기본 사용 |
| `workspace decode ROOT LOG --dbc CHANNEL=PATH` | 지정한 채널의 연결만 이번 실행에서 교체 |
| `workspace decode ROOT LOG --no-cache` | 신호 캐시를 읽거나 쓰지 않고 해석 |
| `workspace cache ROOT info` | 완료/미완료 generation 수, 완료 row 수와 payload 용량 |
| `workspace cache ROOT limit MIB` | 1..16,384 MiB 설정; 오래된 완료 generation부터 제거 |
| `workspace cache ROOT clear` | 모든 신호 캐시 제거; 원본·설정·인덱스 유지 |

`query`와 `decode`는 기존 `--start/--end`, `--channel`, `--id`, `--id-kind`, `--direction`,
`--limit`, `--unsupported skip`, `--recover` 옵션을 사용한다. 시간 범위는 `[start,end)`다.
`index`는 필터와 limit 없이 전체를 검사한다. 같은 원본·설정의 기존 managed index에는 기존 stride가 유지된다.
현재 원본/설정에 맞는 index가 없으면 query/decode는 full scan하며, 자동으로 새 index를 만들지는 않는다.

한 채널에 여러 DBC를 명시할 수 있으나 ID 종류까지 같은 message ID가 중복되면 거부한다.
DBC를 자동으로 추정하거나 첫 후보를 선택하지 않는다. `bind`는 모든 연결을 교체하고
decode의 `--dbc`는 지정 채널만 대체하며 manifest를 바꾸지 않는다.
ID map도 add에서 저장할 수 있고 이번 실행의 `--id-map`으로 대체할 수 있다.

## 저장 구조와 정합성

`workspace.json` schema 2는 name, revision, cache quota와 각 로그의 name/path/id_map/bindings 및 선택적 source_identity(SHA-256·byte 수)를 가진다.
기존 schema 1도 읽으며 설정을 변경할 때 schema 2로 저장한다. SQLite schema는 기존 1을 유지한다.
add/bind/quota 변경에서 revision을 증가시키며 이전 manifest 전체 snapshot을 보관하지는 않는다.
workspace 내부 경로는 상대 경로, 외부 경로는 절대 경로로 저장한다.
상대 경로의 기준은 workspace 폴더다. Windows 절대 경로는 canonical `\\?\` 표기로 저장될 수 있다.
직접 해석과 원본 위치 문자열까지 비교하려면 같은 등록 경로 표기를 사용한다.
원본이 이동하면 자동 검색하지 않는다. `relink`에 새 파일을 명시한다. 자세한 절차는 아래를 따른다.

## 이동한 로그 재연결

```powershell
.\dist\canlog.exe workspace relink .\artifacts\sample_workspace motorola "D:\CANLogs\motorola_matrix.asc"
.\dist\canlog.exe workspace index .\artifacts\sample_workspace motorola --stride 128
.\dist\canlog.exe workspace decode .\artifacts\sample_workspace motorola
```

relink는 파일을 이동·복사하지 않고 등록 경로를 바꾼다. 현재 원본이 있으면 원본의 현재 전체 SHA-256와 byte 수를 비교한다.
원본이 없으면 add 또는 이전 relink에서 저장한 source_identity와 비교한다. 같은 크기의 다른 내용도 거부한다.
원본이 add 이후 변경되었고 그 변경본을 이미 이동했다면 등록 당시 identity와 다를 수 있다.
이 경우 임의로 identity를 덮어쓰지 않으며, 해당 변경본은 새 로그 이름으로 등록한다.
원본이 아직 있으면 변경 후 같은 내용의 복사본에 relink하여 현재 identity를 저장할 수 있다.

schema 1의 기존 로그는 저장된 identity가 없을 수 있다. 원본도 사라졌다면 이전 검증 report의 source_sha256나
`index info`의 source_sha256를 확인하여 명시한다. 새 파일만 보고 기존 파일이었다고 추정하지 않는다.

```powershell
.\dist\canlog.exe index info .\old-index.sqlite
.\dist\canlog.exe workspace relink .\old-workspace log1 "D:\CANLogs\log.asc" --expected-sha256 <이전-검증에서-확인한-64자리-SHA256>
```

이미 알려진 원본 identity가 있으면 `--expected-sha256`도 그 값과 일치해야 한다.
로그 이름, DBC·ID-map 연결과 캐시 quota를 유지하고, 실제 경로/identity가 변경될 때만 manifest revision을 올린다.
동일 경로·identity로 반복 relink하면 revision을 바꾸지 않는다. 상대 경로는 기존과 같이 workspace 내부에만 저장한다.

경로가 바뀌면 기존 index와 signal cache의 key가 달라진다. relink는 이들을 복사하거나 재라벨링하지 않는다.
첫 query/decode는 full scan이고 신호 cache는 misses로 채워지며, explicit index 후에는 새 index를 사용한다.
과거 경로의 cache/index는 보존하며 cache quota/clear와 기존 index 관리 규칙을 적용한다.
DBC·ID-map 파일의 경로는 그대로이므로 이 파일들도 이동했다면 DBC bind 또는 이번 실행의 ID-map 옵션으로 지정한다.
workspace 설정·DB·managed index 및 다른 등록 로그의 hardlink 실체로는 재연결할 수 없다.

`workspace.db`는 SQLite WAL이며 derived source revision catalog와 신호 cache를 담는다.
`indexes/<content-key>.sqlite`는 [sparse index](index-dbc.md)다.
인덱스는 원본 경로·내용 SHA-256·parser 설정으로 구분하고, explicit index 명령에서 catalog에 기록한다.
이전 인덱스 파일은 남으므로 index 전체 용량 관리/GC는 후속 범위다.

신호 cache의 key에는 원본 canonical 경로·전체 내용 SHA-256, parser/limits/ID-map SHA-256,
DBC 내용 SHA-256와 채널, 고정 Rust engine revision, adapter version,
unsupported/recover 정책, 원본 순서 시간축 규칙이 포함된다.
시간/ID 필터는 row key에 포함하지 않으며 해당 검색에서 실제 해석한 frame ordinal만 저장한다.
같은 frame을 다른 범위로 검색하면 재사용할 수 있다.

cached row는 해석 상태와 typed 신호 값·메시지/DB 식별자를 저장한다. 원본 frame payload를 복제하지 않는다.
frame과 provenance는 매번 원본 reader에서 얻고, issue도 선택 범위에서 다시 검사한다.
raw i64/u64, float raw bits와 raw_text를 유지하며 physical f64는 JSON round-trip에서 정확히 복원한다.
row checksum과 schema를 검사하며 손상된 cache는 오류로 보고한다. `cache clear` 후 다시 해석할 수 있다.

실행 중 row는 `building` 상태이며 재사용하지 않는다. 원본·DBC·ID map 내용 재확인과 출력 flush가
성공한 뒤 `complete`로 바꾼다. 정상 error/cancel unwinding은 해당 building generation을 제거한다.
프로세스 강제 종료 후 남은 building은 lookup에서 제외하며 `cache clear`로 제거한다.
generation quality는 요청 범위의 clean/degraded이고 limit으로 검사가 중단되면 unknown이다.
다음 검색은 캐시 적중과 관계없이 issue/완료 상태를 다시 판단한다.

기본 quota는 512 MiB이며 typed JSON payload byte 합계에 적용한다.
batch는 최대 128 rows/2 MiB, 단일 row는 최대 2 MiB다. LRU eviction은 완료 generation 단위다.
현재 building이 quota를 채우면 추가 row 저장을 건너뛰고 해석/출력은 계속한다.
용량 축소는 active building을 제거하지 않는다. SQLite overhead, 보고서 metadata와 sparse index 용량은 quota 밖이다.
`clear`는 SQLite 공간을 재사용 가능하게 만들며 DB 파일을 즉시 축소하는 VACUUM은 하지 않는다.

원본·DBC·ID map·manifest·SQLite 및 WAL/SHM·managed index를 output/report로 덮어쓰는 동작을 거부한다.
알 수 없는 미래 workspace schema도 수정하지 않고 거부한다.
매 실행 전체 source hash와 DBC 검증 비용이 있으므로 캐시 적중이 무조건 더 빠르다는 보장은 없다.
파일 출력 게시 실패와 유효한 해석 cache 완료는 독립적이며 이미 출력한 stdout은 rollback할 수 없다.

manifest 상한은 1 MiB/4,096 logs이며 로그 이름은 128 UTF-8 bytes 이내의 문자·숫자·`_-.`이다.
DBC assignment 상한 16개/각 파일 32 MiB와 parser resource limit을 그대로 적용한다.
resume/crash scan 복구, signal CSV/Parquet, multi-clock merge,
ISO-TP/UDS/CDD, MF4, GUI와 실제 CAN transport는 후속 작업이다.

## 검증 재현

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_workspace.py --out .\artifacts\workspace-rerun
.\target\verify-env\Scripts\python.exe .\scripts\verify_relink.py --out .\artifacts\relink-rerun
```

Python 3.11 이상과 `cantools==43.0.2`는 독립 비교용이다. 새 output 폴더를 지정한다.
증거와 결과는 [workspace 검증](workspace-validation.md) 및 [재연결 검증](relink-validation.md)에 정리했다.
