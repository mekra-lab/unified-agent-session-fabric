이 문서는 [영어판](../reference.md)의 번역이며, 내용이 다르면 영어판이 우선한다. 번역 기준 버전: **0.3.0**.

# 명령·번들 참조

`handoff-receipts [--store DIRECTORY] COMMAND ...`는 `$XDG_DATA_HOME/handoff-receipts` 또는 `~/.local/share/handoff-receipts`를 사용한다.
저장소를 직접 지정하는 옵션은 하위 명령 앞에 둔다. `--version`은 패키지 버전을 출력한다.
모든 명령은 `--json`을 받는다. 기계용 결과는 stdout(표준 출력)으로 나간다.
기본 출력은 시도 결과에 간결한 사람용 보기를, 다른 구조화 결과에 들여쓰기한 JSON을 사용한다.
실행 중 오류는 기계용 모드에서 `error`(오류) 필드가 있는 JSON 객체를 반환한다.
인자 파서 오류는 stderr(표준 오류)를 쓰고 2로 종료한다.
진단에는 원시 원본 stderr나 필터링하지 않은 JSON 이벤트 내용을 절대 넣지 않는다.
list/inspect/dup/which는 손상 번들을 건너뛰고 조회 가능한 결과를 반환한다.
stderr에 JSON 경고(`warning: broken_bundle_skipped`, 시도 ID, 오류 분류)를 낸다. 손상된 내용은 출력하지 않는다.
건너뛴 시도를 inspect로 직접 지정하면 빈 배열과 경고를 반환한다.
verify/export는 엄격한 무결성 실패를 유지한다.

## 명령

Antigravity는 experimental이며 native 해석은 **verified 1.5.9** 범위다.
NDJSON 입력·resume DB 경계·`agy:ID` 조회·네 가지 passthrough·wire 조각·
버전/해시 강등·취소 충돌은 [별도 계약](agy.md)을 참고한다. 기존 JSONL segment 설명은
Codex/Claude에 적용하며 agy의 profile은 `agy-selected-fields-v1`이다.


| 명령 | 인자와 동작 |
|---|---|
| run FILE | 명시적 전송 한 번. 기본 엔진은 codex, 기본 경로는 exec |
| inspect ID | 시도 ID 또는 원본 턴·메시지 ID. ENGINE:ID를 받으며 일치 항목 배열을 반환 |
| dup ID | 시도 또는 재시도 그룹 ID. 중복·유효 시도 평가 |
| which SHA_OR_PATH | 정확한 지시 해시 또는 포착한 경로. 그 경로를 읽지는 않음 |
| list | 저장된 모든 유효 시도 |
| verify [ATTEMPT] | 하나 또는 모든 번들, 체크포인트, 선별 줄 해석 검사 |
| recover ATTEMPT | 명시적 원본 재조회. 체크포인트 추가, 재전송은 하지 않음 |
| export [ATTEMPT] --format jsonl --output FILE | 시도마다 모든 common E와 파생 receipt를 담은 JSONL 레코드 하나 |
| export [ATTEMPT] --format polylogue --output NEW_DIR --polylogue-bridge BINARY | 원본 전달 원장 가져오기와 재조회. common E 보충 자료 포함 |
| forget ATTEMPT | 손상된 번들도 체크포인트 이력까지 삭제. 원본·내보낸 자료는 남김 |

`inspect`, `dup`, `which`, `list`는 `--engine codex|claude|agy` 필터를 받는다.
엔진 구분자가 없는 원본 ID가 여러 엔진에 일치하면 거절한다.
`inspect claude:ID` 또는 `inspect ID --engine claude`를 사용한다.
Claude 턴 ID는 확인된 사용자 메시지 UUID다. 검증된 실행 턴 ID가 아니다.
스레드·세션 ID만으로는 inspect 대상을 선택할 수 없다.
verify, recover, forget은 시도 ID를 받는다.

run 옵션:

