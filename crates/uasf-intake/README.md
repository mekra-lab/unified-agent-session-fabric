# UASF intake

Preserves application-submitted bytes in SQLite with source-scoped retry keys,
stream observations and durable deletion tombstones. It does not produce native
transcripts or call a provider.

See [intake CLI](../../docs/reference.md#intake), [observation semantics](../../docs/concepts.md),
[deletion](../../docs/deletion.md) and [operations](../../docs/operations.md).
The [synthetic example](../../docs/getting-started.md) checks replay and erasure.

## Library interface

`Intake::open(path, grants)` accepts a registry mapping source IDs to trusted
principals. `execute(request, principal)` rechecks deletion admission before an
operation. Principal is supplied by trusted local configuration, not authenticated
by an arbitrary request. `erase` on a bound store requires the inherited coordinator
capability. It verifies the descriptor's device/inode, independent shared-lock
probe and exclusive flock ownership, then restores close-on-exec. The descriptor
does not authorize ordinary submit/get operations.

SQLite stores the immutable envelope, payload BLOB and receipt together, using
immediate transactions, DELETE journals, FULL synchronization and secure deletion.
Successful mutations include parent-directory fsync. Directory permissions remain
the caller's responsibility; media/backups are outside the erasure guarantee.

## Build and tests

From the repository root: `cargo build --offline --locked -p uasf-intake` and
`cargo test --offline --locked -p uasf-intake`. Use a dedicated disk-backed `TMPDIR`.
Native and fixed synthetic conformance tests cover canonical envelopes, exact bytes,
key conflicts, partial streams, restart, tombstone replay and coordinator admission.
