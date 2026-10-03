# CANLOG 0.1 지원 프로파일

## 포맷

| 항목 | Reader | Writer |
|---|---|---|
| ASC Classic | hex/dec, Standard/Extended, Rx/Tx, data/Remote, raw DLC 0–15 | 정수 ns offset의 hex/absolute canonical ASC |
| ASC FD | CANFD 행, 선택적 이름, BRS/ESI, DLC→길이 검증, DLC 0–15 | CANFD canonical 행, 0–64 bytes |
| ASC 시간 | absolute offset 또는 relative delta 누적; 단일 trigger block | 원본 offset 유지, timezone 추정 없음 |
| ASC 이름 ID | decimal `ID = ...` trailer가 있는 CANoe symbolic export | 숫자 ID 출력, 이름/annotations 손실 보고 |
| ASC 명시 매핑 | `--id-map` JSON의 channel/name/ID/extended 지정; Classic/FD | 정규화 CAN frame; 원본 이름 손실 보고 |
| ASC encoding | UTF-8/BOM, UTF-8 실패 시 Windows-1252 decode를 notes로 공개 | UTF-8 |
| BLF container | LOGG, bounded LOBJ, uncompressed/zlib, cross-container carry | 128 KiB 이내 버퍼의 zlib container |
| BLF object | CAN_MESSAGE 1, CAN_MESSAGE2 86, CAN_FD_MESSAGE 100, CAN_FD_MESSAGE_64 101 | CAN_MESSAGE 1 / CAN_FD_MESSAGE 100 |
| BLF object header/time | v1/v2, 10µs 또는 1ns flag, checked integer conversion | v1, 1ns timestamp |
| CAN error | ASC ErrorFrame 및 BLF 2/73/104를 별도 CanError issue로 보고 | 같은 포맷의 `--preserve-records`에서 원문 보존; 교차 포맷 의미 변환 미지원 |
| Native event/object | 보존 모드에서 bounded 원문과 위치·알려진 좌표를 전달 | ASC→ASC / BLF→BLF의 순서·기록 내용 보존 |
| JSONL | schema version 1, validated frame, 선택적 source_location | frame schema version 1 |
| CSV | 미지원 | typed frame export |

Standard ID는 0–0x7FF, Extended는 0–0x1FFFFFFF다. Classic의 DLC 9–15는 payload 8 bytes와 구분하여 원시 코드를 유지한다. Remote에는 payload가 없고 FD Remote는 거부한다. DLC와 선언/실제 payload 길이가 다르면 padding이나 truncation으로 정상화하지 않는다.

ASC ancillary 필드는 canonical FD의 8개 timing/flags 필드와 Classic의 Length/BitCount/ID triplet을 인식한다. DLC 뒤에 예상하지 못한 bytes가 있으면 손상으로 보고한다. 일부 BLF FD64 producer가 잘린 payload를 기록하는 경우에도 누락 byte를 0으로 채우지 않는다.

BLF의 원본 부가 timing/flags/object 전용 필드는 canonical CAN frame 모델의 보존 범위 밖이다. 기본 정규화 경로에서는 알려진 annotations를 손실 category로 보고한다. 보존 모드는 정상 CAN을 포함한 inner object 전체를 유지하지만 압축 container/file header는 재생성한다. ASC 보존은 기존 base와 timestamp 뒤 행을 유지하고 시간을 absolute offset으로 쓴다. byte-for-byte 파일 복원, arbitrary header/주석/trigger 구조의 완전 보존을 지원한다고 주장하지 않는다.

보존 모드는 동일 ASC/BLF 포맷의 파일 출력에만 적용한다. raw payload를 report 예시에 복제하지 않고 최대 100개 위치·종류·설명만 남긴다. 보존된 issue는 손실/부분 성공과 구분하여 `issues_preserved`에 집계하며, 모든 선택 기록을 보존하고 손실이 없으면 complete다. unknown object의 payload를 의미 분석하거나 정상 frame으로 해석했다는 의미는 아니다. 지원 프로파일로 해석한 CAN frame의 손상 판정은 원문 보존으로 우회하지 않는다.

보존 재생은 첫 known timestamp record를 scheduler 기준으로 삼으며 CAN frame 이전의 event 시간도 반영한다. unknown timestamp object는 현재 순서에서 시간 대기 없이 출력하되 pause/stop을 검사한다. 반복, 교차 포맷, ID/방향 필터, 매핑과의 동시 사용은 거부한다. 시간/채널 필터에서 좌표가 unknown인 기록은 포함한다. ASC 중간 base 변경과 여러 trigger block은 거부한다. BLF 보존에서는 원본 header start/stop 값을 유지하며 object count/size는 실제 결과로 갱신한다.

