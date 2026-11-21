이 문서는 [영어판](../getting-started.md)의 번역이며, 내용이 다르면 영어판이 우선한다. 번역 기준 버전: **0.1.0**(Rust 크레이트 선언 버전).

# 시작하기

## 소스에서 빌드

Linux와 저장소에 포함된 잠금 파일에 호환되는 Rust 도구 체인을 사용한다.
저장소 루트에서 실행한다.
이 오프라인 빌드에는 잠금 파일이 지정한 의존성이 Cargo 캐시에 이미 있어야 한다.
의존성 확인에 실패하면 캐시를 별도로 채운다.
다음 명령은 일반 구성요소만 빌드한다.
Python 3는 합성 예시를 실행한다. Rust 구성요소는 독립적으로 실행된다.

```sh
export CARGO_TARGET_DIR="$(pwd)/../uasf-build"
cargo build --offline --locked -p uasf-core -p uasf-feeder -p uasf-staging -p uasf-intake -p uasf-gate -p uasf-del
export UASF_BIN_DIR="$CARGO_TARGET_DIR/debug"
export UASF_EXAMPLE_PARENT="$(pwd)/../uasf-example-data"
mkdir -p "$UASF_EXAMPLE_PARENT"
```

예시 상위 디렉터리는 모든 Git 체크아웃 밖에 있어야 한다.
staging은 저장소 안의 런타임 저장소를 거절한다.
테스트에는 새로운 비공개 상위 디렉터리를 사용한다.
원본 세션 위치, 인증 정보, 데몬, 소켓, 권한이 필요한 설치는 사용하지 않는다.

## 합성 작업 흐름 실행

아래 Python 블록을 `synthetic_example.py`로 저장한 뒤 `python3 synthetic_example.py`를 실행한다.
실제 Rust CLI를 호출해 응답 필드, 상태, 종료 코드, 원본 바이트 해시, 재시작 동작을 검사한다.
그런 다음 자신이 새로 만든 임시 디렉터리를 제거한다. 간단한 검사 보고서만 출력한다.
삭제에 연결하지 않은 합성 작업이다. unmanaged 플래그는 배포 권장 사항이 아니다.

