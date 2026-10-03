# ASC + DBC + CDD를 한 CLI에서 사용하기

분석은 아래 `kwp` 예제, 시간에 맞춰 재생하려면 [replay 예제](replay.md)를 사용한다. 두 경로 모두 실제 DBC 메시지와 CDD 진단 필드를 해석한다.

앞서 정리한 기존 Vector CANSystemDemo 샘플을 그대로 읽는다. 생성한 ASC나 복제한 DBC/CDD가 아니다. CDD 기능을 포함해 빌드한 뒤 프로젝트 루트 PowerShell에서 실행한다.

```powershell
.\scripts\build.ps1 -Cdd -Release
$s = '.\can_example\vector_samples\2026-10-03\groups\CANoe_13.0.172\CAN\CANSystemDemo'

.\dist\canlog.exe kwp "$s\asc\Logging\ComfortDiagData.asc" `
  --dbc "1=$s\dbc\CANdb\Comfort.dbc" `
  --cdd "$s\cdd\CDD\CANSystemDoor.cdd" `
  --ecu Any_ECU_example --variant COMMON_DIAGNOSTICS --allow-experimental `
  --routes .\examples\asc-dbc-cdd\comfort.routes.json `
  --policy .\examples\asc-dbc-cdd\comfort.policy.json `
  -o comfort-combined.jsonl --report comfort-combined.report.json
```

Engine 진단 로그도 같은 방식으로 실행한다.

```powershell
.\dist\canlog.exe kwp "$s\asc\Logging\EngineDiagData.asc" `
  --dbc "2=$s\dbc\CANdb\PowerTrain.dbc" `
  --cdd "$s\cdd\CDD\CANSystem.cdd" `
  --ecu Any_ECU_example --variant COMMON_DIAGNOSTICS --allow-experimental `
  --routes .\examples\asc-dbc-cdd\engine.routes.json `
  --policy .\examples\asc-dbc-cdd\engine.policy.json `
  -o engine-combined.jsonl --report engine-combined.report.json
```

같은 출력 파일에 DBC `decoded_frame`, ISO-TP `payload`, CDD를 포함한 `kwp_transaction`이 기록된다. frame의 `record.location.ordinal`과 transaction의 `request/response.data_locations`로 같은 원본 프레임을 연결한다. 전체 결과의 `timeline_key`는 공통이며 report는 `dbc_counts`, `kwp_counts`, `cdd_counts`를 따로 집계한다. 출력 경로가 이미 있으면 새 경로를 지정한다.

이 DBC의 진단 transport 메시지에는 신호가 없으므로 DBC 메시지 이름과 CDD 식별값을 확인한다. Comfort에는 식별 번호 `9877`, `5433`, `2000`, `8888`과 diagnostic identification `2`가 있다. Engine에는 serial number `3141528`이 있다. legacy 미정의 요청·CDD 경고, Engine 응답 누락·빈 DTC proxy 때문에 종료 코드 **3(부분 성공)**이 예상된다.

위 두 CLI를 실행하고 파일까지 저장하는 편의 스크립트:

```powershell
.\examples\asc-dbc-cdd\run.ps1
.\examples\asc-dbc-cdd\run.ps1 -Sample comfort
.\examples\asc-dbc-cdd\run.ps1 -Sample engine
```

기본 출력은 프로젝트의 새 `artifacts/asc-dbc-cdd_<timestamp>_<suffix>` 폴더다. `-OutputDirectory`와 `-Exe`로 경로를 지정할 수 있다. 기존 출력과 report는 보호한다. 스크립트의 성공은 분석 실행 성공을 뜻하며 원본의 부분 품질을 complete로 변경하지 않는다.

`--cdd`는 CDD assignment가 없는 **단일 route policy**의 편의 옵션이다. 여러 ECU/route에서는 policy에 각 CDD를 지정하고 `--dbc`를 반복한다. CLI `--cdd`와 policy의 기존 CDD assignment를 동시에 지정하면 오류다. DBC와 CLI CDD path는 현재 작업 폴더 기준, policy 내부 CDD path는 policy 폴더 기준이다. `uds` 명령에도 동일 옵션이 있지만 이 기존 샘플의 프로토콜은 KWP다.

CDD 옵션을 생략하면 ISO-TP raw `payload.data_hex`와 DBC frame 결과를 출력한다. KWP 서비스 식별·transaction 연결은 CDD가 지정된 경로에서만 수행한다. 내부 KWP profile은 사용하지 않는다.
