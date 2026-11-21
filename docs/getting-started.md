Korean: [Korean translation](ko/getting-started.md)

# Getting started

## Build from source

Use Linux and a Rust toolchain compatible with the checked-in lockfile. Run from
the repository root. This offline build needs the locked dependencies already
present in your Cargo cache; populate that cache separately if resolution fails.
The following builds only the ordinary components.
Python 3 drives the synthetic example; the Rust components run independently.

```sh
export CARGO_TARGET_DIR="$(pwd)/../uasf-build"
cargo build --offline --locked -p uasf-core -p uasf-feeder -p uasf-staging -p uasf-intake -p uasf-gate -p uasf-del
export UASF_BIN_DIR="$CARGO_TARGET_DIR/debug"
export UASF_EXAMPLE_PARENT="$(pwd)/../uasf-example-data"
mkdir -p "$UASF_EXAMPLE_PARENT"
```

The example parent must be outside any Git checkout: staging refuses runtime
storage under a repository. Use a new private parent for tests. No native-session
locations, credentials, daemon, socket or privileged installation are used.

## Run a synthetic workflow

Save the following Python block as `synthetic_example.py` and run
`python3 synthetic_example.py`. It invokes the actual Rust CLIs, checks response
fields, statuses, exit codes, original byte hashes and restart behavior, then
removes its own fresh scratch directory. It prints only a compact check report.
This is unbound synthetic operation; unmanaged flags are not deployment advice.

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

Expected: one final JSON report with `status: "pass"` and **18** invocations.
Each invocation checks exit code and status; response sizes/hashes are reported
as observations because paths, receipt IDs and receive times vary. The
assertions cover exact original bytes/anchors, replay equality, deletion scope,
rebuild non-disclosure and tail withholding. Native parsing, archive reads, feeder
transport and cross-user integration require separate tests.

## Tests and cleanup

From the repository root, set `TMPDIR` to a new disk-backed directory outside the
checkout, then run `cargo test --offline --locked`. The gate tests require that
directory and its parents to be free of group and world write permission (a
root-owned sticky `/tmp` is accepted); `mktemp -d` gives a suitable directory. The tests use synthetic data;
core replays fixed synthetic conformance data directly in Rust. Socket
and separate-user deployments require their own environment validation.
The Python example removes only the directory it created; retain its summary if
needed, then remove an empty example parent and the script. Removing a test directory
is not a product-wide deletion acknowledgement.

No executable gate/archive/deletion-completion installation example is provided:
the external bridge, archive deployment and general completion executor are missing
from the public tree. See [Operations](operations.md#deployment-prerequisites) for
those integration responsibilities. Do not treat that missing coverage as a pass.
