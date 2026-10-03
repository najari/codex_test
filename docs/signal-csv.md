# DBC 신호 CSV

`decode`, `workspace decode`, `replay --dbc`의 typed DBC 결과를 streaming CSV로 내보낸다. 같은 pinned Rust compiled decoder와 기존 신호 캐시를 재사용한다. CSV 전체를 메모리에 쌓지 않는다.

## CLI

```powershell
.\dist\canlog.exe decode .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' -o .\motorola-signals.csv --report .\motorola-signals.metadata.json
.\dist\canlog.exe decode .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' --format csv --limit 10
.\dist\canlog.exe workspace decode .\artifacts\my-workspace motorola --format csv -o .\workspace-signals.csv --report .\workspace-signals.metadata.json
.\dist\canlog.exe replay .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' --sink csv --speed 2
.\dist\canlog.exe replay .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' --no-wait -o .\replayed-signals.csv --sync
```

workspace 예제는 [workspace.md](workspace.md)에 따라 `motorola`를 등록하고 DBC를 bind한 root를 사용한다. CSV는 `.csv` 확장자에서 자동 선택하거나 `--format csv`로 지정한다. stdout은 decode에서 `--format csv`, replay에서 `--sink csv`를 사용한다. `.csv` destination에 JSONL을 쓰려면 `--format jsonl`을 명시한다. 다른 임의 확장자는 decode에서 기존과 같이 JSONL로 처리한다.

Index/workspace query는 raw JSONL 전용이다. 원시 frame CSV는 `export --format csv`를 사용한다. `--sink csv`에는 `--dbc`가 필요하다. 원본 포맷 보존과 신호 분석 export는 별도 기능이므로 `--preserve-records`와 함께 사용할 수 없다.

## Schema v1

schema identity는 `canlog-signal-csv-v1`이고 header는 33개 column이다. UTF-8(BOM 없음), comma delimiter, CRLF record separator, CSV double quote escaping을 사용한다. 셀 안의 원래 줄바꿈·쉼표·따옴표·한글은 그대로 보존한다. CSV reader로 record를 읽어야 하며 단순 line split을 사용하지 않는다.

| Column 그룹 | 의미 |
|---|---|
| `schema_version`, `row_kind` | 버전 1, `signal` 또는 `frame_status` |
| `timestamp_ns`, `channel`, `id`, `extended`, `direction` | 원본/재생 offset ns, 명시 channel, numeric ID 및 standard/extended, `rx/tx/unknown` |
| `remote`, `fd`, `raw_dlc`, `data_hex`, `brs`, `esi` | 원본 CAN 의미; payload는 대문자 hex, DLC와 payload 길이를 구분 |
| `source`, `ordinal`, `line`, `container`, `object_offset` | 원본 위치; workspace는 resolved/canonical source 경로를 유지 |
| `engine_revision`, `frame_status`, `database_sha256`, `message`, `error` | 정확한 엔진/DB identity와 frame 해석 상태 |
| `signal_key`, `definition_ordinal`, `signal_name`, `signal_status` | DB/channel/message/ordinal 기반 신호 identity, `valid/inactive/non_finite/decode_error` |
| `raw_type`, `raw_value`, `raw_text`, `physical`, `unit`, `description` | `signed/unsigned/float_bits` raw 타입, 정확한 decimal raw 값/문자열, 유한 physical f64, unit, enum label |

프레임마다 decoder의 모든 signal sample을 원본 definition 순서로 한 행씩 출력한다. inactive multiplex sample도 행을 유지하며 raw/physical은 null이다. signals가 없는 remote·미지정 채널·미등록 ID·payload/format 불일치·zero-signal 메시지는 `row_kind=frame_status`의 1행을 출력하고 신호 column을 null로 둔다. 같은 이름의 신호를 서로 다른 message/channel/DBC 사이에서 합치지 않는다.

정수는 float를 경유하지 않은 decimal text다. 최대 u64와 최소 i64, timestamp/ordinal/object offset을 정확히 보존한다. float raw bits도 decimal u64이며 `raw_type=float_bits`로 식별한다. physical은 f64 round-trip decimal text이고 NaN/Infinity는 숫자로 내보내지 않는다. non-finite sample은 raw bits와 상태를 유지하고 physical을 null로 둔다. spreadsheet가 큰 정수를 자동으로 float로 바꾸지 않도록 해당 column을 text로 가져온다.

null cell은 정확히 `\N`이다. non-null 문자열이 backslash로 시작하면 앞에 backslash 1개를 추가한다. 따라서 literal `\N`은 CSV reader를 통과한 값이 `\\N`이고 null과 구분된다. 빈 문자열은 빈 문자열이며 null이 아니다. CSV reader로 unquote한 뒤 null/escape를 해제한다.

```python
import csv

def value(cell):
    if cell == "\\N":
        return None
    return cell[1:] if cell.startswith("\\\\") else cell

with open("motorola-signals.csv", encoding="utf-8", newline="") as stream:
    for row in csv.DictReader(stream):
        row = {name: value(cell) for name, cell in row.items()}
        # raw_type에 따라 raw_value를 int로, physical을 float로 읽는다.
```

timestamp는 단일 입력의 clock offset이며 UTC를 추측하지 않는다. speed는 대기 간격만 바꾸며 반복의 span/gap offset은 timestamp에 적용한다. 반복해도 source ordinal과 raw/신호 값은 유지한다. time/channel/ID/direction filter와 frame limit는 기존 의미를 유지한다. 한 frame이 여러 CSV rows를 생성할 수 있으므로 `output_rows`와 `frames_selected/frames_written`을 구분한다. header는 row count에 포함하지 않으며 빈 결과는 header만 출력한다.

