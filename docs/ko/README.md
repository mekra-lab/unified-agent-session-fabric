이 문서는 [영어판](../README.md)의 번역이며, 내용이 다르면 영어판이 우선한다. 번역 기준 버전: **0.1.0**(Rust 크레이트 선언 버전).

# UASF 문서

UASF는 관측 가능한 에이전트 기록을 보존하고 바이트 앵커 기반 세션 보기를 도출한다.
이 문서들이 현재 공개 제품 계약이다.
구현은 실험 단계이며, 검증 범위는 아래에 설명한 내용으로 한정된다.

## 읽는 순서

1. [작업 흐름](workflow.md): 쉬운 요약, 개념적 흐름, 상태 태그가 붙은 구현 경로.
2. [개념](concepts.md): 식별, 관측, 검증, 증거.
3. [구조](architecture.md): 구성요소 경계와 영속 상태.
4. [시작하기](getting-started.md): 소스 빌드와 실행 가능한 합성 예시.
5. [운영](operations.md): 등록, 재시작, 복구, 철거.
6. [참조](reference.md): 현재 인터페이스, JSON, 응답 규칙.
7. [삭제](deletion.md): 삭제, 제공 보류, 조정된 완료.
8. [보안과 한계](security-and-limits.md): 신뢰 경계와 강제되는 제한.

독립적인 [handoff-receipts CLI](../../tools/handoff-receipts/README.ko.md)는 로컬 Codex·Claude·실험적 Antigravity headless 전송 증거를 제공한다.
검증 범위는 자체 패키지 문서에 적혀 있다.
이 문서들에서 receipt는 받은 내용과 관측한 사실을 나중에 검증할 수 있게 남긴 기록을 뜻한다.

## 구현 상태

| 구성요소 | 구현 범위 | 조건과 한계 |
|---|---|---|
| [Core](../../crates/uasf-core/README.md) | 등록된 Codex 창, Claude 관측 그래프, 제출된 API 호출. 영속 증거와 삭제 | 명시적으로 등록한 바이트의 오프라인 파생 보기 |
| [Feeder](../../crates/uasf-feeder/README.md) | 인증된 등록 원본 스냅샷과 수집기 검증 | 명시적 OS 역할과 사전에 준비한 키 필요 |
| [Staging](../../crates/uasf-staging/README.md) | 변경 불가능한 완전한 줄 접두부와 receipt | 외부 아카이브 수집 필요 |
| [Intake](../../crates/uasf-intake/README.md) | 원본별 범위의 애플리케이션 바이트, 재시도, 스트림, 삭제 표식 | 원본 대화 기록을 만들지 않음 |
| [Gate](../../crates/uasf-gate/README.md) | receipt에 연결된 읽기와 삭제 검사 | 외부 아카이브 브리지 실행 파일 필요 |
| [삭제 조정자](../../crates/uasf-del/README.md) | 영속 의도, 재시도 가능한 단계, 선택적 후속 완료 | 범용 배포용 완료 실행기는 제공하지 않음 |
| [JSON](../../crates/uasf-json/README.md) | 공통 제한형 디코더와 정규형 값 처리 | 참조 문서의 숫자 계약 참고 |
| [Queue](../../crates/uasf-queue/README.md) | 명시적으로 선택하는 로컬 접수 제어 서비스 | 실험 기능. 성능 승인 기준을 **충족하지 못함** |
| [Index probe](../../crates/uasf-index-probe/README.md), [page probe](../../crates/uasf-page-probe/README.md) | 합성 데이터 기반 실현 가능성 확인 도구 | 운영 설치에서 제외 |

**page probe를 실제 기록에 사용하거나 소비자에게 노출하지 않는다.**
정책 오버레이가 core 접근 정책으로 차단된 본문을 반환할 수 있다.
두 probe 모두 지원되는 제품 페이지 나누기나 비동기 준비 계약을 정의하지 않는다.

고정된 합성 적합성 자료와 OS 사용자를 분리한 통합 검사로 구성요소 연결과 삭제를 시험한다.
명시적으로 등록한 종료된 원본 세션 두 개를 사용한 제한적 통합 시험도 통과했다.
Claude 하나와 Codex 하나다.
수집, receipt로 검증한 읽기, core 해석, 시험용 사본 삭제, 재유입 거절, 철거를 다뤘다.
원본 파일은 바뀌지 않았고 큐 연결도 없었다.
이는 해당 두 세션에 대한 증거일 뿐이다.
완전한 설치 프로그램, 일반적인 수집 준비, 임의의 파서 버전 호환성, 성능 SLA를 입증하지 않는다.
실제 세션의 아카이브 삭제에는 원인을 밝히지 못했고 재현되지 않은 실패 한 건이 있다.
따라서 아카이브 삭제 완료를 확인해야 한다.
UASF는 중앙 UI나 Hub와 독립적으로 실행된다.
UASF는 UASS에서 파생된 프로젝트다. 범위와 적합성 상태는 [UASS와의 관계](../../README.ko.md#uass와의-관계)를 참고한다. 라이선스는 Apache-2.0이다([LICENSE](../../LICENSE)). 릴리스 패키징과 기여 정책은 아직 정하지 않았다.
