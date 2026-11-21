이 문서는 [영어판](../reference.md)의 번역이며, 내용이 다르면 영어판이 우선한다. 번역 기준 버전: **0.1.0**(Rust 크레이트 선언 버전).

# 인터페이스 참조

이 문서는 구현된 인터페이스의 현재 제품 계약이다.
[시작하기](getting-started.md)의 예시는 아카이브나 소켓 없이 실행된다.
아래 명령 개요는 인자를 설명한다. 대문자 자리표시자는 실행 가능한 설치 지침이 아니다.
각 CLI는 호출마다 작업 하나를 받는다.

## JSON과 동일성

모든 구성요소는 원시 JSON을 `uasf-json`으로 디코딩한다.
`$serde_json::private::Number`와 `$serde_json::private::RawValue`를 포함한 일반 객체 키는 그대로 키로 남는다.
정수는 큰 정수도 정확히 유지한다.
소수점·지수 표기 숫자는 유한한 binary64 부동소수점 수다.
`1e0`, `1.00`, `1.0`은 같은 부동소수점 값이다. 정수 `1`, 불리언 `true`와는 다르다.
부동소수점 `-0.0`은 `0.0`과 다르다. 정수 `-0`은 `0`과 같다.
변경 불가능한 등록의 정규형 비교에서는 객체 키 순서를 무시한다.
배열 순서, 자료형, null과 필드 부재의 차이, 부동소수점 0의 부호는 유지한다.
원래 표기는 원본 바이트로 보존한다. 정규형 JSON으로 재구성하지 않는다.

표준 디코더는 중복 객체 키 중 마지막 것을 사용한다.
feeder 메타데이터와 완전한 객체 줄에 쓰는 고유 키 디코더는 중복을 거절한다.
이스케이프를 달리해 같은 키를 표현한 경우도 포함한다.
잘못된 UTF-8·유니코드, 뒤에 붙은 입력, 비유한 값, 부동소수점 오버플로, 128을 넘는 깊이는 거절한다.
소비자는 내부 표식 키를 숫자로 바꾸는 JSON 디코더를 사용해서는 안 된다.
gate에는 별도의 아카이브 값 비교 호환 규칙도 있다.
값이 같은 정수·부동소수점 수는 같다고 비교한다. 불리언은 숫자 0·1로 비교한다.
부호가 다른 부동소수점 0도 같다고 비교한다.
구조적 길이, 순번, 오프셋에는 여전히 부호 없는 정수가 필요하다.
이 필드에는 불리언과 부동소수점 수를 허용하지 않는다.
이 규칙은 core의 변경 불가능한 등록 동일성을 바꾸지 않는다.

## Core 등록

```text
uasf-core --index INDEX --persistent PERSISTENT rebuild|query|name|evidence|delete
```

stdin(표준 입력)으로 JSON을 보내거나 `--json JSON`을 사용한다(`--json -`는 stdin 선택).
인덱스와 영속 SQLite 파일은 서로 달라야 한다.
재구축 입력은 `sources`(원본 목록)와 `captures`(포착 목록) 배열이 있는 객체다.
각 포착 항목에는 `capture` 객체와 로컬 `fixture_path` 연결이 들어 있다.
경로는 어댑터 설정이지 식별 정보가 아니다.
기존 등록은 생략해도 남는다.
변경 불가능한 식별 정보·해시 요약값의 충돌은 거절한다.
재구축 완료는 이력이 완전하다는 주장이 아니다.

| 객체 | 필수 필드 | 기본값과 제약 |
|---|---|---|
| Source | `id`, `engine` | 엔진은 `codex`, `claude`, `api` 중 하나. `observation` 기본값은 `fixture` |
| Capture | `source`, `capture`, `generation`, `length`, `sha256`, `session` | 길이는 부호 없는 정수. SHA-256은 소문자 16진수. `window`와 `file_generation` 기본값은 null. `native_session` 기본값은 true |
| Anchor | `source`, `capture`, `generation`, `start`, `end`, `sha256` | `file_generation`은 선택 사항이며 null 허용. 바이트 범위는 `[start,end)`. 증거·삭제에는 비어 있지 않은 범위 필요 |
| 이름 요청 | `source`, `session`, `name` | 앞뒤 공백을 제거한 문자열. 빈 문자열은 사용자 이름을 지움 |
| 증거 | `evidence_id`, `source`, `session`, `fact`, `assessment`, `method`, `observed_at`, `anchors` | 앵커 목록은 비어 있으면 안 됨. `body`, `target_turn`, `target_call` 기본값은 null |