| 옵션 | 의미 |
|---|---|
| --engine codex\|claude\|agy | 기본값 codex. 모델 이름으로 엔진을 선택하지 않음 |
| --via exec\|daemon | 기본값 exec. daemon은 Codex 전용이며 --thread 필요 |
| --resume ID, -r ID | 명시적 Codex 스레드·Claude 세션·agy 대화 UUID. 마지막 세션을 암묵적으로 고르지 않음 |
| --thread THREAD --socket PATH | 명시적 데몬 대상. 소켓 기본값은 ~/.codex/app-server-control/app-server-control.sock |
| --retry-of ATTEMPT | 같은 엔진·지시 바이트·재시도 그룹을 쓰는 새 시도 |
| -m MODEL, --model MODEL | 원본 --model 인자. 생략하면 선택한 엔진의 기본값 |
| -e EFFORT, --effort EFFORT | Codex model_reasoning_effort 설정. Claude --effort(low/medium/high/xhigh/max). agy --effort |
| -C DIR, --cwd DIR | 프로세스 cwd와 원본 경로 범위. 새 Codex exec에는 -C도 전달. Claude resume은 원래 cwd 사용 |
| -o PATH, --output PATH | 안전한 최종 메시지 파일. -n을 쓰거나 기존 디렉터리를 지정하면 그 안에 기록 |
| --child-stderr PATH | Codex·Claude·agy exec 대상 프로세스의 stderr 원문 파일 |
| -n NAME, --name NAME | 사람이 읽는 이름과 출력 파일의 기본 이름(`NAME.md`) |
| --codex EXECUTABLE | Codex 실행 파일 변경. Claude에서는 거절 |
| --claude EXECUTABLE | Claude 실행 파일 변경. 기본값 claude |
| --agy EXECUTABLE | 실험적 Antigravity 실행 파일. 기본값 agy |
| --session-root DIRECTORY | 원본 루트 변경. Codex: CODEX_HOME/sessions 또는 ~/.codex/sessions. Claude: ~/.claude/projects. agy: ~/.gemini/antigravity-cli/conversations |
| --observation-timeout SECONDS | 원본 재조회 시간 한도. 0..30초, 기본값 2. 전송은 재시도하지 않음 |
| -- EXTRA_ARGS | 엔진별 exec 인자 그대로 전달. Claude는 명시적 허용 목록 사용 |

`run --child-stderr PATH`는 대상 자식의 stderr 바이트를 그대로 저장소 밖의 호출자 관리 파일에 쓴다.
파일은 0600으로 생성한다. 기존 파일은 덮어쓰고 권한을 0600으로 설정한다.
부모 디렉터리는 미리 존재하고 쓸 수 있어야 한다. 목적지를 사용할 수 없으면 전송 전에
종료 코드 2로 거절하고 attempt를 만들지 않는다. 옵션을 생략하면 자식 stderr는 DEVNULL로 버린다.

증거가 아니며 계정 정보 등이 들어 있을 수 있으니 호출자 책임으로 관리한다.
파일 내용과 경로는 receipt나 저장소에 기록하지 않는다.
이 옵션은 `run --via exec` 전용이다.
fingerprint의 `agy --version` 같은 보조 프로세스의 stderr는 계속 버린다.

최상위와 run의 긴 옵션은 전체 이름을 사용한다.
`--sock`, `--cod` 같은 축약형은 엔진 보호 규칙을 우회하지 못하도록 거절한다.
`-r`처럼 명시적으로 정한 짧은 별칭은 계속 지원한다.
Claude는 `--via daemon`, `--thread`, 명시적으로 지정한 Codex 전용 `--socket`/`--codex` 옵션을 전송 전에 거절한다.

래퍼가 stdin 프롬프트, 세션 선택, JSON 스트림, 최종 출력을 관리한다.
Codex에 그대로 전달하는 인자로 `--json`, `-o`/`--output-last-message`, `-C`/`--cd`를 바꿀 수 없다.
`--ephemeral`을 켜거나 암묵적 `--last`/`--all`을 선택할 수도 없다.
나머지 Codex 옵션은 argv(인자 배열)로 전달한다. 유효 여부는 설치된 버전이 결정한다.
본문은 stdin으로 보내며 `--skip-git-repo-check`를 사용한다.
새 exec의 샌드박스 예시는 `-- -s read-only`, resume 예시는 `-- -c sandbox_mode=read-only`다.

