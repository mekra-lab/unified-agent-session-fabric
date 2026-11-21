이 문서는 [영어판](../agy.md)의 번역이며, 내용이 다르면 영어판이 우선한다. 번역 기준 버전: **0.3.0**.

# 실험적 Antigravity 엔진

0.3.0은 `--engine agy`를 추가한다. 지원 등급은 **experimental**이다.
native 필드 맵의 verified 범위는 Antigravity CLI **1.5.9**와 코드에 고정한 실행 파일 SHA-256이다.
운영자는 2026-11-15에 `gemini-3.8-flash-low`로 실행 시도 8회를 기록했다.
모델 프로세스 7개와 전송 전 거절 1건이며 새 대화·resume·Unicode 입력·중단·timeout·recover를 포함한다.
[native 실측과 후속 수정](validation.md#2026-11-15-antigravity-native-실측과-후속-수정)을 참고한다.

```sh
handoff-receipts run instruction.txt --engine agy --agy /path/to/agy -C "$PWD" -o answer.md
handoff-receipts run follow-up.txt --engine agy --resume CONVERSATION_UUID -C "$PWD"
handoff-receipts inspect agy:EXECUTION_UUID --json
handoff-receipts list --engine agy --json
handoff-receipts dup agy:ATTEMPT_ID --json
handoff-receipts which agy:/absolute/path/to/instruction.txt --json
```

native CLI 설치·인증은 별도로 한다. `--agy`로 실행 파일을 지정한다.
전송 전에 실행 파일을 절대경로로 한 번 해석한다. 슬래시가 있으면 `-C`를 기준으로 삼는다.
이름만 있으면 PATH에서 찾으며, PATH의 상대 항목과 빈 항목도 `-C` 기준으로 해석한다.
해시·버전 검사·모델 실행·recover 모두 저장된 절대경로를 사용한다.
공통 `-m`, `-C`, `-e`(native `--effort`), 출력, 명시적 재시도, 관측 예산 옵션을 지원한다.
daemon, `--thread`, 다른 엔진의 명시적 전용 옵션은 전송 전에 거절한다. retry 그룹은 같은 엔진 안에서만 구성한다.

래퍼의 `--` 뒤에는 값 하나를 받는 `--mode`, `--settings`, `--print-timeout`과
값이 없는 `--disable-slash-commands`만 허용한다. 값 옵션은 `--option=VALUE` 형식도 받으며 한 토큰으로 전달한다.
어느 형식이든 `-`로 시작하는 값은 거절한다.
`-p`, `--print`, `--input-format`, `--output-format`, `--conversation`, 위치 인자 프롬프트를
비롯한 나머지 인자는 자식 실행 없이 거절한다. 도구 설정은 호출자가 선택한다.

## 입력·식별·복구

한 attempt는 UTF-8 지시 하나를 모델 프로세스 하나에 보낸다.
`--input-format stream-json --output-format stream-json`을 사용하고 `-p`는 넣지 않는다.
stdin에는 다음 NDJSON 한 줄과 EOF를 보낸다.

```json
{"event":"user","message":{"content":"the exact instruction"}}
```

JSON을 해제하면 원래 UTF-8 바이트·공백·Unicode가 복원된다. 비UTF-8 지시는 거절한다.
완전하고 유효한 result가 정확히 하나 있어야 한다. `num_turns`로 결과 개수를 판단하지 않는다.

새 대화에서는 attempt ID만 전송 전에 봉인한다. 이 자식 stdout에서 conversation ID를 받으면 즉시
체크포인트로 남긴다. init·step·result의 ID는 같아야 한다. ID가 없으면 recover도 native DB를 찾지 않는다.
조회 범위는 기록된 native 루트의 `CONVERSATION_UUID.db` 하나로 제한한다.
기본 루트는 `~/.gemini/antigravity-cli/conversations`이며 `--session-root DIRECTORY`로 바꾼다.

resume은 전송 전에 정확한 DB가 존재하고, `trajectory_meta.cascade_id`가 요청 UUID와 같으며,
`steps.idx` 최댓값·경계 레코드 해시·trajectory ID를 읽을 수 있어야 한다.
steps가 비어 있으면 경계는 -1과 빈 바이트 해시다. DB가 없거나 읽을 수 없으면 전송하지 않는다.
후속 읽기에서도 저장된 경계를 확인한다. 자식이 다른 ID를 반환하면 요청·실제 ID를 모두 남기고,
요청 대상 입력·상태는 unknown, 래퍼 exit는 2다. 재전송하지 않는다.

입력 귀속에는 완전한 자식 stdout 관측에서 보존한 `user_input.step_index`의 서로 다른 값이 정확히 하나 있어야 한다.
그 index와 같은 `idx`의 USER_INPUT 한 행을 선택하고 `user_response` UTF-8 바이트가 지시와 같은지 확인한다.
resume은 index가 저장된 경계보다 커야 한다. 같은 대화의 후속 입력은 각각의 index로 구별한다.
planner·executor 관측은 선택된 입력의 execution ID로 연결한다.
같은 index의 반복 stream update는 허용한다. 불완전한 stream, user_input의 conversation ID 누락,
무효·모호한 index는 capture와 recover 모두 native 귀속을 unknown으로 만든다.

`recover ATTEMPT_ID`는 같은 DB를 저장된/재지정된 관측 예산 안에서 다시 읽는다.
저장된 실행 파일에 `agy --version` 메타데이터 검사를 실행하고 현재 버전·해시를
`agy_version_recover` 이벤트에 기록한다. native 응답·종료 이유가 나중에 보여도 유실된 result는 unknown이다.
agy 상태가 unknown이면 run·recover의 래퍼 exit는 2다. 알려진 결과에서는 다른 비정상 자식 exit를 보존하며,
자식 exit 0인데 failed/interrupted이면 2를 반환한다. 자식의 실제 exit code는 별도 사실로 남긴다.
recover는 보존된 stream control 이벤트에서 reported 상태와 출력 해시를 다시 계산한다.
native 재조회는 observed와 보조 근거를 갱신한다. 후속 native 읽기가 실패해도 저장된 stream 이벤트로 reported를 계산한다.

## 사실·버전 한계

| 사실 | Antigravity 해석 |
|---|---|
| attempted / dispatched / process | 기존 로컬 프로세스 observed 규칙 |
| target_input_confirmed | verified 버전·해시, ID, 경계, 유일한 stream index, 정확한 native 입력이 모두 있을 때 observed |
| turn_started / completed_at | 항상 unknown |
| turn_status | 아래 status·finished_reason 판정표로 계산하며 reported |
| final_output_sha256 | 선택된 completed 결과의 `result.response` UTF-8 그대로. reported |
| model_read_or_used | 항상 unknown |

| status | finished_reason | 판정 |
|---|---|---|
| SUCCESS | 부재 또는 null | completed |
| SUCCESS | print_timeout | failed |
| SUCCESS | 그 밖 | unknown |
| ERROR | interrupted | interrupted |
| ERROR | 그 밖, 부재·null 포함 | failed |
| 그 밖 status | 아무 값 | unknown |

stream control은 reason 부재·null을 JSON null로, 미지 문자열을 `unknown`으로 구분해 저장한다.
SUCCESS response가 문자열이 아니거나 UTF-8로 인코딩되지 않으면 malformed이며 상태·출력 모두 unknown이다.
ERROR는 response가 없거나 null 또는 다른 비문자열 값이어도 위 판정표를 따른다.
민감정보를 포함한 응답은 completed 보고를 유지하고 출력을 제외한다.

native 턴 키는 `{kind: execution, value: execution_id}`이며 전체 식별은
`(engine, conversation_id, trajectory_id, execution_id)`다. 각 ID는 구별한다.
native planner response 해시는 보조 사실이며 마지막 LF를 보정하지 않는다.
NO_TOOL_CALL·USER_CANCELED는 observed 보조 종료 이유로, 성공으로 승격하지 않는다.
completed 보고와 같은 execution의 USER_CANCELED가 충돌하면 상태·최종 출력은 unknown,
exit 2이며 양쪽 증거는 유지한다. timeout/interrupted와 USER_CANCELED는 일관된 결과다.

모델 프로세스 실행 직전·직후 `agy --version`과 실행 파일 해시를 기록한다.
`version_verified=true`에는 verified before와 verified after가 필요하다.
after가 없으면 가장 최근의 verified recover 기록을 사용한다.
before·after·recover의 모든 버전·해시가 pin과 같아야 한다. 한 번 불일치하면 이후 recover가
pin과 일치해도 false다. 미검증이면 경고를 내고 유효한 stream 결과는 reported로 유지하며,
native 입력·execution/trajectory 키와 연결된 보조 사실은 unknown으로 낮춘다.
선택된 바이트 일치는 `unverified_interpretation`에 기록한다. 종료 이유는 숫자로 남기며 취소 충돌 규칙에 쓰지 않는다.
verified 버전 확장은 native 검증 뒤 코드 상수를 갱신해야 한다.
각 fingerprint에 `stat.st_dev`, `stat.st_ino`, `stat.st_size`, `stat.st_mtime_ns`,
`stat.st_ctime_ns`도 기록한다. after가 없으면 before와 가장 최근 recover의 다섯 값이 모두
존재하며 같아야 한다. 누락·차이가 있으면 unverified다. after가 있으면 기존 버전·해시 규칙을 사용한다.

stream-json 입력의 timeout 실측에서는 SUCCESS, `finished_reason=print_timeout`, 빈 response,
자식 exit 0이 나왔다. 수정된 규칙은 verified/unverified 모두 failed/reported와 래퍼 exit 2를 예상한다.
수정 후 native 재실측에서 failed/reported와 래퍼 exit 2를 확인했다.

## 선별 증거·개인정보

자식 종료 후 SQLite를 `file:...?mode=ro`로 열고 짧은 단일 read transaction에서
`trajectory_meta`, `steps`, `executor_metadata`의 필요한 열만 메모리로 읽고 닫는다.
busy·미완성은 공통 관측 예산 안에서 다시 읽는다. DB 사본·checkpoint·journal 변경·immutable 연결은 없다.
보조 필드 제외 경고와 executor 대기를 독립적으로 기록한다.
planner 필드를 제외한 경우에도 executor가 아직 없으면 예산 안에서 재조회를 계속한다.

`agy-selected-fields-v1`은 type/status, execution ID, 정확한 입력, 필요한 planner response,
executor 종료 이유의 **원래 leaf field wire bytes**만 보존한다.
각 조각에는 table·PK·column·숫자 field path·셀 내부 `[cell_start,cell_end)` 범위·셀 길이와 해시·
조각 해시·profile·`wire_base64`를 남긴다. 감싸는 메시지 bytes는 제외한다.
파생 step/executor 값은 `kind=derived_projection`으로 구분한다.
필수 필드 중복·wire type 오류·SQL/protobuf 불일치는 관련 native 판정을 unknown으로 만든다.

전체 BLOB, gen_metadata, 설정·환경, tool payload, protobuf Any는 보존하지 않는다.
선택된 decoded 문자열에도 기존 개인정보 검사를 적용한다.
원시 stdout/stderr, init 계정 데이터, 자유 형식 오류 본문은 저장하지 않는다.
receipt 필터링이 native CLI 자체 이력을 바꾸지는 않는다.

오프라인 inspect/verify/export는 선별 wire 조각과 파생 판정의 일관성만 확인한다.
원래 셀 없이 원본 소속이나 제외한 상위 필드를 독립 인증하지 못한다. 셀 해시는 서명이 아니다.
[개인정보 보호](privacy.md), [검증 기록](validation.md)을 참고한다.