<!-- executable-example: synthetic-workflow -->
```python
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

bin_dir = Path(os.environ["UASF_BIN_DIR"]).resolve()
parent = Path(os.environ["UASF_EXAMPLE_PARENT"]).resolve()
parent.mkdir(parents=True, exist_ok=True)
checks = []

def digest(data):
    return hashlib.sha256(data).hexdigest()

with tempfile.TemporaryDirectory(prefix="synthetic-", dir=parent) as scratch:
    root = Path(scratch)
    for name in ("home", "tmp", "config", "cache", "data", "original"):
        (root / name).mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
           "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"),
           "XDG_CONFIG_HOME": str(root / "config"),
           "XDG_CACHE_HOME": str(root / "cache"),
           "XDG_DATA_HOME": str(root / "data")}

    def call(binary, args, request, expected_status, expected_exit=0):
        raw = b"" if request is None else json.dumps(request).encode()
        result = subprocess.run([str(bin_dir / binary), *map(str, args)],
                                input=raw, capture_output=True, env=env,
                                cwd=root, timeout=30)
        value = json.loads(result.stdout)
        assert result.returncode == expected_exit, (binary, result.returncode)
        assert value["status"] == expected_status, (binary, value["status"])
        checks.append({"binary": binary, "status": value["status"],
                       "exit_code": result.returncode,
                       "response_bytes": len(result.stdout),
                       "response_sha256": digest(result.stdout)})
        return value

    record = {"type": "api-record", "schema": "uasf.synthetic-api/1",
              "conversation": "chat", "call_id": "call-1",
              "request": {"text": "synthetic input"},
              "response": {"text": "synthetic output"}, "complete": True}
    raw = (json.dumps(record, separators=(",", ":")) + "\n").encode()
    original = root / "original" / "record.jsonl"
    original.write_bytes(raw)
    capture = {"source": "app", "capture": "capture-1",
               "generation": "generation-1", "length": len(raw),
               "sha256": digest(raw), "session": "chat", "native_session": False}
    manifest = {"sources": [{"id": "app", "engine": "api"}],
                "captures": [{"capture": capture, "fixture_path": str(original)}]}
    core = ["--index", root / "index.sqlite", "--persistent", root / "persistent.sqlite"]
    rebuilt = call("uasf-core", core + ["rebuild"], manifest, "complete")
    assert rebuilt["captures_indexed"] == 1
    assert rebuilt["meaning"] == "rebuild_finished_not_history_completeness"
    sessions = call("uasf-core", core + ["query"], {"op": "sessions"}, "complete")
    session = sessions["data"]["sessions"][0]
    anchor = session["anchors"][0]
    assert anchor == {"source": "app", "capture": "capture-1",
                      "generation": "generation-1", "file_generation": None,
                      "start": 0, "end": len(raw), "sha256": digest(raw)}
    request = {"op": "history", "source": "app", "session": "chat"}
    missing = call("uasf-core", core + ["query"], request, "incomplete")
    assert missing["reasons"] == ["explicit_capture_boundary_required"]
    assert missing["data"] == {}
    request["boundary"] = session["capture_ids"]
    history = call("uasf-core", core + ["query"], request, "unknown")
    assert history["schema"] == "uasf-core.query/1"
    assert history["reasons"] == ["submitted_scope_only_total_conversation_unknown"]
    assert history["data"]["events"][0]["body"] == record
    assert history["data"]["events"][0]["anchors"] == [anchor]
    call("uasf-core", core + ["name"],
         {"source": "app", "session": "chat", "name": "Synthetic chat"}, "complete")
    deleted = call("uasf-core", core + ["delete"],
                   [{"kind": "session", "source": "app", "session": "chat"}], "deleted")
    assert deleted == {"status": "deleted", "requested": 1,
                       "body_policy": "explicit_erasure_and_reversible_dependency_blocking",
                       "scope": "core_index_and_durable_evidence_only"}
    core[1] = root / "replacement-index.sqlite"
    call("uasf-core", core + ["rebuild"], manifest, "complete")
    after = call("uasf-core", core + ["query"], request, "deleted")
    assert "synthetic input" not in json.dumps(after)
    assert original.read_bytes() == raw

    grants = root / "grants.json"
    grants.write_text(json.dumps({"app": ["writer"]}))
    intake = ["--database", root / "intake.sqlite", "--registry", grants,
              "--principal", "writer", "--unmanaged-test-mode"]
    submission = {"source": "app", "session": "chat", "key": "before-send",
                  "payload_base64": base64.b64encode(raw).decode()}
    first = call("uasf-intake", intake, submission, "accepted")
    second = call("uasf-intake", intake, submission, "accepted")
    assert not first["replayed"] and second["replayed"]
    assert first["receipt"] == second["receipt"]
    receipt = first["receipt"]
    assert receipt["payload_length"] == len(raw) and receipt["payload_sha256"] == digest(raw)
    get = {"op": "get", "source": "app", "receipt_id": receipt["receipt_id"]}
    fetched = call("uasf-intake", intake, get, "ok")
    assert base64.b64decode(fetched["payload_base64"]) == raw
    call("uasf-intake", intake, {"op": "erase", "receipt_ids": [receipt["receipt_id"]]}, "deleted")
    call("uasf-intake", intake, submission, "deleted", 2)
    call("uasf-intake", intake, get, "deleted", 2)

    # A synthetic JSONL file exercises byte staging, not native parser conformance.
    original.write_bytes(raw + b'{"held":')
    registry = root / "sources.json"
    registry.write_text(json.dumps({"version": 1, "sources": {"native": {
        "kind": "codex", "root": str(original.parent), "files": [original.name]}}}))
    staging = ["--root", root / "stage", "--registry", registry, "--unmanaged-test-mode"]
    published = call("uasf-staging", staging + ["capture", "native", original.name], None, "published")
    staged_files = list((root / "stage").rglob("*.jsonl"))
    assert len(staged_files) == 1 and staged_files[0].read_bytes() == raw
    call("uasf-staging", staging + ["capture", "native", original.name], None, "unchanged")
    call("uasf-staging", staging + ["delete", "native", original.name], None, "deleted")
    call("uasf-staging", staging + ["capture", "native", original.name], None, "deleted", 2)
    assert not list((root / "stage").rglob("*.jsonl"))

print(json.dumps({"status": "pass", "invocations": len(checks), "checks": checks}))
```

예상 결과는 `status: "pass"`와 **18회** 호출을 담은 최종 JSON 보고서 하나다.
각 호출에서 종료 코드와 상태를 검사한다.
경로, receipt ID, 수신 시각이 달라지므로 응답 크기·해시는 관측값으로 보고한다.
검사는 정확한 원본 바이트·앵커, 재실행 결과의 동일성, 삭제 범위, 재구축 후 비공개 유지, 꼬리 부분 보류를 확인한다.
원본 파싱, 아카이브 읽기, feeder 전송, 사용자 간 통합에는 별도 시험이 필요하다.

## 테스트와 정리

저장소 루트에서 `TMPDIR`을 체크아웃 밖의 새 디스크 디렉터리로 설정한다. gate 시험은 이 디렉터리와 상위 디렉터리에 그룹·전체 쓰기 권한이 없어야 통과한다(root 소유 sticky `/tmp`는 허용). `mktemp -d`로 만든 디렉터리면 된다.
그런 다음 `cargo test --offline --locked`를 실행한다.
테스트는 합성 데이터를 사용한다.
core는 고정 합성 적합성 자료를 Rust에서 직접 재실행한다.
소켓·사용자 분리 배포에는 각각의 환경 검증이 필요하다.
Python 예시는 자신이 만든 디렉터리만 제거한다.
필요하면 요약을 보관한 뒤 빈 예시 상위 디렉터리와 스크립트를 제거한다.
테스트 디렉터리를 제거했다고 제품 전체 삭제가 확인된 것은 아니다.

실행 가능한 gate·archive·삭제 완료 설치 예시는 제공하지 않는다.
외부 브리지, 아카이브 배포 환경, 범용 완료 실행기가 공개 트리에 없기 때문이다.
이 통합 책임은 [운영](operations.md#배포-전제-조건)을 참고한다.
시험하지 못한 범위를 통과로 취급하지 않는다.