## Metadata와 출력 보호

`--report *.metadata.json`을 선택적 metadata sidecar로 사용한다. 기존 report의 원본 clock/profile/date/processing/DBC assignment/issue/완료 상태와 함께 `export_schema`(encoding, null/escape, numeric/time 규칙, header), `output_rows`를 포함한다. filename suffix는 필수가 아니다. 자동 sidecar는 만들지 않는다. report와 데이터 파일은 각각 원자적으로 게시하며 한 트랜잭션으로 게시하는 기능은 없다.

CSV/JSONL projection은 cache semantic key를 바꾸지 않는다. 캐시는 정확한 typed 해석 결과를 저장하고 매번 원본 frame/위치를 다시 읽어 다른 writer로 출력한다. warm cache에서도 issue와 완료/partial 상태를 다시 평가한다.

처리 실패·DBC/source 변경·취소 시 file output은 게시하지 않고 기존 파일을 보존한다. 원본/DBC/ID map/index/workspace/SQLite와 output/report의 충돌을 보호한다. stdout은 이미 출력한 header/rows를 되돌릴 수 없으며 stderr/report의 최종 상태를 확인한다. CSV 신호 파일은 분석용 projection으로 CSV 입력 Reader나 원본 ASC/BLF round-trip 형식이 아니다.

decode/workspace decode는 기존 분석 export 정책을 따른다. replay 파일 출력은 기존 손실 허용 정책을 유지한다. 이벤트 제외에는 `--unsupported skip --allow-loss unsupported-record`, source date/provenance 손실에는 `--allow-loss field:source-metadata`, 부가 필드 손실에는 `--allow-loss field:format-metadata`가 필요하다. `no_database/no_message/length_mismatch/format_mismatch/signal_error` 및 선택 범위의 input issue는 부분 성공 코드 3으로 남긴다.

## 검증 재현

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_signal_csv.py --out .\artifacts\signal-csv-rerun
```

CSV 검증 스크립트는 Python 3.11 이상 표준 `csv` 모듈을 사용한다. Rust 테스트의 독립 CSV Reader는 dev dependency `csv=1.4.0`이며 release/runtime dependency가 아니다. 원본 샘플을 변경하지 않고 결과/명령별 stdout/stderr를 새 artifacts 폴더에 저장한다.

## 2026-10-03 검증 결과

Windows MSVC에서 전체 테스트 **108개**(신호 CSV 8개 포함), Clippy `-D warnings`, release 빌드, Rust 1.88.0 `check --locked --all-targets`가 통과했다. 독립 Rust CSV Reader로 quoting/Unicode/멀티라인/null과 빈 문자열/선행 backslash, u64/i64 extrema, float raw bits/NaN, physical f64/-0.0, 상태 행, 필터/limit/index/반복, empty header, 캐시 재사용, 출력 보호/실패/취소를 확인했다. 기존 100개 테스트도 통과했다.

최종 `dist/canlog.exe` SHA-256은 `3982dc49f7e4c3ac6f810df9823cb6f6519215e4808b985ca3775270eba77705`이다. Python 표준 CSV Reader로 34개 CLI 명령을 실행해 CSV의 모든 33개 column을 typed JSONL과 대조했으며 physical f64는 정확한 bit 일치를 검사했다.

| 샘플 | 프레임 | CSV rows | 유효 신호 | inactive 신호 | input issue | CSV warm hits |
|---|---:|---:|---:|---:|---:|---:|
| Motorola ASC + matrix DBC | 50 | 230 | 230 | 0 | 0 | 50 |
| Model3 ASC + Battery/Drive/Thermal DBC | 5,986 | 57,974 | 57,154 | 820 | 47 | 5,986 |
| CANoe demo BLF + easy DBC, 채널 1/2 | 1,132 | 2,264 | 2,264 | 0 | 403 | 1,132 |
| 합계 | 7,168 | 60,468 | 59,648 | 820 | 450 | 7,168 |

CSV 파일/직접 stdout/replay stdout이 byte 단위로 일치했다. JSONL cold cache를 CSV warm cache에서 전부 재사용했고 warm/bypass CSV도 byte 단위로 같았다. workspace source는 Windows canonical 경로의 verbatim prefix를 유지하므로 direct source 문자열과 다른 경우 Python `samefile`로 같은 원본임을 확인한 뒤 그 source column만 alias 비교했다. 나머지 column은 그대로 일치하며 workspace 내부 cached/bypass/cold 비교는 source 문자열까지 정확히 검사했다. manifest는 변경되지 않았다.

별도로 생성한 한글·쉼표 filename의 JSONL/DBC fixture에서 u64 최대값, i64 최소값, 2^53을 넘는 timestamp와 NaN raw bits/null을 Python CSV Reader로 대조했다. 원본 로그/DBC 8개 파일은 검사 전후 SHA-256이 동일했다.

같은 최종 바이너리로 기존 `verify_replay_dbc.py`도 다시 실행해 17개 CLI 명령과 cantools 43.0.2의 59,648개 유효 신호 비교를 통과했다. Model3/easy의 partial 상태는 미지원 입력 event/object의 47/403개 집계이며 정상 CAN 프레임의 해석 상태는 모두 `decoded`였다. cantools가 거부하는 easy의 환경 변수 선언은 oracle의 메모리 문자열에서만 제외하고 원본 DBC는 그대로 사용했다.

실행 증거는 `artifacts/signal_csv_samples_2026-10-03_02/results.json`, 명령별 stdout/stderr·CSV/report, `build.log`, `msrv.log` 및 `artifacts/signal_csv_replay_oracle_2026-10-03_01/results.json`에 있다. 샘플/생성 결과는 Git에 포함하지 않는다.
