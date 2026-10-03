# DBC를 연결한 파일 재생

`replay`에 반복 지정 가능한 `--dbc CHANNEL=PATH`를 추가했다. 기존 `decode`의 pinned Rust DBC parser와 compiled codec를 재사용하며 재생 scheduler가 프레임을 내보내는 시점에 신호를 해석한다. 물리 CAN 장비 송수신은 기존과 같이 포함하지 않는다.

## CLI 명령

프로젝트 루트에서 다음 명령을 실행한다.

```powershell
.\dist\canlog.exe replay .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' --sink console --speed 2
.\dist\canlog.exe replay .\can_example\web_downloads\2026-10-03\py_canoe_demo\demo_log.blf --dbc '1=.\can_example\web_downloads\2026-10-03\py_canoe_demo\easy.dbc' --dbc '2=.\can_example\web_downloads\2026-10-03\py_canoe_demo\easy.dbc' --sink console --unsupported skip --on-regression immediate
```

기본 1배속이며 `--no-wait`는 대기를 생략한다. JSONL 파일에 신호를 기록하려면:

```powershell
.\dist\canlog.exe replay .\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.asc --dbc '1=.\can_example\web_downloads\2026-10-03\atemall_motorola\motorola_matrix.dbc' --no-wait -o .\motorola-replay-signals.jsonl --sync --report .\motorola-replay-report.json
```

기존 출력에는 `--overwrite`가 필요하고 report는 새 경로를 사용한다. 원본 날짜가 있는 입력을 JSONL 파일로 기록할 때는 `--allow-loss field:source-metadata`가 필요하다. 미지원 이벤트가 있으면 `--unsupported skip`과 `--allow-loss unsupported-record`를 함께 지정한다. 부가 필드 손실은 `--allow-loss field:format-metadata`를 별도로 허용한다.

## 출력 및 시간 계약

- DBC가 없으면 기존 raw frame 재생이다. DBC가 있으면 JSONL stdout/파일은 기존 `decode`와 같은 `schema_version=1`, `kind=decoded_frame`, `record`, `status`, `database_sha256`, `message`, `error`, `signals` 구조다. 분석용 출력이며 원시 frame JSONL 입력 포맷과 다르다.
- console은 원본 payload와 메시지 이름, 신호의 물리값·단위·정확한 raw 문자열·enum 설명·상태를 출력한다. 비활성 multiplex 신호는 `inactive`로 표시한다.
- 필터는 원본 시간/채널/ID/방향에 적용한다. 반복은 선택된 시간 span과 gap으로 `record.frame.timestamp_ns`만 이동한다. payload, 해석된 신호와 `record.location`은 유지한다. speed는 대기 간격만 바꾼다.
- pause/resume/stop, Ctrl+C, 시간 역행 검사, frame limit는 기존 scheduler와 선택 루프를 사용한다. report의 `scan_complete=false`는 limit로 전체 입력을 읽지 못했음을 뜻한다.
- `no_database`, `no_message`, `length_mismatch`, `format_mismatch`, `signal_error` 프레임도 출력한다. 미지원 입력 issue와 별도로 `decode_counts`에 집계하고 부분 성공 코드 3을 반환한다. remote 프레임은 `remote`이며 신호를 해석하지 않는다.
- report에는 engine revision과 DBC channel/path/SHA-256/메시지 수가 포함된다. DBC는 실행당 한 번 load/compile하고 반복마다 재사용하며 종료 전에 다시 SHA-256을 확인한다.
- 신호를 포함하는 파일 출력은 JSONL만 지원한다. ASC/BLF/CSV와 `--preserve-records`의 조합은 명시적으로 거부한다. `-o`를 사용하면 파일에 JSONL을 쓰며 `--sink`는 stdout 모드에 적용한다.
- 출력은 임시 파일에 쓰고 flush/검증 성공 후 게시한다. 실패·취소·DBC 변경은 기존 출력을 보존한다. stdout은 이미 출력된 행을 되돌릴 수 없으며 최종 상태는 stderr/report를 따른다. DBC 원본과 출력의 path/hardlink 충돌은 거부한다.

## 검증 재현

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_replay_dbc.py --out .\artifacts\replay_dbc_samples_2026-10-03_01
```

독립 비교는 cantools 43.0.2와 Python 3.11 이상을 사용한다. 실행 파일에는 Python 의존성이 없다. 결과 JSON, 명령별 stdout/stderr, 파일 출력과 report를 새 artifacts 폴더에 기록한다. 원본 파일은 수정하지 않는다.

## 2026-10-03 검증 결과

Windows MSVC에서 전체 테스트 **100개**(새 DBC 재생 테스트 10개 포함), Clippy `-D warnings`, release 빌드, Rust 1.88.0 `check --locked --all-targets`가 통과했다. 신호의 signed Motorola 값·multiplex 활성/비활성·enum·채널별 연결, 필터/반복 timestamp/원본 위치, 실제 시간 대기와 pause/resume/stop, atomic publication과 취소, DBC hardlink 보호 및 실행 중 변경 검출, 미지원 이벤트와 입력 JSONL 출처 손실 정책을 검증했다. 기존 raw replay 테스트도 통과했다.

최종 `dist/canlog.exe` SHA-256은 `8b668b40f7509eed9873dd30046ff5e7df9f2508b26aa9bb8311f9ad9774d323`이다. 최종 바이너리로 17개 CLI 명령을 실행했고, 전체 JSONL 행이 `decode`와 정확히 일치하며 JSONL 파일 출력도 stdout과 일치했다. cantools 43.0.2와 활성 신호 집합·raw 정수·물리값을 독립 비교했다.

| 샘플 | 프레임 | 비교한 유효 신호 | 미지원 입력 issue | 재생 상태 |
|---|---:|---:|---:|---|
| Motorola ASC + matrix DBC | 50 | 230 | 0 | complete |
| Model3 ASC + Battery/Drive/Thermal DBC | 5,986 | 57,154 | 47 | partial |
| CANoe demo BLF + easy DBC, 채널 1/2 | 1,132 | 2,264 | 403 | partial |
| 합계 | 7,168 | 59,648 | 450 | |

부분 성공은 `--unsupported skip`으로 제외한 event/object를 숨기지 않고 보고한 결과다. 비교한 CAN 프레임의 해석 상태는 모두 `decoded`였다. easy DBC의 `ENVVAR_DATA_`는 cantools가 거부하므로 독립 oracle의 메모리 문자열에서 해당 환경 변수 선언만 제외했다. 원본 파일은 그대로 사용하며 검사 전후 8개 로그/DBC의 SHA-256이 모두 일치했다.

실행 증거는 로컬 `artifacts/replay_dbc_samples_2026-10-03_02/results.json`, 명령별 stdout/stderr, report, `build.log`, `msrv.log`에 있다. 해당 폴더와 원본 샘플은 Git에 포함하지 않는다.