Claude는 `claude -p --output-format stream-json --verbose`에 `--session-id UUID` 또는 `--resume UUID`를 사용한다.
선택 사항인 `--model`, `--effort` 뒤에 허용한 추가 인자를 붙인다.
새 UUID는 프로세스를 시작하기 전에 생성해 변경 불가능한 요청에 확정한다.
본문은 stdin으로 보낸다. 래퍼의 `--` 구분자를 위치 인자 프롬프트의 구분자로 전달하지 않는다.
두 엔진 모두 셸 확장을 사용하지 않는다.

Claude의 추가 플래그는 `--safe-mode`, `--disable-slash-commands`, `--strict-mcp-config`, `--no-chrome`, `--include-partial-messages`다.
값 하나를 받는 허용 옵션은 `--tools`, `--allowedTools`/`--allowed-tools`, `--disallowedTools`/`--disallowed-tools`,
`--permission-mode`, `--permission-prompts`, `--max-budget-usd`, `--append-system-prompt`, `--system-prompt`, `--setting-sources`다.
예를 들어 `-- --tools ""`는 도구를 명시적으로 비활성화한다. 래퍼 기본값은 아니다.
원본 CLI의 검증은 여전히 적용된다.
알 수 없는 플래그, 위치 인자를 통한 프롬프트 교체, 축약형, 짧은 옵션 묶음, 래퍼가 관리하는 옵션은 거절한다.
특히 추가 session-id, resume, continue, fork-session, output-format, input-format, print, no-session-persistence 옵션으로 확정된 요청을 바꿀 수 없다.

Claude는 원본 루트 아래 기록된 프로젝트 폴더에서 정확한 `<session UUID>.jsonl`만 검사한다.
관측된 폴더 인코딩은 절대 cwd의 `/`를 `-`로 바꾼다.
resume은 기존 파일의 길이와 접두부 SHA-256을 변경 불가능하게 저장한다.
관측 중 접두부가 달라졌으면 거절한다.
resume 전에 파일이 없었어도 전송에서 reported(보고됨) 사실을 얻을 수 있다.
다만 원본 기록 연결은 `claude_preflight_unavailable`과 함께 unknown(알 수 없음)으로 남는다. 복구도 마찬가지다.
다른 프로젝트 폴더는 검색하지 않는다.
반면 Codex resume은 전송 전에 rollout(원본 기록 파일)이 없으면 거절한다.
원본 경로 구조는 해당 버전에서 관측한 결과다. 모든 버전에 통하는 경로 계약이 아니다.

Codex 최종 출력은 보존된 안전한 원본 최종 메시지(`observed`, 관측됨)를 우선한다.
없으면 유일하게 연결되고 완료된 exec 스트림의 `item.completed`(항목 완료 이벤트) 에이전트 메시지를 쓸 수 있다.
개인정보 검사를 통과해야 하며 평가 수준은 `reported`다.
receipt의 `output`(출력 정보)은 평가, 출처(`target_rollout` 또는 `exec_json_stream`), 해시 요약값, 목적지를 기록한다.
나중 스트림 후보가 민감하거나 너무 크면 앞선 후보도 출력하지 않는다.
Claude 최종 출력은 선택한 상태가 completed(완료)일 때 개인정보 검사를 통과한 `result.result`만 사용한다.
출처는 `claude_json_result`, 평가 수준은 reported다.
원본 assistant 텍스트 해시는 보조 정보다. 출력 파일의 근거로 쓰지 않는다.
`is_error=false`와 `terminal_reason=completed`가 완료를 결정한다. `subtype`은 쓰지 않는다.
오류·중단 결과는 최종 출력의 근거가 되지 않는다.

최종 메시지가 없거나 생략되면 요청한 출력 파일은 그대로 둔다.
기존 출력 파일을 쓰기 전에 최종 출력 사실을 inspect로 확인한다.
receipt 저장소 안의 출력 경로는 거절한다.

recover는 `--observation-timeout SECONDS`로 저장된 또는 기본 재조회 시간 한도를 바꿀 수 있다.
`-o PATH` / `--output PATH`로 저장된 목적지도 바꿀 수 있다. 변경 불가능한 요청은 다시 쓰지 않는다.
`-o`가 없으면 원래 출력 목적지를 사용한다.
복구에도 같은 엔진별 출력 규칙을 적용한다. 유실된 Claude result를 원본 end_turn에서 재구성할 수 없다.