ID 매핑은 schema version 1 JSON의 channel/name 쌍이 정확히 일치할 때만 사용한다. ID 범위·extended·채널·중복·숫자로도 해석 가능한 이름을 검사하고 파일 256 KiB/4096항목/이름 128 UTF-8 bytes 상한을 둔다. 실제 사용한 문서를 report에 기록하며 반복 재생에도 같은 메모리 snapshot을 사용한다. 숫자 ID와 decimal trailer를 우선하며 누락 매핑을 추정하지 않는다.

## 오류·자원·시간

- 기본 상한: ASC/JSONL 행 64 KiB, BLF object 1 MiB, compressed/decompressed container 각각 8 MiB. CLI에는 각 상한 옵션과 설정 범위 검증이 있다.
- info scan/stats의 상세 통계 key는 4096개로 제한하고 초과 시 오류를 반환한다. 기록·재생·view는 이 부가 histogram을 수집하지 않으므로 ID 종류가 많은 입력도 스트리밍한다.
- 압축 해제 선언/실제 크기를 모두 검사한다. trusted object 경계의 최대 3 zero padding만 허용하며 손상 입력에서 무제한 magic 재검색을 하지 않는다.
- 여러 ASC trigger block은 서로 다른 clock 처리 기능이 없으므로 거부한다. 이 버전은 multi-file merge와 clock synchronization을 지원하지 않는다.
- 상대시간 프로파일은 모든 timestamped record의 delta를 누적한다. 선택되지 않은 event도 시간 누적에 반영한다. python-can 4.6.1의 relative 헤더 해석과 동일하다고 가정하지 않는다.
- 상대 delta가 손상되면 이후 시간을 복구할 수 없으므로 recover 모드에서도 중단한다. 이 프로파일의 offset/delta는 음수가 아닌 정수 ns 범위다.
- ASC의 local date와 BLF SYSTEMTIME을 timezone 없이 metadata로 보관한다. 해석 가능한 date 성분은 BLF header로 전달하지만 서브밀리초 origin이 잘리면 손실 허용을 요구한다.
- 날짜는 기존 영어 표기와 독일어 weekday `Mon/Die/Mit/Don/Fre/Sam/Son`, `Mo/Di/Mi/Do/Fr/Sa/So`, month `Mär/Mrz/Mai/Okt/Dez` 별칭을 명시적으로 지원한다. 알 수 없는 언어/시간대는 추정하지 않는다.
- origin이 없는 canonical ASC는 호환용 1970 date와 `canlog-date-origin unknown` marker를 함께 출력한다. 엔진 Reader는 이 marker를 읽어 origin을 unknown으로 유지한다.
- 알 수 없는 issue 시간/채널은 필터 밖이라고 추측하지 않는다. 숫자 ID가 없는 `Stress2` 등의 CAN 행도 추측하지 않는다.
- 재생은 첫 선택 frame 기준 monotonic deadline을 쓰며 10ms 이하 대기 단위에서 cancel/control을 검사한다. OS scheduling과 느린 출력 sink 때문에 deadline보다 늦게 출력될 수 있다. hard real-time CAN 송신 기능은 아니다.
- stdout와 파일 writer는 동기 backpressure를 사용한다. stdin 수신 queue는 1개 item, control queue는 32개 command로 제한한다. stdin의 OS read가 남아 있어도 cancellation 경로는 receive timeout으로 반환한다.

## 다음 확장

장비 transport adapter, error/event의 typed 분석과 교차 포맷 Writer, SQLite index, DBC/CDD·진단, MF4, multi-file clock/merge, GUI는 후속 확장이다. 보존 기록의 JSONL/CSV export, 반복 time shift, arbitrary ASC dialect도 별도 확장이 필요하다.

독립 비교 근거: [python-can 4.6.1 ASC 구현](https://python-can.readthedocs.io/en/4.6.1/_modules/can/io/asc.html), [BLF 구현](https://python-can.readthedocs.io/en/4.6.1/_modules/can/io/blf.html). python-can의 epoch float64 시간 양자화와 FD zero-length ASC를 remote로 표시하는 차이는 검증 report에 별도로 기록한다.
