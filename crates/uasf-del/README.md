# UASF deletion coordinator

Owns a durable local deletion journal with fixed targets and retryable effects.
It uses staging's library, an intake CLI and a trusted external archive executable.

See [coordinator CLI](../../docs/reference.md#deletion-coordinator),
[deletion semantics](../../docs/deletion.md), [operations](../../docs/operations.md)
and [executor limits](../../docs/security-and-limits.md#other-component-limits).

## Selector shapes

Selectors have these exact shapes (synthetic illustrative values). These are
request fragments, not runnable deployment examples; execution also requires registered stores, verified input
mappings and an external archive executor:

```json
{"kind":"file","source":"s","relative":"sessions/rollout-x.jsonl"}
```

```json
{"kind":"session","source":"s","session":"session-reference"}
```

```json
{"kind":"text","text":"literal text"}
```

```json
{"kind":"receipt","source":"s","receipt_id":"receipt-reference"}
```

`input_map` maps each selected absolute staged path to all verified archive input
IDs. Missing mappings reject planning before durable intent. Explicit receipt links
supply provenance; equal hashes alone do not. The `resume` operation takes no job
selector and resumes outstanding jobs.

## Executor interfaces

Intake receives `{"op":"erase","receipt_ids":[...]}`. Only exit zero with exactly
`{"status":"deleted"}` acknowledges it. The coordinator substitutes `{lock_fd}`
with its inherited capability descriptor in configured intake arguments. Intake
must validate that capability against the current binding and lock.

Archive arguments contain the exact `{scope}` placeholder. The trusted wrapper
must address an already running owned daemon and await actual excision. No implicit
startup or enqueue-only success is accepted: exit zero, `dry_run: false`, and no
error/queued report are required. The wrapper is deployment configuration, not
caller-supplied code. Executors must tolerate repetition after an effect whose
acknowledgement was interrupted.

Optional `integration` binds core's persistent store and a completion command on
a fresh journal. It adds completion after archive and before blocklist. The frozen
job is sent as `op: complete_deletion`; only exact `status: complete` with exit zero
acknowledges it. This includes required offline-producer purge confirmation. A
general completion executor or provenance discovery implementation is not supplied.

All executor paths bound stdin/stdout/exit time, kill the process group and reap
the direct child. Other descriptors remain close-on-exec. Deliberately escaping
the group or forwarding the capability is outside this trusted-executor interface.

## Build and tests

From the repository root: `cargo build --offline --locked -p uasf-del` and
`cargo test --offline --locked -p uasf-del`. Use a dedicated disk-backed `TMPDIR`.
Tests include real child exits between durable steps, hanging executors and retry,
final-save failure recovery, and [fixed synthetic conformance data](tests/golden.json).
The `examples/probe.rs` executable is a callback/crash test driver, not a production completion executor. Live archive
deletion and privileged installation require separate integration validation.