Codex 데몬 전송에서는 상태를 바꾸기 전에 thread/read가 idle(유휴)을 보고해야 한다.
래퍼는 turn/start를 보내기 전에 기록한다. 이후 반환된 정확한 턴 ID를 기다린다.
전체 상태가 idle이 되기를 기다리지 않는다.
새 스레드에는 rollout 경로·파일이 없을 수 있다.
턴 목록이 비어 있거나 아직 생성되지 않았다면 0바이트 사전 점검을 허용한다.
턴이 끝나면 경로를 다시 읽는다.
기존 스레드의 rollout 누락, 다른 읽기 실패, busy(사용 중) 또는 notLoaded(아직 로드되지 않음) 스레드는 turn/start 전에 거절한다.
notLoaded 메시지는 다음과 같다.
`thread not loaded in the daemon; open it in the Codex app or use --via exec --resume`.
스레드 로드나 소유권 이전은 시도하지 않는다.
거절 결과는 `facts.attempted=false`(시도하지 않음), `facts.dispatched=false`(전송하지 않음), `rejection_reason`(거절 사유)을 제공한다.
사람이 읽는 inspect/list에는 `not sent`(전송 안 됨)를 표시한다.
의도만으로는 요청이 서버에 도달했을 가능성이 있다는 뜻이 아니다.
응답이 유실되었으면 기록된 경계 뒤에서 유일하게 정확히 일치하는 원본 입력으로만 해당 전송을 연결할 수 있다.
나중 `recover`로 빠진 사실을 채울 수 있다.
모델과 추론 노력 수준은 turn/start에 전달한다. 여기서는 exec 인자를 그대로 전달하는 기능을 지원하지 않는다.

번들을 건너뛰었다는 경고가 있는 조회는 0을 반환한다.
그 밖의 잘못된 인자, 엄격한 무결성 검사 실패, 사용할 수 없는 내보내기는 2를 반환한다.
run은 가능하면 원본 프로세스 종료 코드를 유지한다. 음수 시그널 반환 코드는 셸 관례로 변환한다.
원본 종료 코드가 0이면 선택된 턴 상태가 completed일 때 0을 반환한다. observed 완료와 reported 완료 모두 해당한다.
원본 증거가 없으면 stderr에 `target_observation`(대상 관측) 경고를 낸다. 구체적인 진단 코드와 설명을 포함한다.
observed 실패·중단은 스트림 완료보다 우선한다. 이때 원본 종료 코드가 0이어도 2를 반환한다.
결과가 unknown이어도 2를 반환한다.
Claude에서 완료 보고가 연결된 원본 오류·중단 흔적과 충돌하면 `claude_status_conflict`가 발생한다.
상태와 최종 출력은 unknown이 되고, 원본 종료 코드가 0이면 래퍼는 2를 반환한다.
유실된 결과는 복구 뒤에도 unknown으로 남는다. 원본 흔적만으로는 전체 턴 완료를 확정할 수 없다.
원본 종료 코드가 0이 아니면 완료가 관측되어도 0이 아닌 코드를 유지한다.
최종 출력이 없으면 `output_unavailable`(출력 사용 불가)을 낸다.
다만 생명주기에 근거한 이 종료 정책은 바꾸지 않는다.
receipt의 사실은 프로세스 상태, 턴 결과, 출력 사용 가능 여부를 서로 독립적으로 유지한다.
데몬에서 성공하지 못하면 2를 반환한다.
recover는 재조회를 성공적으로 기록하면 0을 반환한다.
경고가 있는 reported/unknown 결과도 포함한다. 과거 종료 코드를 재현하지는 않는다.

## 번들 버전 1 / 해석 버전 2

```text
STORE/ATTEMPT_ID/
  source.json              # engine/profile, source pointer, digest, retry link
  body.txt                 # exact UTF-8 instruction
  request.json             # argv or RPC intent, target pointer, preflight boundary
  INTERPRETATION.md         # packaged interpretation at initial capture
  manifest.json            # hashes of the four preceding files
  states/SHA256.json       # immutable {version, previous, state} checkpoints
  CURRENT                  # durable head checkpoint digest
  .lock                    # advisory capture/recovery lock, no private contents
```

