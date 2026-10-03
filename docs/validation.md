# CANLOG 구현 검증 결과

검증일: 2026-10-03. 파일 기반 recording/replay 0.1의 지원 프로파일을 검증했다.

## 실행 환경과 빌드

- OS: `Windows-11-10.0.26200-SP0`
- CPU: 12th Gen Intel(R) Core(TM) i5-1240P; RAM 약 16 GiB.
- Compiler: `rustc 1.98.1 (48a229cea 2026-09-01)`; Rust MSVC release build.
- `cargo test --locked`: 37개 통과 (단위 7, 통합 30).
- `cargo clippy --locked --all-targets -- -D warnings`, `cargo fmt --all -- --check`: 통과.
- 최소 Rust 1.88.0: `cargo +1.88.0 check --locked --all-targets` 통과.
- GitHub CI workflow는 Windows/Linux 및 MSRV gate를 추가했다. 원격 CI 실행 성공은 이 로컬 결과에 포함하지 않는다.
- `dist/canlog.exe` SHA-256: `139dbff02ce94a1dafec0c165995686fed71b0569478c2d60873588a2cc245c7`

## 샘플·독립 비교

- ASC 21개 + BLF 22개, 총 43개를 전수 검사했다.
- CAN 데이터/Remote 프레임 102,049개를 source → recorded BLF → replayed ASC에서 정수 ns와 전체 CAN 필드로 비교해 일치했다.
- issue 합계: `{'UnsupportedRecord': 144698, 'UnresolvedId': 443, 'CanError': 1700}`. issue는 정상 프레임으로 바꾸지 않았다.
- 재작성은 skip/recover와 이름 있는 손실 허용을 명시했고, 각 파일의 실제 손실을 report로 남겼다. 부분 성공은 종료 코드 3이다.
- CAN 프레임 0개인 event/비CAN 파일은 0개 결과와 issue를 검증했다. 파일 전체의 모든 기록을 보존했다는 의미가 아니다.
- 독립 reader: python-can 4.6.1. 원본 BLF의 CAN frame 수·ID·channel·direction·payload·FD flags 및 생성된 ASC/BLF를 비교했다.
- 원본 ASC의 symbolic ID/relative delta 변종은 외부 reader와 dialect가 달라 canonical 숫자 ASC/BLF를 비교했다. 원본 relative 시간 해석의 외부 동등성을 주장하지 않는다.
- 별도 합성 profile은 FD DLC 0–15/길이 0–64, BRS/ESI, Classic raw DLC 0–15, Remote, 최대 ID/channel, 독립 v2/FD64를 검증했다.
- python-can ASC의 DLC=0 FD를 remote로 표시하는 차이를 report에 명시했다. 원본 BLF epoch float64 시간 오차 허용은 500ns이고, Rust 내부 왕복 시간은 정수 ns의 정확한 일치다.
- 외부 FD 비교로 CAN_FD_MESSAGE의 flags/valid-bytes offset 오류를 발견해 수정했으며 독립 byte-layout 회귀 테스트를 추가했다.
- [Checksum/결과 manifest](sample-manifest.json), [지원·손실 범위](support.md). 원본 샘플은 수정·복사·Git 추가하지 않았다.

