이 문서는 [영어판](../validation.md#current-status)의 번역이며, 내용이 다르면 영어판이 우선한다. 번역 기준 버전: **0.3.0**.

# 검증 기록 — 2026-11-01

## 현재 상태

이 요약은 **0.2.0** 기준이다. 0.3.0의 실험 Antigravity 엔진은 뒤쪽 절과 [agy.md](agy.md)에 기록했다.
과거 측정은 아래에 연결한 영어판에 변경 없이 남아 있다. 원래 범위와 표현은 당시 릴리스를 설명한다.
**I**는 구현됨, **S**는 합성 테스트로 검증됨을 뜻한다.
**N**은 명시한 표본 안에서만 원본 환경 검증을 마쳤다는 뜻이다.
**N-copy**는 보존한 receipt 사본을 다시 읽어 원본 자료로 검증했다는 뜻이다.
엔진 전반의 호환성이나 의미상 활용을 주장하는 표시는 아니다.
단계별 범위는 [작업 흐름 태그](workflow.md#구현된-작업-흐름)를 참고한다.

| 기능 | 구현됨 | 합성 테스트로 검증됨 | 원본 환경 검증 증거와 한계 |
|---|---|---|---|
| 두 엔진과 오프라인 조회 | I: Codex 기본값, Claude headless exec | S: Python 3.14.4 / 3.10.22에서 81개 테스트. 엔진 혼합 저장소와 원본 ID 충돌 포함 | N: [0.2.0 일련의 실행과 보완용 직접 실행](../validation.md#version-020--two-engine-headless-receipts), 호스트 한 대 |
| CLI 보호 규칙과 호환성 | I: 래퍼가 관리하는 인자 보호, 최상위·run 옵션 축약 거절, 변경 불가능한 기존 번들 | S: 미지원·래퍼 관리·축약 옵션, 기존 Codex 테스트 자료, 엔진별 재시도 그룹 | --sock/--cod 회귀 검증은 합성 증거다. 원본 환경 전송 테스트가 아님 |
| Codex exec, resume, 데몬 | I: 기존 사실·종료 규칙 유지 | S: 기존 회귀 사례 유지 | N: Codex 0.166.0 new/resume과 보완용 직접 실행. 과거 Codex 0.163.0 데몬의 새 스레드/notLoaded 증거 |
| Claude new/resume, 입력과 결과 | I: 전송 전에 확정한 세션 UUID, 접두부 경계, 사용자 메시지 키, reported 결과 | S: 정상, 여러 assistant 메시지, 오류, 중단, 결과 유실, 식별 정보 불일치 | N: Claude Code 2.1.310. 릴레이 14개 사례 표본에 중립성·위임 사례와 새 직접 실행 1회 추가 |
| Claude 부모 연결과 개인정보 보호 | I: 조건을 충족하는 parentUuid 연결 사슬과 파생 정보 | S: attachment를 통한 연결, 다른 user에서 연결 차단, 끊긴·순환·범위 밖 연결 사슬, 민감한 줄 생략, 원본 없이 재도출 | N: 원본과 원장의 독립 비교에서 differences=[]. 비공개 원본 행을 복사하지 않고 선별한 필드·범위·해시 검사 |
| 무결성, 게시, 권한, forget | I: 원자적 초기 게시와 체크포인트 연결 사슬 | S: 이름 변경 전 비정상 종료, 손상, 잠금, 0700/0600, 삭제 | 원본 환경 표본은 비정상 종료 시 원자성을 입증하지 않음. 근거는 합성 테스트 |
| Codex 맥락 압축과 스트림 대체 경로 | I: 턴 연결 보존, observed 우선 사실·출력 | S: 큰 맥락 압축, ID 불일치, 안전한·민감한·크기 초과 스트림 후보 | N-copy: Codex 0.163.0 사본 복구. Codex 스트림만 이용한 대체 출력은 여전히 합성 테스트로만 검증 |
| 지연 관측과 복구 | I: 제한된 재조회, 재전송 없음 | S: 250 ms 지연 기록 프로세스와 출력 설정 변경 | N-copy: 과거 Codex 복구. N: Claude 래퍼 강제 종료·유실 뒤 복구는 입력을 확인하지만 유실된 결과를 재구성하지 못함 |
| 설치와 내보내기 | I: 설치 가능한 Python 패키지, JSONL과 선택적 Polylogue | S: JSONL과 브리지 누락 사례 | 과거 uv 설치와 Polylogue 재조회. 0.2.0은 소스에서 실행했으며 전역으로 재설치하지 않음 |
| 유용성과 모델 활용 | I: 모델 활용은 항상 unknown | 테스트로 의미상 활용을 증명할 수 없음 | 과거 Codex 위임 12건과 중복 판단 2건. 0.2.0의 일련의 실행은 기능 증거이며 새로운 유용성 평가가 아님 |

현재 원본 환경 증거는 호스트 한 대의 Claude Code **2.1.310**, Codex CLI **0.166.0** 표본이다.
이전 Codex **0.163.0** 증거는 따로 보존한다.
19회 실행에는 릴레이 실행 파일을 사용했다. 제품의 프로세스 사실은 그 릴레이 자식 프로세스를 가리킨다.
이후 직접 실행 2회가 이 한계를 줄인 범위는 정상적인 새 세션뿐이다.
이번 문서 갱신의 확인 범위는 소스와 저장된 증거 검토에 한정한다.
버전·호스트 전반의 호환성은 입증되지 않았다.

날짜순 검증 로그는 [영어판의 설치·합성 검사 이후 기록](../validation.md#installation-and-synthetic-checks)을 참고한다.

## 0.3.0 — 실험적 Antigravity 합성 검증

native 필드 맵은 **verified 1.5.9**와 [엔진 안내](agy.md)의 실행 파일 해시에 고정된다.
이번 구현에서는 실제 agy 실행·모델 호출이 모두 **0회**였고, native 인증·이력 디렉터리를 열지 않았다.
이후 운영자가 수행한 native 실측은 아래에 기록했다. 합성 시험에서만 메모리의 verified pin을 fake 해시로
대체한다. 제품 실행 시 검증 우회 옵션은 없다.

기존 81개를 유지하고 신규 40개를 추가하여 **전체 121개**다.
신규 모듈 단독 실행은 Python 3.14.4에서 `Ran 40 tests ... OK`, exit 0이다.
전체 실행은 Python **3.14.4 / SQLite 3.46.1**, **3.10.22 / SQLite 3.53.1**에서 모두
`Ran 121 tests ... FAILED (errors=7)`, exit 1이다.
7개 오류는 기존 daemon 테스트 6개 메서드(하나는 하위 사례 2개)의 Unix 소켓 bind를 샌드박스가
`PermissionError: [Errno 1] Operation not permitted`로 거절한 것이다.
변경 전 81개에서도 같은 오류 7개가 있었다. 테스트 삭제·skip·완화는 하지 않았다.
**전체 통과 완료 조건은 이 환경 제약으로 미충족 상태다.**

패키지 디렉터리에서 실행한 명령:

```sh
PYTHONPATH=src python3 -m unittest discover -s tests -p test_agy.py
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src UV_CACHE_DIR=/tmp/handoff-receipts-ag1-uv uv run --no-project --python 3.10 python -m unittest discover -s tests
PYTHONPATH=src python3 -m handoff_receipts run --help
PYTHONPATH=src python3 -m handoff_receipts --version
```

help와 version은 exit 0이다. 각각 `--engine {codex,claude,agy}`·`--agy AGY`, `0.3.0`을 출력한다.
금지 passthrough CLI 테스트는 fake 전송 없이 2를 반환한다.
신규 시험은 정상·긴 Unicode exact 입력·동일 본문 resume·없는/잘못된 resume 사전 거절·
반환/stream ID 충돌·ID 없는 오류·복수/없는/손상 result·fake에 대한 실제 SIGINT 전달·timeout·
성공/취소 충돌·LF 차이·손상/중복/wire type 오류·SQL 불일치·미지 필드·WAL·실제 SQLite busy·
지연 commit/recover·늦은 executor 취소 commit·버전/해시 불일치·실행 중 해시 변경·민감 필드 제외·오프라인 검증·
projection/fragment 불일치를 포함한다. 고정 wire fixture는 fake encoder와 별도로 손으로 작성했다.
합성 시험은 해석 정책과 로컬 동작의 검증이며 현재 native 바이너리나 모델 동작 검증은 아니다.


## 2026-11-15 Antigravity native 실측과 후속 수정

운영자는 **2026-11-15**에 Antigravity CLI **1.5.9**, 고정 실행 파일 해시,
모델 **gemini-3.8-flash-low**로 실행 시도 8회를 보고했다.
모델 프로세스 7개와 전송 전 거절 1건이다.

| 사례 | 기록된 native 결과와 후속 해석 |
|---|---|
| 새 대화 | 입력 observed, completed reported, native NO_TOOL_CALL |
| 같은 대화 resume | 입력 observed, completed reported, native NO_TOOL_CALL |
| 약 10 KB Unicode 입력 | 입력 observed, completed reported, native NO_TOOL_CALL |
| invalid model | conversation ID 없음, 상태 unknown, 래퍼 exit 2 |
| 래퍼에 SIGINT | interrupted reported, native USER_CANCELED, 입력 observed, 자식 exit 1 |
| stream-json 입력의 print-timeout | SUCCESS, finished_reason=print_timeout, 빈 response, 자식 exit 0. 수정 전에는 verified 버전의 취소 충돌 규칙으로 unknown이었다. 수정 후 재실측(아래)에서 failed/reported와 래퍼 exit 2를 확인했다. |
| 래퍼 SIGKILL 뒤 recover | conversation ID 보존, result가 없어 상태 unknown. 수정된 recover는 저장된 실행 파일의 현재 fingerprint를 기록해 native 입력 확인에 사용한다. |
| 없는 resume 대상 | 시작 전 거절, 전송 없음 |

completed 세 사례의 native response 해시와 reported 출력 해시는 마지막 LF 차이로 달랐으며 예상된 결과다.
선행 argv 모드의 timeout probe는 ERROR와 자식 exit 3을 냈다.
이번 stream-json 입력에서는 위 표의 SUCCESS 형태가 나왔다.

현재 판정은 status와 reason을 함께 사용한다. SUCCESS의 reason 부재·null은 completed,
print_timeout은 failed, 그 밖은 unknown이다. ERROR의 interrupted는 interrupted,
그 밖은 부재·null을 포함해 failed다. 다른 status는 unknown이다.
control은 reason 부재·null과 미지 문자열을 구별해 저장한다.

recover는 저장된 동일 실행 파일의 --version 메타데이터 검사와 현재 해시를 agy_version_recover에 기록한다.
verified before와 verified after가 필요하며 after가 없으면 가장 최근 verified recover를 사용한다.
모든 버전·해시가 pin과 같아야 하며 이전 불일치는 이후 일치하는 recover로 지워지지 않는다.
after가 없으면 before와 가장 최근 recover의 실행 파일 st_dev, st_ino, st_size,
st_mtime_ns, st_ctime_ns도 모두 같아야 한다. 누락되면 unverified다.
result가 유실되면 상태는 unknown을 유지하며, 위 조건을 충족한 recover-time fingerprint를 native 입력 확인에 사용한다.

후속 수정 검증에는 fake producer를 사용했다. verified/unverified의 정확한 SUCCESS timeout 형태,
after 유실 복구, recover fingerprint 불일치와 이력 검증을 포함해 회귀 테스트 6개를 추가했다.
기존 결과 형식 테스트도 finished_reason 부재·null·빈 값·잘못된 형식·미지 값을 검사한다.
합성 테스트 명령과 결과는 아래에 기록한다.


후속 수정 검증은 기존 121개를 유지하고 6개를 추가해 전체 127개다.
Python **3.14.4**, **3.10.22** 모두 agy 모듈 **46개 테스트가 통과**했다.
Python 3.14.4의 전체 실행은 `Ran 127 tests ... FAILED (errors=7)`이다.
오류는 기존 daemon 테스트의 알려진 Unix 소켓 bind PermissionError 7개다.
운영자는 해당 daemon 테스트가 자신의 환경에서 통과한다고 보고했다.

| 패키지 디렉터리에서 실행한 명령 | 종료와 결과 |
|---|---|
| PYTHONPATH=src python3 -m unittest discover -s tests -p test_agy.py | 0, 46 tests OK |
| PYTHONPATH=src UV_CACHE_DIR=/tmp/handoff-receipts-ag1-uv uv run --no-project --python 3.10 python -m unittest discover -s tests -p test_agy.py | 0, 46 tests OK |
| PYTHONPATH=src python3 -m unittest discover -s tests | 1, 전체 127개, 알려진 소켓 PermissionError 7개 |

후속 수정 검증에서는 fake producer와 그 fake에 대한 메타데이터 검사를 실행했다.
리뷰 회귀 수정 후의 native 재실측은 아래에 기록했다.


## 2026-11-15 Antigravity 검토 회귀

어댑터 수정 전에 합성 회귀 메서드 8개를 추가했다. 이전 코드에서 8개 지정 실행은
exit 1, `Ran 8 tests`, 하위 사례를 포함해 `FAILED (failures=25, errors=3)`였다.
각 메서드에서 실패 기대값을 확인했다. 수정 후 같은 8개는 exit 0, `Ran 8 tests ... OK`다.

아래 메서드는 모두 `tests/test_agy.py`의 `AG1Test`에 있다.

| 항목 | 테스트 메서드 | 수정 전 재현 |
|---|---|---|
| R1 | test_r1_recover_pins_input_after_same_body_resume | 동일 본문 resume 후 첫 attempt recover에 ambiguous_target_match 발생 |
| R2 | test_r2_recover_preserves_stream_report_and_output | null reason 사례가 recover 후 unknown이었다. 정상 reason 부재 사례는 수정 전에도 보고를 보존했다. 수정 후 둘 다 native 재조회 실패를 거쳐 control과 completed 출력을 보존한다. |
| B1 | test_b1_recover_requires_unchanged_executable_stat | stat 누락·차이에도 verified였으며 fingerprint에 stat이 없었다. bytes·mtime 복원 후 ctime 변경 사례도 검사한다. |
| B2 | test_b2_status_reason_matrix | SUCCESS/null, SUCCESS/interrupted, ERROR/그 밖, 미지 status가 판정표와 달랐음 |
| B3 | test_b3_incomplete_input_stream_blocks_capture_and_recover | user_input conversation ID 누락·손상 stream에도 native 입력 확인 |
| B4 | test_b4_omitted_planner_does_not_stop_executor_rereads | 민감한 planner 제외가 재조회를 멈춰 지연 executor 취소를 놓침 |
| B5 | test_b5_invalid_utf8_success_is_malformed | UTF-8 불가 SUCCESS response가 completed였음 |
| B6 | test_b6_relative_path_uses_child_cwd_for_every_execution_check | 자식 cwd·상대 PATH에서 argv 실행 파일이 상대경로였음 |

기존 127개를 유지하고 8개를 추가해 **전체 135개**다.
Python **3.14.4**, **3.10.22** 모두 agy 모듈 **54개 테스트가 통과**했다.
Python 3.14.4 전체 실행은 exit 1, `Ran 135 tests ... FAILED (errors=7)`이다.
기존 daemon 테스트의 알려진 Unix 소켓 bind PermissionError 7개다.

| 패키지 디렉터리에서 실행한 명령 | 종료와 결과 |
|---|---|
| PYTHONPATH=src python3 -m unittest discover -s tests -p test_agy.py | 0, 54 tests OK |
| PYTHONPATH=src UV_CACHE_DIR=/tmp/handoff-receipts-ag1-uv uv run --no-project --python 3.10 python -m unittest discover -s tests -p test_agy.py | 0, 54 tests OK |
| PYTHONPATH=src python3 -m unittest discover -s tests | 1, 전체 135개, 알려진 소켓 PermissionError 7개 |

fingerprint에 stat 다섯 값을 기록하며, after가 없을 때 before/recover stat 일치와
verified 버전·해시를 모두 요구한다. 자식 cwd와 상대 PATH 기준으로 실행 파일을 한 번 절대경로로 고정한다.
recover의 reported는 저장된 stream control로 계산하며 reason 부재·null과 미지 문자열을 구분한다.
native 귀속은 이 attempt의 완전한 stream index 근거를 사용한다.
보조 필드를 제외해도 executor 대기를 예산 안에서 재조회하며, UTF-8 불가 SUCCESS response는 malformed다.
제한 없는 로컬 환경에서 Python 3.14.4 전체 탐색은 `Ran 135 tests ... OK`, exit 0이었다.

## Antigravity 수정 후 native 재실측 — 2026-11-15

Antigravity CLI 1.5.9(고정 해시), 모델 `gemini-3.8-flash-low`, 모델 프로세스 4개, 자동 재시도 없음.
수정 전에 쓴 receipt는 이전 reason·fingerprint 인코딩이라 여기 근거로 쓰지 않았다.

| 경우 | 결과 |
|---|---|
| 새 대화 | 입력 observed, completed reported, native NO_TOOL_CALL |
| 그 대화의 같은 본문 resume | 입력 observed, completed reported, 새 execution ID |
| resume 뒤 첫 attempt recover | 같은 execution, completed와 출력 해시 보존, 모호성 없음 |
| stream-json 입력의 print timeout | failed reported, native USER_CANCELED, 자식 exit 0, 래퍼 exit 2 |
| 래퍼 SIGKILL 뒤 recover | 실행 파일 stat 불변으로 버전 verified, 입력 observed, native가 나중에 NO_TOOL_CALL이어도 result가 없어 상태 unknown |

## 지속적 통합(CI)에서 찾은 문제 (2026-11-16)

첫 CI 실행(Ubuntu 24.04, Python 3.10과 3.14.10)에서 문제 두 가지가 드러났다. 첫 공개 전이라 버전은 바꾸지 않고 둘 다 고쳤다.

- 깊게 중첩된 JSON 줄이 항상 `RecursionError`를 낸다고 가정한 시험이 있었다. Python 3.14.4에서는 오류가 났지만 3.14.10은 그 줄을 정상으로 해석했다. 이제 시험은 두 결과를 모두 받아들이고, 그 줄이 unrecognized로 기록되는지만 확인한다.
- Codex 스트림 경로는 어떤 JSON 값이든 받은 뒤, 객체가 아닌 유효한 줄(`[0]`, `5`, `null` 등)에서 `AttributeError`로 실패했다. 이제 Claude·Antigravity 경로처럼 그런 줄을 `stream_unrecognized`로 기록하고 capture를 계속한다. 이 경우를 다루는 합성 시험을 새로 추가했다.

- 두 번째 CI 실행과 로컬 Python 3.10 반복 실행에서 시간에 따라 결과가 바뀌는 Antigravity 시험 두 개(`busy`, `delayed_commit`)가 드러났다. 가짜 기록기가 고정 0.3초 뒤에 잠금을 풀거나 커밋해서, 느린 실행에서는 첫 관측이 시험하려는 상태를 놓칠 수 있었다. 이제 가짜 기록기는 첫 관측이 끝난 뒤 시험이 보내는 해제 신호를 기다린다.

수정 뒤 두 시험은 Python 3.10.22와 3.14.4에서 각각 10회 중 10회 통과했고, CPU 부하를 건 3.10에서도 10회 중 10회 통과했다. 이어서 두 버전 모두 전체 160개 시험을 세 번씩 돌려 모두 `OK`였다.