게시 전에는 첫 체크포인트를 포함한 초기 번들 전체가 `STORE/.pending-*`에 있다.
fsync와 한 번의 디렉터리 이름 변경으로 게시한다. 숨김 준비 디렉터리는 조회에서 제외한다.
초기화 프로세스가 강제 종료되면 이 디렉터리가 남을 수 있다. 비공개 의도 정보가 들어 있다.
실행 중인 초기화 프로세스가 없을 때만 삭제한다.
최종 ID로 이미 게시된 손상 디렉터리는 forget으로 삭제할 수 있다.

`state.control`(제어 상태)은 프로세스·스트림·RPC 이력이다.
안전한 스트림 최종 메시지 후보를 포함해 명시적으로 고른 필드만 저장한다.
`state.target`(대상 상태)은 ID, 선별된 대상 구간, 선택적 정확한 RPC 상태, 관측 문제를 담는다.
`diagnostics`(진단)는 고정된 안전한 코드·설명 쌍과 선택적 개수·오프셋을 담는다.
원시 원본 내용이나 예외 텍스트는 절대 담지 않는다.
`issues`(문제)는 호환성을 위해 코드 문자열을 유지한다.
`observation_attempts`(관측 시도 이력)는 매 재조회의 타임스탬프와 코드를 기록한다.
나중 읽기에 성공하면 현재 문제를 지운다. 이전 시도와 과거 체크포인트는 보존한다.
각 구간은 복사한 JSONL 바이트를 UTF-8 `copy_text`(복사 텍스트)로 저장한다.
보존한 줄의 원본·사본 오프셋과 해시, 생략한 줄의 범위·해시·사유 목록,
전체 구간·스냅샷 해시, 파생 요약 필드도 저장한다.
조회는 요약 플래그를 신뢰하지 않고 보존한 바이트에서 사실을 다시 도출한다.
새 source/request 파일은 `engine`과 `interpretation_profile`을 변경 불가능하게 저장한다.
프로필은 `codex-selected-lines-v2` 또는 `claude-selected-lines-v1`이다.
엔진 메타데이터가 없으면 Codex다. 기존 번들과 그 해석 파일은 마이그레이션하지 않는다.
전체 규칙은 [엔진별 해석 규칙](../../src/handoff_receipts/INTERPRETATION.md)에 있다.

새 Claude 구간은 `association=parent-chain-v1`과 `turn_key={kind: user_message, value: UUID}`를 선언한다.
선택 구간은 사전 점검 뒤 유일하게 일치하는 입력에서 시작해 완전한 스냅샷 경계에서 끝난다.
assistant·중단 행을 연결하려면 부모 연결 사슬이 그 입력에 도달해야 한다.
다른 실제 입력, 누락·구간 밖 연결, 중복 ID, 순환이 있으면 연결할 수 없다.
명시적 promptId가 충돌해도 연결할 수 없다.
표식이 없는 기존 구간은 명시적 promptId 해석을 유지한다.

`connection_projections`(파생 연결 정보)의 각 항목은 정확히 다음 필드로 구성된다.
`kind=derived_connection`, `uuid`/`parentUuid`만 담는 `fields`, 생략한 원본 줄에 대응하는 `start`, `end`, `sha256`이다.
파생된 연결 정보이며 원본 JSONL 행이 아니다.
오프라인 검증은 필드·출처를 확인하고 보존한 바이트로 연결을 다시 도출한다.
생략된 원본이 없으면 그 값이 주장하는 원본 해시의 행에서 왔는지 인증할 수 없다.
보조 해시·흔적만 이 연결을 사용한다.

파생 receipt는 `facts`(사실), 필드별 `fact_levels`(평가 수준), `observed_facts`(관측 사실),
별도의 `reported` 요약, `identity_levels`(식별 정보의 평가 수준), `output`,
`observation_issues`(관측 문제), `observation_attempts`를 제공한다.
사람이 읽는 inspect/list에는 예를 들어 `completed (reported)`와 진단 설명을 표시한다.
reported 생명주기는 입력, 누락된 정확한 턴 ID, 원본 완료 시각을 확인해 주지 못한다.
R2/R3는 여전히 observed 입력 증거가 필요하다.
Claude `dup`에는 서로 다른 세션·사용자 메시지 쌍의 종료 보고도 필요하다. 입력 쌍만으로는 세지 않는다.
오프라인 조회는 예전의 일반적인 문제를 원인 미상의 과거 문제로 유지한다.