형식이 정의된 등록·이름·증거 요청에서 알 수 없는 필드는 거절한다.
증거 사실과 평가 값은 [개념](concepts.md#검증접근증거)에 열거되어 있다.
같은 원본 범위의 증거 ID는 재등록으로 바꿀 수 없다.
삭제된 증거는 재실행으로 되살릴 수 없다.
이름은 독립적인 메타데이터다.
라벨은 관측된 제목이나 엔진과 세션 접미사의 조합으로 대체될 수 있다.
라벨로 식별 정보가 확정되지는 않는다. 호스트 정보도 추론하지 않는다.

로컬 공급자는 `ByteProvider::read(capture,start,end)`를 통해 파일·메모리 바이트 연결을 지원한다.
반환값에는 선택적 바이트, 해시 요약값, 검증, 사유가 있다.
일부를 잘라 반환하기 전에 고정한 접두부 전체를 검증한다.
뒤에 내용을 추가해도 접두부는 유지될 수 있다. 접두부가 바뀌면 바이트 제공을 보류한다.
포착 시점은 인증하지 않는다. 파서는 반환된 길이·해시를 다시 검사한다.

지원되는 제출 API 줄에는 `type: "api-record"`, `schema: "uasf.synthetic-api/1"`,
`conversation`, `call_id`, `request`, `response`, `complete`가 있다.
완전한 UTF-8 JSONL 레코드여야 한다.
제출된 범위는 대화 전체가 완전하다는 증거가 아니다.

### Codex 창 등록

Codex 파서는 명시적으로 등록한 `Capture.window`를 창 식별 정보로 사용한다.
`session_meta.payload.session_id`를 `Capture.session`과 비교한다.
등록된 창이 없으면 `window_id_required`를 반환한다.
이 사유는 `session_meta`에 `window_id` 필드가 있어야 한다는 뜻이 아니다.
core는 그 필드, `payload.id`, 파일 경로에서 창을 도출하지 않는다.

통합은 재구축 전에 창을 제공해야 한다.
이력의 `leaf`(선택할 끝 창)를 고를 때도 등록된 창을 사용해야 한다.
알려진 파일명 형식에서는 검증된 원래 rollout(원본 기록 파일) 파일명으로 창을 얻을 수 있다.
이름을 바꾼 staging 사본은 원래 파일명을 대신할 수 없다.
파일명 추출은 관측된 형식에 대한 어댑터 규칙이다. 영구적인 원본 엔진 계약이 아니다.
세션 ID와 창 ID를 혼동해서는 안 된다.
등록은 순번, 이력 모드, 부모, 분기 경계 검증을 우회하지 않는다.

## Core 조회

요청에는 `op`(작업)와 아래 필드가 있다.
응답 봉투는 `{schema, op, status, reasons, data}`이며 스키마는 `uasf-core.query/1`이다.
`reasons`(사유)는 정렬하고 중복을 제거한다.
`data`(데이터)는 작업마다 다르다.

| 작업 | `op` 외 필드 | 반환 데이터 |
|---|---|---|
| `sources` | 없음 | `sources` |
| `sessions` | 선택적 `source` | `capture_ids`, 앵커, 라벨이 있는 `sessions` |
| `history` | `source`, `session`, `boundary`, 선택적 `leaf` | 엔진별로 선택한 파생 보기 |
| `events` | `source`, `session`, `boundary` | 잘못되었거나 UUID가 없는 레코드를 포함한 물리적 `events` |
| `turn` | 이력 필드와 전체 `turn_id` | 선택·제외된 턴과 원래 앵커 |
| `lineage` | 이력 필드, 선택적 `target`, `kind` | 참조 간선과 해석 결과 |
| `evidence` | `source`, `session` | `facts`, `automatic_promotion: false`, `absent_facts` |
| `status` | 이력 필드 | 이벤트 본문을 제외한 파생 보기·접근 진단 |

`boundary`(경계)는 순서가 있고 비어 있지 않아야 한다.
`sessions`에서 얻은 같은 원본의 포착 ID를 담는다.
명시적 부모 세션 포착도 포함할 수 있다.
Codex에는 명시적 끝 창이 필요하다.
경계가 없으면 불완전한 결과를 낸다. 최신 파일을 고르지 않는다.
조회는 해당 작업에 없는 추가 필드를 거절한다.
비교에서는 객체 키 순서를 무시하고 배열 순서와 null·부재의 차이를 유지한다.

물리적 이벤트는 경계 순서, 바이트 위치 순서로 나열한다.
Codex 이력은 선택한 부모→자식 창 사슬을 따른다.
활성·제외 구분과 턴의 첫 등장 순서를 유지한다.
Claude는 물리적 관측과 선택한 부모 사슬을 반환한다.
논리적 부모, 보존 구간, 요청 참조는 그 사슬 밖의 그래프 간선으로 남는다.
참조 해석이 노출을 뜻하지는 않는다.
목록은 결정론적인 객체 순서를 사용한다.
클라이언트는 이력을 타임스탬프나 원본 UUID로 다시 정렬해서는 안 된다.
바이트 범위 식별 정보와 원래 앵커가 계속 우선한다.

### Claude 레코드 유형과 형식 거절

인식하는 Claude 유형 18개는 `queue-operation`, `user`, `assistant`, `system`,
`attachment`, `file-history-snapshot`, `atis-latch`, `api-request-shape`,
`api-request-blob`, `api-request`, `last-prompt`, `cost-state`, `mode`,
`custom-title`, `ai-title`, `progress`, `summary`, `permission-mode`다.
인식한 유형도 필드 형식 검사를 통과해야 한다.

`permission-mode`는 **정확히** `type`, `permissionMode`, `sessionId`만 있는 객체를 지원한다.
`permissionMode`는 문자열이어야 한다.
`sessionId`는 등록된 포착 세션과 같아야 한다.
키 누락·추가, UUID·부모·참조 필드 추가, 문자열이 아닌 모드, 세션 불일치는 `unsupported_claude_record`로 거절한다.
관측된 생산자 값에는 `plan`과 `default`가 있다. 허용 값이 이 둘로 한정되지는 않는다.
값은 해석하지 않는 세션 메타데이터로 보존한다. 인가 지시로 취급하지 않는다.
이 레코드는 턴, 계보 간선, 압축 루트, 삭제 의존 관계를 만들지 않는다.
바이트, 앵커, 본문에는 여전히 일반 삭제 정책을 적용한다.

알 수 없는 Claude `type`은 포착을 `unsupported`로 표시한다.
사유는 `unsupported_claude_record`다.
Claude 이력은 물리적 이벤트를 반환하지만 그래프 계산은 멈춘다.
이 응답의 빈 그래프가 관계 부재를 증명하지는 않는다.
Codex는 알 수 없는 유형을 `unsupported_codex_record`로 거절한다.
알 수 없는 레코드를 불투명하게 해석하는 일반 기능은 구현되어 있지 않다.

기본 파서는 기존 입력의 `m1.2` 출력 버전을 유지한다.
`permission-mode`가 포함된 Claude 포착은 거절된 형식도 포함해 파생 객체에 `claude/m1.3`을 사용한다.
해당 세션도 같은 리비전을 쓴다.
그런 포착을 포함한 재구축은 `parser_version: "m1.3"`을 보고한다.
다른 포착은 각자의 버전을 유지한다.
라이브러리의 명시적 `rebuild_version` 재정의는 계속 호출자 라벨일 뿐이다. 호환성 증거가 아니다.
새 바이너리를 설치해도 저장된 기록을 다시 해석하지 않는다.
이 확장을 적용하려면 검증된 바이트에서 파생 인덱스를 재구축한다.
기존 미지원 포착은 그때까지 미지원 상태로 남는다.
영속 등록, 증거, 이름, 삭제 기록은 유지해야 한다.
파서가 바뀌는 것만으로 바이트 앵커 식별 정보가 바뀌지는 않는다.

### 상태와 사유

상태는 `complete`, `incomplete`, `unsupported`, `unknown`, `deleted` 중 하나다.
집계 시 심각도 순서는 deleted, unsupported, incomplete, unknown, complete다.
`body_access`(본문 접근)는 독립적이며 `available`, `blocked`, `deleted` 중 하나다.
제공을 보류한 본문은 null이며 `body_reasons`(본문 사유)가 있다.
`masked` 상태는 없다. 주요 사유는 다음과 같다.

| 사유 | 의미 |
|---|---|
| `query_fields_or_operation`, `query_field_type` | 미지원 필드·작업 또는 잘못된 필드 자료형 |
| `source_and_session_required` | 범위가 정해진 작업에 식별 정보가 없음 |
| `explicit_capture_boundary_required` | 명시적 경계를 제공하지 않음 |
| `duplicate_boundary_capture` | 경계에서 같은 포착이 반복됨 |
| `capture_boundary_source_or_id_mismatch` | 경계에 알 수 없거나 다른 원본의 포착이 있음 |
| `session_capture_missing_at_boundary` | 경계에 이 세션의 포착이 없음 |
| `session_not_indexed` | 인덱스에 알려진 세션이 아님 |
| `native_authority_not_compared`, `model_input_unknown` | 관측 그래프는 모델 입력을 권위 있게 재구성한 결과가 아님 |
| `submitted_scope_only_total_conversation_unknown` | 제출된 API 관측만 알고 있음 |
| `index_requires_rebuild`, `query_scope_unavailable` | 관련 파생 준비 상태·정책 메타데이터로 조회를 지원할 수 없음 |

주요 사유 목록이다. 클라이언트는 반환된 다른 사유도 처리해야 한다.
범위 한도와 정확한 거절 코드는 [보안과 한계](security-and-limits.md#core-한계)에 있다.

## 응답과 오류

core가 처리한 의미적 조회는 `incomplete`, `unsupported`, `unknown`, `deleted`여도 종료 코드가 0이다.
종료 코드만 보지 말고 상태와 사유를 확인한다.
범위 제한 거절은 `data`가 비어 있다.
잘못된 입력이나 저장소 실패는 최소 봉투 `{"status":"unsupported","reasons":["invalid_input_or_unavailable_store"]}`와 함께 종료 코드 2를 낸다.
접수·출력 오류도 해당 사유를 담은 최소 봉투를 사용한다.
예로 `query_capacity_exceeded`, `query_admission_unavailable`, `query_request_limit`,
`query_spool_limit`, `query_output_limit`이 있다.

선택적으로 연결한 삭제 장벽은 최소 응답 `{"status":"incomplete","reasons":["pending_deletion"]}`를 처리된 결과로 반환한다(종료 코드 0).
전체 조회 스키마나 빈 data 필드를 만들어 붙이지 않는다.
정상 재구축은 `complete`, `captures_indexed`, `parser_version`, `meaning`을 반환한다.
`meaning`은 재구축 성공과 이력 완전성을 명시적으로 구분한다.
name은 `complete`를 반환한다. evidence는 자신의 ID도 반환한다.
core 삭제는 `deleted`, 요청 개수, `body_policy: "explicit_erasure_and_reversible_dependency_blocking"`,
`scope: "core_index_and_durable_evidence_only"`를 반환한다.

writer 경로는 목적지에 전달하기 전에 파생 보기 생성, 정확한 직렬화 길이 검증, 스풀 기록을 끝낸다.
내부 실패는 부분 JSON을 노출하지 않는다.
전송 실패 시에는 이미 전달한 접두부가 남을 수 있다.
종료 코드 2라고 해서 그 뒤에 두 번째 JSON 오류를 덧붙이지 않는다.
소비자는 불완전한 JSON이나 실패한 전송을 거절해야 한다.
이미 보낸 바이트를 회수하겠다는 약속은 아니다.

## Intake

```text
uasf-intake --database DATABASE --registry REGISTRY --principal PRINCIPAL
  [--deletion-root ROOT] [--collector-uid UID] [--coordinator-lock-fd FD]
  [--unmanaged-test-mode]
```

레지스트리는 **원본 ID를 허용된 principal(주체)에 매핑**한다.
stdin의 JSON 요청 하나가 JSON 응답 하나를 만든다.
principal은 신뢰하는 로컬 설정이다.
명시적으로 연결을 생략한 합성 테스트 외에는 연결이 필요하다.

| 작업 | 요청 필드 | 응답 |
|---|---|---|
| `submit`(기본값) | `source`, `session`, `payload_base64`. 선택적 `key`, `representation`, `kind`, `stream`, `sequence` | `accepted`, `replayed`, receipt. 또는 원래 receipt ID를 포함한 `conflict` |
| `get` | `source`, `receipt_id` | `ok`, receipt, 정확한 `payload_base64`. 또는 `not_found`/`deleted` |
| `stream_state` | `source`, `session`, `stream` | `ok`, chunks(청크), terminal(종료), contiguous(연속 여부), 스트림 상태 |
| `erase` | `receipt_ids` | 신뢰하는 로컬 삭제. 삭제한 receipt·키의 표식 유지 |

representation 기본값은 `app_bytes`다. `sdk_json`도 허용한다.
kind 기본값은 `record`다.
청크에는 스트림과 0 이상의 정수 순번이 필요하다.
종료 항목 `complete`/`interrupted`는 빈 페이로드와 null 순번을 사용한다.
완전한 스트림에는 0부터 연속된 청크와 complete 종료 항목이 필요하다.
대화 전체의 완전성에 대해서는 아무것도 말해 주지 않는다.
알 수 없는 요청 필드는 거절한다.

첫 전송 전에 원본별 범위의 키를 고정하면 정확히 같은 재시도는 여러 번 해도 결과가 같다.
키가 없으면 바이트가 같아도 별도의 관측이다. 중복 제거 여부는 알 수 없다.
receipt는 원본·세션, 표현·종류·스트림·순번, 페이로드 길이·SHA-256,
관측 근거, 포괄 범위의 한계, 영속 저장 참조를 연결한다.
receipt ID와 수신 시각은 달라진다.
accepted/ok와 성공한 erase는 종료 코드 0을 낸다.
get/submit의 deleted를 포함한 나머지 상태는 모두 종료 코드 2를 낸다.
파싱 오류는 `invalid_request`다.
접수 제어나 저장소를 사용할 수 없으면 성공을 확인해 주지 않는다.

## Staging

```text
uasf-staging --root ROOT --registry REGISTRY [--deletion-root ROOT]
  [--collector-uid UID] [--unmanaged-test-mode] capture SOURCE RELATIVE
uasf-staging [same options] scan SOURCE
uasf-staging [same options] delete SOURCE RELATIVE
uasf-staging [same options] inspect
```

레지스트리 버전 1에는 `sources`가 있다.
각 ID를 `kind`(`codex`/`claude`), 절대 `root`, 선택적인 정확한 상대 `files`에 매핑한다.
files가 없으면 각각 `sessions/**/rollout-*.jsonl`, `projects/**/<uuid>.jsonl` 패턴을 사용한다.
capture는 receipt를 포함한 `published` 또는 관측을 포함한 `unchanged`를 반환한다.
비었거나 불완전한 부분만 있는 입력은 `not_captured`다.
scan은 배열을 반환한다.
published, unchanged, 로컬 delete 성공은 종료 코드 0을 낸다.
scan은 모든 항목이 성공해야 한다.
inspect는 복구된 상태를 종료 코드 0으로 반환한다. 실패는 종료 코드 2를 낸다.

버전 1 receipt는 원본·종류, 포착한 입력 경로와 device/inode 정보, 세대·파일 계보,
staging 상대 경로, 원본 전체와 게시 접두부의 길이·해시, 보류한 꼬리, 상태를 연결한다.
게시 상태는 rename/fsync 전에 pending이며 이후 active가 된다.
삭제 표식에는 상태, 세대, staging 상대 경로만 남는다.
연결된 저장소의 직접 delete는 `coordinator_required`를 반환한다.
삭제 검사를 수행할 수 없으면 `not_captured/deletion_inspection_unavailable`을 반환한다.
새 사본·receipt를 만들지 않으며 삭제 표식을 만들어내지도 않는다.

## Feeder

```text
uasf-feeder --config CONFIG --key KEY --state STATE --policy POLICY --inbox INBOX
  --role producer|collector COMMAND
```

명령은 `init`, `emit SOURCE RELATIVE`, `retry GENERATION`, `accept RECORD_NAME`,
`materialize GENERATION`, `delete SOURCE RELATIVE`, `purge`,
수집기용 `publish GENERATION --staging ROOT [--unmanaged-synthetic]`이다.
설정 버전 1은 16진수 32자리 스트림 ID, 생산자·수집기 UID,
절대 루트·종류가 있는 원본, 명시적 `files` 또는 `pattern: "native-jsonl"`을 연결한다.

inbox 레코드는 정규형 ASCII JSON 메타데이터, LF, 정확한 접두부 바이트 순서다.
HMAC-SHA256이 서명 전 헤더, LF, 페이로드를 묶는다.
메타데이터에는 설정 해시 요약값, 스트림·원본·경로·종류, 세대·선행 항목·순서,
정책 리비전, 원래 device/inode/시각, 길이·해시·꼬리가 포함된다.
헤더만으로 후속 게시가 성공했다는 증거는 되지 않는다.
수집기 게시는 사본을 다시 검증하고 원래 출처를 보존하면서 staging을 호출한다.

JSON 객체 하나를 반환한다.
성공은 종료 코드 0, 거절되었거나 불가능한 포착은 종료 코드 2를 낸다.
수집기 삭제는 `deleted`, `policy_revision`, `producer_purge: "required"`,
`downstream_deletion: "required"`를 보고한다.
생산자 purge는 `purged`를 보고한다. 그래도 후속 삭제가 필요하다.
[삭제](deletion.md)를 참고한다.

## Gate

```text
uasf-gate serve --socket SOCKET [--socket-mode MODE] --bridge BRIDGE
  --registry REGISTRY --sources SOURCES --staging STAGING --source SOURCE
uasf-gate query --socket SOCKET
uasf-gate check-local --bridge BRIDGE --registry REGISTRY --sources SOURCES
  --staging STAGING --source SOURCE
```

query/check-local은 JSON 객체를 받는다.
`read`, `resolve`, `anchor` 작업에는 최대 100자인 `generation`이 필요하다.
`reference`는 선택 사항이다.
resolve는 `expected_sha256`을 제공하고 anchor는 정수 `ordinal`을 제공한다.
소켓 요청은 쓰기 EOF로 끝나는 JSON이다. 지속적인 다중화 프로토콜이 아니다.
브리지는 신뢰하는 외부 실행 파일이다.
목록, 포착 검증, 읽기, 앵커, excisions, 참조 해석을 제공한다.
이 워크스페이스에는 포함되어 있지 않다.

gate는 내용을 내보내기 전에 receipt 연결, 전체 해시, 현재 삭제 정책을 검증한다.
상태는 `verified`, `unverified`, `mismatch`, `deleted`, `out_of_scope`, `unsupported`다.
verified 출력에는 generation(세대), reference(참조), length(길이), SHA-256, `original_provenance`가 포함된다.
read는 바이트를 `hex`로 담는다.
이름과 달리 gate의 `original_provenance`에는 staging receipt의 `source`, `generation`,
`file_generation`, `relation`, `length`, `sha256` 필드만 들어 있다.
원래 device/inode 메타데이터나 feeder의 전체 `staging_link.original_provenance`를 반환하지 않는다.
`capture_time_verified: false`와 `deletion_check: "read_snapshot_not_delivery_lease"`를 명시적으로 설정한다.
거절 시에는 내용·참조를 생략한다.
query/check-local은 verified일 때만 종료 코드 0을 낸다. 나머지는 2다.
이 내용 제공 인터페이스에서는 대기 중인 삭제가 deleted/unverified로 나타날 수 있다.
사본 제거 완료 확인이 아니다.
외부 어댑터 자체의 원시·디버그 기능은 소비자 접근 인터페이스가 아니다.

## 삭제 조정자

```text
uasf-del --config CONFIG
```

설정에는 절대 `root`, `staging`, `intake`, 원본 `registry`, `intake_command`,
`archive_command`, 선택적 `command_timeout_ms`가 들어 있다.
명령은 절대 실행 파일을 담은 배열이다. 셸 텍스트를 사용하지 않는다.
archive 명령 인자에는 정확한 `{scope}` 자리표시자가 있어야 한다.
외부 아카이브 어댑터는 실제 실행으로 끝난 내용 제거를 보고해야 한다.
dry-run이나 단순한 큐 접수는 안 된다.

| JSON `op` | 입력·결과 |
|---|---|
| `init` | 영속 상태 생성·연결. `ok` |
| `state` | 버전 1 상태 객체. 합성 status를 추가하지 않음 |
| `request` | `selector`, `input_map`, 선택적 `request_id`. 고정된 대기 작업 |
| `resume` | 추가 필드 없음. complete 또는 pending 작업들 |
| `link_receipt` | `receipt_id`, `source`, `relative`. JSON null 확인 응답 |
| `put_cache` | `key`, `hex`. ok 또는 삭제 후 disabled |
| `read_cache` | `key`. ok, not found, pending 또는 deleted 내용 제공 결과 |

선택자는 등록 파일, 원본 세션, 텍스트, 명시적 receipt를 다룬다.
정확한 형식은 [조정자 크레이트](../../crates/uasf-del/README.md#selector-shapes)를 참고한다.
`input_map`은 절대 staging 경로를 검증된 아카이브 입력 ID 목록에 매핑한다.
request/resume의 pending 상태는 종료 코드 2를 낸다. complete는 0이다.
잘못된 작업·오류는 사유와 함께 unsupported를 반환하고 종료 코드 2를 낸다.
그 밖의 성공 작업은 종료 코드 0을 낸다.
link_receipt의 JSON null과 캐시 내용 제공 상태도 여기에 포함된다.
종료 코드뿐 아니라 응답도 확인한다.

선택적 통합에는 `core: {index,persistent}`와 `completion_command`가 있다.
연결은 새 저널에서 초기화한다. core 영속 상태에 유지한다.
기존 저널을 암묵적으로 업그레이드하지 않는다.
완료 요청은 고정한 작업이 담긴 `{"op":"complete_deletion","job":...}`다.
정확히 `{"status":"complete"}`인 응답과 종료 코드 0만 완료 확인으로 인정한다.
필요한 실행기는 신뢰하는 배포 통합 구성요소다. 범용 제품 도구로 제공하지 않는다.

## 선택적 큐

연결은 명시적으로 설정한다. 연결되지 않은 저장소는 일반 접수 제어를 사용한다.
core와 gate 작업자는 성공 응답 바이트를 보존하고 실행 시 접근 권한을 다시 검사한다.
큐 거절 사유에는 `query_queue_full`, `query_source_queue_full`, `query_deadline_exceeded`,
`query_duplicate_request`, `query_service_unavailable`, `query_service_stopping`,
`query_worker_failed`, `query_memory_limit`이 있다.
core는 unsupported/reasons를, gate는 unsupported/reason을 사용한다.
대기 요청은 삭제 리스를 보유하지 않는다.
서비스가 종료되어도 요청을 재실행하거나 암묵적으로 다른 경로로 전환하지 않는다.
서비스와 클라이언트에는 같은 빌드를 사용한다.
실험적 IPC에는 버전 협상이 없다.
[큐 설정과 생명주기](../../crates/uasf-queue/README.md#configuration-and-lifecycle)를 참고한다.
성능 승인 기준은 아직 충족하지 못했다. 운영에 권장하는 경로가 아니다.
