# ASC 호환성 및 추가 샘플 검증

검증일: 2026-10-03. 새 Rust release 빌드의 `dist/canlog.exe`로 검사했다.
원본 설치 샘플과 다운로드 파일은 수정하지 않았다.

## 구현 변경

- `Begin Triggerblock`/`Begin TriggerBlock`의 ASCII 대소문자와 공백 변형, End 구문의 동일한 변형을 인식한다. 단일 block과 상대시간 누적 계약은 유지하며 여러 block은 여전히 거부한다.
- CAN FD 후행 부가 필드 7개/8개를 지원한다. 선언된 payload 길이와 DLC를 검증하고 부가 필드는 bounded hex 정수로 검사한다. 7개 형식의 의미를 CAN flags로 추정하지 않는다. 같은 ASC 출력에서 원문 보존 또는 정규화 시 annotations 손실 보고가 가능하다.
- Classic 앞쪽 numeric ID와 decimal ID trailer, 반복된 ID trailer 간 충돌은 `ConflictingId`로 구분한다. strict에서는 중단, `--unsupported skip`에서는 해당 행 제외다. 전체 행의 기본 문법/payload가 유효하면 동일 ASC의 `--preserve-records`로 해석되지 않은 원문을 유지한다. 충돌 ID 중 하나를 정상 frame ID로 선택하지 않는다.
- 회귀 테스트는 case/공백과 relative timeline, 여러 clock 거부, 64-byte FD payload·BRS/ESI·원본 보존, invalid/truncated FD, ID 값·종류·중복 충돌, symbolic trailer 및 malformed 충돌 출력 보호를 검증한다.

## 빌드 검사

- `scripts/build.ps1 -Test -Release`: 통과. `dist/canlog.exe` 갱신.
- `cargo test --locked`: 55개 통과 — 단위 8개, 기존 통합 30개, 보존/매핑 12개, ASC 호환성 통합 5개.
- `cargo clippy --locked --all-targets -- -D warnings`: 통과.
- `cargo fmt --all -- --check`: 통과.

## 샘플 실행 결과

| 입력 묶음 | ASC/BLF | skip/recover 완전 읽기 | 부분 읽기 | 읽기 거부 | CAN 프레임 | 프레임 있는 파일 | 같은 포맷 원본 보존 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Vector 설치 샘플 | 59 (30/29) | 7 | 52 | 0 | 103,784 | 25 | 59 통과 |
| 웹 다운로드 샘플 | 17 (9/8) | 7 | 4 | 6 | 9,613 | 9 | 읽힌 11개 통과 |

CAN 프레임이 있는 34개 파일 모두 view/replay, record BLF→replay ASC, CSV export, ID/channel/ID-kind filter 검사를 통과했다.
엔진이 해석한 원본 frame → BLF → ASC의 모든 CAN 필드와 정수 ns timestamp를 정확히 비교했다.
생성한 ASC/BLF는 python-can 4.6.1로 별도 비교했다. 외부 BLF reader의 float epoch 시간 오차는 기존 비교기의 500ns 이내로 검사했다.
같은 포맷의 원본 보존은 70개 파일, 총 282,532개 레코드에서 source→record→replay의 content digest가 일치했다.
독립 scanner의 비교 대상은 ASC base·정수 ns·timestamp 뒤 행과 BLF 순서 있는 inner object bytes다. 주석·header·compression 전체 파일의 byte 일치를 주장하지 않는다.

각 파일에서 info, strict stats, recover/skip stats, view, 즉시 replay를 실행했다. 출력 검사는 해당 파일의 읽기가 가능할 때 수행했다.
canlog 실행은 Vector 817회 + 웹 179회 = 996회다. 읽힌 입력의 후속 작업 검사 실패와 native 보존 비교 실패는 0건이다.
Vector 원본 277개, 정리본 286개, 웹 원본/출처 증거 55개의 hash 불변을 확인했다.
CAN 프레임 0개인 36개 읽기 가능 파일의 빈 정규화 출력은 별도로 확인했으며, CAN frame 왕복 검증 건수 34개에 합산하지 않았다.

## 남아 있는 제한과 데이터 문제

- Vector의 매크로·환경/시스템 변수·LIN/MOST/Ethernet/A429 등 기록은 정상 CAN frame으로 만들지 않는다. skip에서 partial로 집계할 수 있으며 같은 원본 형식에서 보존하는 것과 의미 분석 지원은 별개다.
- 숫자가 없는 `Stress2` 같은 이름은 DBC 또는 검증된 `--id-map` 없이 추정하지 않는다. DBC/CDD 신호·진단 분석은 이번 구현에 포함하지 않는다.
- 웹 Model3 ASC의 trigger 대소문자 오류가 제거됐다. CAN 5,986개는 유지하고 `CorruptedRegion` 1개는 없어졌으며, 같은 ASC 원본 보존은 이제 통과한다. TFS 등 기타 기록 47개는 계속 UnsupportedRecord다.
- 웹 FD64 ASC는 2개 frame과 같은 포맷 원본 보존이 통과한다. 부가 필드를 정규화 출력에서 제외하는 손실은 report에 남긴다.
- 웹 powertrain/body_chassis ASC의 앞쪽 ID와 ID trailer가 충돌한다. 각각 45개/53개 `ConflictingId`를 보고하며 skip에서 CAN 0개다. 두 파일의 모든 행은 같은 ASC에서 손실 없이 원문 보존할 수 있다. 이전 빌드와 frame 수 차이 98개는 충돌 ID를 더 이상 임의 선택하지 않기 때문이다.
- 웹 BLF 6개는 계속 거부한다. 5개는 파일 크기/header 불일치, FD64 1개는 object 범위를 벗어난 extDataOffset이다. 원본을 보정하거나 누락 byte를 채워 성공시키지 않는다.
- 원본 ASC의 symbolic ID, 상대 delta, Windows-1252 dialect는 python-can의 해석 범위와 차이가 있다. 원본 외부 비교 실패/차이는 상세 결과에 남겼고, 생성된 canonical 출력의 외부 비교와 구분했다.

## 재현 및 상세 증거

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_corpus.py --manifest .\can_example\vector_samples\2026-10-03\manifest.json --out .\artifacts\vector-audit-rerun
.\target\verify-env\Scripts\python.exe .\scripts\verify_corpus.py --manifest .\can_example\web_downloads\2026-10-03\manifest.json --out .\artifacts\web-audit-rerun
```

출력 폴더는 존재하지 않는 새 경로를 지정한다. 검증 Python은 CLI 런타임 의존성이 아니다.
원본/정리본 샘플과 실제 결과는 Git에서 제외된 로컬 폴더에 보관한다.

- Vector 결과: `artifacts/vector_canlog_test_2026-10-03_01/README.md`, `results.json`.
- 웹 후속 결과: `artifacts/web_canlog_test_2026-10-03_02/README.md`, `results.json`.
- 각 결과 폴더에 996개 명령의 argv/종료 코드/시간·stdout/stderr와 실제 기록·재생·CSV·필터·보존 출력을 보관했다.
- 원래 파일 기반 검증 기록: [validation.md](validation.md). 실제 지원 계약: [support.md](support.md).

이 문서는 ASC 호환성 검사 당시의 snapshot이다. 후속 SQLite 인덱스/DBC adapter의 범위와 명령은 [index-dbc.md](index-dbc.md), 최신 검사 결과는 [index-dbc-validation.md](index-dbc-validation.md)를 따른다.
