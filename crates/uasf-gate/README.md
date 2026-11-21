# UASF receipt gate

A local JSON socket service and collector-side checker. It validates staging
receipts and full byte hashes before exposing archive records. It invokes a trusted
external bridge executable and is independent of archive-engine crates.

See [gate CLI and protocol](../../docs/reference.md#gate),
[deployment prerequisites](../../docs/operations.md#deployment-prerequisites),
[deletion](../../docs/deletion.md) and [limits](../../docs/security-and-limits.md).
The external bridge and archive deployment are not supplied in this workspace.

## Verification sequence

The gate holds deletion admission and staging locks while recovering receipts,
checking source facts and complete hashes, locating the immutable archive binding,
verifying capture, reading and hashing returned bytes, and rechecking excisions.
Anchor/resolve requests also require full verification. Receipt source/path/
generation binding and integer range checks are independent of a supplied hash.
Private bridge temporary storage and a cleared child environment keep adapter
execution separate from consumer inputs. Consumers receive the gate socket, not
raw bridge access, registry files or archive paths.

Ordinary `serve` handles connections serially and holds an admission slot for its
lifetime. Scheduled `serve` instead forwards requests through queue admission and
holds no execution slot or deletion lock while they wait.
`check-local` performs the same verification without a consumer socket. Neither
is a general authorization service for arbitrary untrusted source registries.
An experimental explicit queue binding dispatches requests to workers; it has not
met performance acceptance. The ordinary gate path operates independently.

## Build and tests

From the repository root: `cargo build --offline --locked -p uasf-gate` and
`cargo test --offline --locked -p uasf-gate`. Use a private disk-backed `TMPDIR`.
Native tests and fixed synthetic conformance responses cover binding, hashes,
JSON inspection and deletion refusal. Socket/OS-role deployment and archive or
consumer service installation require separate environment checks.
