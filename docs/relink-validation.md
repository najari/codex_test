# 원본 이동 재연결 검증

검사일: 2026-10-03, Windows MSVC. 사용법과 identity 규칙은 [workspace.md](workspace.md)를 따른다.

## 최종 빌드와 회귀 검사

`scripts/build.ps1 -Test -Release`에서 canlog **90개 테스트**가 통과했다.
기존 84개와 relink/path 보호 6개다. Clippy `-D warnings`, rustfmt와 release build가 통과했다.
Rust 1.88.0의 `cargo check --locked --all-targets`도 통과했다.
최종 `dist/canlog.exe` SHA-256은 `30ec1b495b090ea107be7cdb4c96e892de9e216fe14f259faaddc37ce0c95f52`다.

새 회귀 테스트는 다음을 확인한다.

- 원본 이동·동일 내용 재연결, DBC 연결 유지, revision 증가와 반복 실행의 불변성
- 이전 경로의 cache/index 미재사용, 새 index와 warm cache 결과 동등성
- 같은 크기의 다른 내용, 충돌/잘못된 SHA-256, 다른 로그 hardlink와 관리 파일 거부
- legacy schema 1의 원본 존재 시 현재 내용 확인, 원본 부재 시 explicit SHA 요구와 schema 2 저장
- 다른 로그의 원본 폴더 전체가 사라져도 정상 로그 output/report 보호 검사 통과
- 취소 및 미래 manifest schema 거부에서 기존 manifest 유지

manifest schema 2는 선택적 source_identity를 저장하고 schema 1도 읽는다.
설정 변경 시 2로 저장하며 SQLite/cache/index schema는 변경하지 않았다.
이전 바이너리는 새로운 manifest schema를 거부하므로 최신 실행 파일을 사용한다.
SHA 기준은 현재 원본이 있으면 현재 내용, 없으면 저장된 등록/재연결 identity다.
legacy missing source에서는 사용자가 이전에 검증한 SHA-256를 제공해야 한다.

## 실제 ASC·BLF 복사본의 이동 시험

`scripts/verify_relink.py`는 원본의 임시 복사본을 등록·index·decode한 뒤 그 파일만 다른 폴더로 이동하고
빈 이전 폴더를 제거한다. 같은 byte 수의 변조 후보 거부와 manifest 불변을 확인한 뒤 정상 파일을 relink한다.
재연결 후 full scan, 새 index, warm cache, 직접 view의 결과를 비교했다.

| 샘플 | frame | valid 신호 | inactive 신호 | 재연결 첫 misses | warm hits | issue |
|---|---:|---:|---:|---:|---:|---:|
| Motorola ASC | 50 | 230 | 0 | 50 | 50 | 0 |
| Model3 ASC + 3 DBC | 5,986 | 57,154 | 820 | 5,986 | 5,986 | 47 |
| CANoe demo BLF, easy DBC 채널 1·2 | 1,132 | 2,264 | 0 | 1,132 | 1,132 | 403 |
| 합계 | 7,168 | 59,648 | 820 | 7,168 | 7,168 | 450 |

이동 전후 전체 frame·signal row는 source 경로 문자열 변경을 제외하고 같았다.
새 경로를 통한 warm 출력과 첫 decode 출력은 경로를 포함해 정확히 같다.
JSON 비교는 physical 값에 오차를 허용하지 않았으며 inactive 상태도 포함한다.
issue 수·종류·decode count·selection 완료 여부와 complete/partial 상태도 유지했다.
Motorola는 종료 코드 0, Model3/easy는 기존 unsupported issue 때문에 종료 코드 3이다.
단위 테스트를 포함해 기존 DBC 해석은 변경하지 않았다. 이전 독립 cantools 비교는 [workspace-validation.md](workspace-validation.md)의 별도 근거다.

query 결과는 새 경로의 직접 full view와 동일하고, 첫 이동 후 decode는 index 없이 처리했다.
explicit index 이후 새 index와 cache hits 7,168개를 확인했다.
과거와 새 cache generation 6개/rows 14,336개가 남으며 building은 0개다. cache payload는 32,093,472 bytes다.
자동 cache/index 이동이나 garbage collection을 수행했다고 주장하지 않는다.

3개 로그·5개 DBC의 원래 파일 8개는 검사 전후 SHA-256가 동일하다.
실제 사용자 원본은 이동하거나 수정하지 않았고 artifact 내 복사본만 이동했다.
이 검사는 파일 경로 재연결이며 다른 로그 clock의 병합·DBC 파일 이동·중단 scan resume는 포함하지 않는다.

## 증거 및 재현

최종 35개 명령의 argv/종료 코드/시간/stdout/stderr, 각 decode report와 checksum은
`artifacts/relink_samples_2026-10-03_02/results.json`과 같은 폴더에 있다.
처음 `_01`도 통과했으며 최종 `_02`에서는 valid/inactive 신호 집계를 추가했다.
build/MSRV/help 기록은 `artifacts/relink_2026-10-03_01/`에 보관한다.
검증 script, fixture와 집계 문서는 Git 대상이며 원본/복사본·결과 데이터는 제외한다.

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_relink.py --out .\artifacts\relink-rerun
```

이 script는 Python 3.11 이상만 필요하고 추가 Python CAN 라이브러리를 사용하지 않는다.
canlog runtime에도 Python 의존성이 없다.
source 재연결 이후 남은 범위는 resume/crash scan 복구, signal CSV/Parquet,
index 용량 관리, ISO-TP/UDS/CDD, MF4, multi-file clock/merge, GUI와 장비 transport다.