Claude는 `turn_key`, `native_traces`, `auxiliary`도 제공한다.
auxiliary는 원본 출력 해시, 메시지 종료 사유·타임스탬프, reported 출력과의 비교를 담는다.
turn_started와 completed_at은 unknown으로 남는다.
원본 오류·중단 흔적이 완료 보고와 충돌하면 상태는 unknown이 된다. 원래 `reported` 보기는 보존한다.
[사실 표](concepts.md#독립된-사실)를 참고한다.

주요 관측 진단:

| 코드 | 의미 |
|---|---|
| rollout_not_found / rollout_unreadable | 경로·파일 누락 또는 파일시스템 읽기 실패 |
| rollout_unsafe_path / rollout_too_large | 안전하지 않은 경로 또는 32 MiB를 넘는 스냅샷 |
| rollout_incomplete / rollout_invalid_json / rollout_invalid_shape | 불완전하거나 형식이 잘못된 원본 데이터 |
| rollout_metadata_missing / rollout_thread_mismatch | 원본 식별 정보 누락 또는 불일치 |
| preflight_prefix_changed | 앞서 해시를 기록한 접두부가 더 이상 일치하지 않음 |
| ambiguous_rollout_path / ambiguous_target_match | 가능한 원본 파일 또는 대상 구간이 여러 개임 |
| no_matching_turn | 사전 점검 경계 뒤에 해당 전송으로 식별할 수 있는 원본 턴이 없음 |
| input_binding_omitted / target_input_missing | 안전하고 정확한 입력 연결을 사용할 수 없음 |
| sensitive_required_lines | 개인정보 검사기가 필수 줄을 생략함 |
| turn_terminal_missing / final_output_missing | 해당 턴으로 식별할 수 있는 종료 기록 또는 안전한 최종 출력을 사용할 수 없음 |
| native_selection_invalid | 그 밖의 지원하지 않는 선택 구조. 원시 예외는 저장하지 않음 |
| claude_preflight_unavailable | 원래 resume 경계가 없음. 원본 기록 연결을 재구성할 수 없음 |
| claude_identity_mismatch | init/result 세션이 확정된 대상과 다름 |
| claude_result_unavailable | 사용할 수 있는 유일한 result가 없음. 원본 흔적은 이를 대신하지 못함 |
| claude_status_conflict | 완료 보고가 연결된 원본 오류·중단 흔적과 충돌 |

일시적 부재·불완전한 기록, 턴·입력·종료·출력 누락은 설정된 시간 한도까지 100 ms 간격으로 재조회한다.
개인정보 문제, 모호성, 접두부 변경, 그 밖의 영구적 오류는 즉시 멈춘다.
시간 한도가 0이어도 한 번은 읽는다.
한도는 추가 읽기를 예약하는 시간을 제한한다. 파일시스템 작업의 소요 시간을 제한하지는 않는다.

제품의 모든 비공개 디렉터리는 생성 시 0700, 파일은 0600이다.
명시적으로 내보낸 파일과 최종 메시지 파일도 0600이다.
새로 만드는 내보내기·출력 디렉터리는 0700이다.
호출자 소유의 기존 상위 디렉터리는 변경하지 않는다.
외부에서 권한을 바꾸는 행위는 암호학적 변조 탐지 대상이 아니다.

검증은 변경 불가능한 핵심 파일 해시, 모든 완전한 체크포인트 해시와 최신 체크포인트 연결 사슬,
원본·본문 연결, 선별 사본 해시, 줄 역할과 범위를 검사한다.
예상하지 않은 번들 파일이 있으면 검증에 실패한다.
불완전한 `.writing-*` 파일은 사실의 근거가 될 수 없다.
생략된 원본 바이트는 원본 없이 검증할 수 없다. 전체 범위 해시는 포착 당시의 기준값으로 남는다.
원본 소스는 읽지 않는다.
