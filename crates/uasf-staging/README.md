# UASF staging

Publishes immutable complete-line JSONL prefixes and version-1 receipts from
explicitly registered roots, independently of archive engines. External
collection consumes published files; the gate checks receipt binding.

See [staging CLI and registry](../../docs/reference.md#staging),
[capture and recovery](../../docs/operations.md),
[deletion admission](../../docs/deletion.md), and [security and limits](../../docs/security-and-limits.md).

## Library interface

`Store::new(root, registry)` validates absolute disjoint roots and registry version.
Runtime roots under a Git checkout are refused. `capture`, `scan`, `delete` and
`state` share recovery and locking. `with_capture` permits the feeder to supply a
validated copy under its capture lease; lock order is deletion domain, feeder
policy, feeder state, then staging. The original provenance remains distinct from
the collector file's identity. A bound CLI cannot use local delete as a coordinator
bypass. The explicit unmanaged option is for unbound synthetic stores only.

## Build and tests

From the repository root: `cargo build --offline --locked -p uasf-staging` and
`cargo test --offline --locked -p uasf-staging`. Set `TMPDIR` outside the checkout
on disk. Native tests include nofollow opening, exact size limits, concurrent
writers, process-exit recovery, generation classification and fail-closed deletion
inspection. Fixed synthetic conformance data checks receipts, state and published
bytes. Cross-user deployment and archive ingestion require separate integration
validation.