| 샘플 | CAN frames | Issues | 상태 |
|---|---:|---:|---|
| `can_example/asf/Action.asc` | 0 | 28 | partial |
| `can_example/asf/can1.asc` | 0 | 0 | complete |
| `can_example/asf/can2.asc` | 0 | 1,776 | partial |
| `can_example/asf/CANOE.ASC` | 146 | 636 | partial |
| `can_example/asf/ComfortDiagData.asc` | 20 | 0 | complete |
| `can_example/asf/Demo.asc` | 0 | 3 | partial |
| `can_example/asf/Demo1.asc` | 0 | 129 | partial |
| `can_example/asf/DemoMakro.asc` | 0 | 302 | partial |
| `can_example/asf/DiagDataA.asc` | 18 | 0 | complete |
| `can_example/asf/DoorFL_ReadEcuRelatedInfo.asc` | 0 | 5 | partial |
| `can_example/asf/EasyLog.asc` | 0 | 446 | partial |
| `can_example/asf/EngineDiagData.asc` | 36 | 0 | complete |
| `can_example/asf/Logging_CAN1.asc` | 783 | 2,414 | partial |
| `can_example/asf/Logging_CAN2.asc` | 643 | 2,399 | partial |
| `can_example/asf/Macro.asc` | 0 | 11 | partial |
| `can_example/asf/Macro1.asc` | 0 | 496 | partial |
| `can_example/asf/MacroDiagnostics.asc` | 0 | 7 | partial |
| `can_example/asf/Makro.asc` | 0 | 26 | partial |
| `can_example/asf/MOSTStreamPlayer.asc` | 0 | 2,016 | partial |
| `can_example/asf/RemoteKey.asc` | 0 | 8 | partial |
| `can_example/asf/ReplayEnvSV.asc` | 0 | 38 | partial |
| `can_example/blf/A429MainDemoOffline.blf` | 0 | 13,072 | partial |
| `can_example/blf/ActiveX.blf` | 732 | 72 | partial |
| `can_example/blf/CANOE.blf` | 2,367 | 281 | partial |
| `can_example/blf/CANSystem_CAN1.blf` | 2,635 | 482 | partial |
| `can_example/blf/CANSystem_CAN2.blf` | 2,064 | 482 | partial |
| `can_example/blf/CANWIN.blf` | 0 | 1,267 | partial |
| `can_example/blf/Debug.blf` | 802 | 30 | partial |
| `can_example/blf/DemoLog.blf` | 367 | 6,102 | partial |
| `can_example/blf/DemoLog_MOST_only.blf` | 0 | 3,925 | partial |
| `can_example/blf/Easy.blf` | 388 | 38 | partial |
| `can_example/blf/Ethernet.blf` | 0 | 77,608 | partial |
| `can_example/blf/FuelSystem_Sensors.blf` | 0 | 1,160 | partial |
| `can_example/blf/LINSystem_1.blf` | 0 | 7,330 | partial |
| `can_example/blf/LINSystem_2.blf` | 0 | 6,558 | partial |
| `can_example/blf/Log_001.blf` | 40,206 | 1,307 | partial |
| `can_example/blf/Logging.blf` | 826 | 2,875 | partial |
| `can_example/blf/Logging_CAN2.blf` | 1,581 | 25 | partial |
| `can_example/blf/SampleLog.blf` | 0 | 236 | partial |
| `can_example/blf/Stress.blf` | 18,360 | 6,682 | partial |
| `can_example/blf/test.blf` | 1,000 | 668 | partial |
| `can_example/blf/Truck.blf` | 29,075 | 5,408 | partial |
| `can_example/blf/VideoGps.blf` | 0 | 493 | partial |

## 실제 재생과 자원 측정

3-frame, 0.4s 구간으로 실제 CLI를 실행했다. 배속 전후 timestamp는 원본 값을 유지했다.

| 모드 | 실제 경과 시간 |
|---|---:|
| 1배속 | 0.452s |
| 2배속 | 0.260s |
| no-wait | 0.039s |
| pause-resume | 0.550s |

pause/resume의 0.15s 정지를 반영했다. 3600s frame 간격에서도 stop 제어로 1s 이내 취소와 임시 출력 폐기를 통합 테스트했다.

메모리 측정은 Windows process peak working set이다. 첫 frame 지연은 프로세스 시작/CLI 종료 비용을 포함한 limit=1 측정이다. 단일 CAN ID 합성 fixture, OS cache를 비우지 않은 로컬 실행이며 실차 일반 성능 보장이 아니다.

| Format | Frames | 입력 bytes | 시간 | Frames/s | Peak working set | 첫 frame 지연 |
|---|---:|---:|---:|---:|---:|---:|
| ASC | 100,000 | 4,390,029 | 0.347s | 287,967 | 5.29 MiB | 36.3ms |
| BLF | 100,000 | 386,696 | 0.239s | 418,207 | 5.75 MiB | 36.5ms |
| ASC | 1,000,000 | 44,890,029 | 2.814s | 355,418 | 5.30 MiB | 28.0ms |
| BLF | 1,000,000 | 3,865,290 | 1.575s | 634,896 | 5.76 MiB | 40.6ms |

10배 frame 수에서도 ASC/BLF 각각 64 MiB 미만과 16 MiB 이내의 peak 증가 조건을 통과했다. 이 수치는 위 fixture의 scan 결과이고, 설정된 최대 container/object를 가진 모든 입력의 동일 RSS를 보장하지 않는다.

## 원자적 출력·실패 검증

기존 출력·동일 파일·hardlink 보호, 게시 직전 noclobber 경쟁, malformed JSONL/ASC, DLC/payload 불일치, oversized 행/container, 압축 해제 상한, BLF EOF/carry, flush I/O 실패, 역행 시간·반복, 원본 위치와 date 정밀도 손실을 테스트했다.
ID 종류 5000개를 가진 입력도 기록·재생했다. 상세 통계의 key 상한은 info/stats에만 적용하여 recording/replay를 막지 않는다.

## 재현과 증거

```powershell
.\scripts\build.ps1 -Test -Release
.\target\verify-env\Scripts\python.exe .\scripts\verify_samples.py --exe .\dist\canlog.exe
```

최종 상세 실행 결과: `artifacts/verification-1790993265991884700/verification.json`. 원본 checksum, actual loss, 독립 비교의 차이와 benchmark 입력 checksum을 포함한다.

장비 송수신, MF4, SQLite/DBC/CDD/진단, arbitrary ASC/BLF dialect 및 raw error/event 재작성은 이번 검증 범위 밖이다.
