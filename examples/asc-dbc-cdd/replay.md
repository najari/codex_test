# ASC·DBC·CDD replay 예제

기존 Vector Comfort 로그를 시간 순서대로 재생하면서 DBC 메시지와 CDD 진단 필드를 해석한다. 물리 CAN 장비 송신 없이 파일을 읽는 replay다. CDD 기능을 포함해 빌드한 뒤 프로젝트 루트 PowerShell에서 실행한다.

```powershell
.\scripts\build.ps1 -Cdd -Release
$s = '.\can_example\vector_samples\2026-10-03\groups\CANoe_13.0.172\CAN\CANSystemDemo'

.\dist\canlog.exe replay "$s\asc\Logging\ComfortDiagData.asc" `
  --dbc "1=$s\dbc\CANdb\Comfort.dbc" `
  --cdd "$s\cdd\CDD\CANSystemDoor.cdd" `
  --ecu Any_ECU_example --variant COMMON_DIAGNOSTICS --allow-experimental `
  --protocol kwp2000-vector `
  --routes .\examples\asc-dbc-cdd\comfort.routes.json `
  --policy .\examples\asc-dbc-cdd\comfort.policy.json `
  --speed 2 --sink console
```

`--speed 2`는 2배속이며 첫 프레임부터 마지막 프레임까지 약 6초다. `--no-wait`를 추가하면 시간 대기 없이 실행한다. 벽시계 대기만 배속으로 변경하며 ISO-TP·진단 timeout은 원본 timestamp로 판단한다. 같은 scanner/reassembler/matcher/CDD 엔진을 분석과 재생에서 사용한다. 콘솔에는 `Diag_Request`/`Diag_Response`, KWP 요청·응답, CDD 필드 값과 경고가 나타난다. 이 원본 DBC transport 메시지에는 신호가 없으며 진단 값은 CDD가 해석한다.

전체 원본 payload와 field key는 실제 콘솔·JSONL에 표시한다. Comfort의 식별 번호는 `9877`, `5433`, `2000`, `8888`이며 diagnostic identification은 `2`다. 미정의 session/reset 요청 2개와 DTC 값 설명 경고 1개는 그대로 표시하므로 종료 코드 **3(부분 성공)**이 예상된다.

기본 2배속 콘솔 예제와 대기 없는 JSONL 저장은 다음 스크립트로도 실행한다. 결과·report는 새 `artifacts/asc-dbc-cdd-replay_<timestamp>_<suffix>` 폴더에 저장한다.

```powershell
.\examples\asc-dbc-cdd\replay.ps1
.\examples\asc-dbc-cdd\replay.ps1 -NoWait -Sink jsonl
.\examples\asc-dbc-cdd\replay.ps1 -Sample engine -Speed 4
```

직접 CLI에서 저장하려면 `--sink console` 대신 `--sink jsonl -o comfort-replay.jsonl --report comfort-replay.report.json`을 사용한다. 콘솔은 stdout, 파일은 JSONL을 지원하며 둘을 동시에 지정하면 오류다. JSONL은 분석과 같은 원본 위치·timestamp·timeline/result identity를 유지하고 report의 `replay`에 배속·대기·sink 설정을 기록한다.

콘솔 재생은 데이터 파일이 없으므로 `published=false`가 정상이다. `scan_complete=true`는 로그를 끝까지 처리했음을 뜻한다. JSONL 저장 모드는 정상 완료 시 `published=true`다.

`--control-stdin`으로 `pause`, `resume`, `stop`을 입력할 수 있으며 Ctrl+C도 지원한다. 취소·정의 파일 변경·파싱 실패 시 임시 JSONL은 게시하지 않는다. 진단 replay는 현재 전체 로그 **1회**만 지원한다. 필터, 반복/gap, CSV·ASC·BLF 출력은 거부한다. 일반 raw/DBC replay의 기존 필터·반복 기능은 계속 사용할 수 있다. 여러 CDD는 기존 policy assignments를 사용하고 프로토콜은 `--protocol uds2013` 또는 `--protocol kwp2000-vector`로 명시한다.

CDD 옵션을 생략하면 DBC frame과 ISO-TP raw payload를 시간에 맞춰 출력한다. CDD를 지정한 경로에서만 native KWP transaction과 필드를 해석한다.
