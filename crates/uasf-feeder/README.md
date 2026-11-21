# Registered-source feeder

Transfers immutable registered-source snapshots from a source owner to a collector.
The library and CLI share an implementation independent of archive engines.
Capture requires explicit registration and does not establish model exposure.

See [configuration and wire fields](../../docs/reference.md#feeder),
[provisioning and recovery](../../docs/operations.md), [deletion](../../docs/deletion.md)
and [security and limits](../../docs/security-and-limits.md).

## Role interfaces

Producer commands are `init`, `emit`, `retry`, and `purge`; collector operations
validate inbox records, materialize accepted generations and publish them to staging.
The configuration binds numeric OS UIDs, stream, source roots and allowed files.
The signed record binds original identity, generation, predecessor, sequence, policy
revision and byte hashes. A reserved producer send and a committed collector
acceptance can be recovered independently after exit.

Collector `publish` verifies the immutable copy and calls staging while retaining
original device/inode provenance. It requires a deletion binding except for explicit
unmanaged synthetic use. Lock order is deletion domain, policy, state, staging.
Collector deletion is not producer-purge or downstream-deletion acknowledgement;
see the actual reply fields in the shared reference. Operators provision OS
users, groups, ACLs and service management separately.

## Build and tests

From the repository root: `cargo build --offline --locked -p uasf-feeder` and
`cargo test --offline --locked -p uasf-feeder`. Set a dedicated disk-backed `TMPDIR`.
Native tests and compact fixed conformance data cover authentication, ordering,
restart, process exits, source mutation, limits, safe opening and bound staging
publication. Separate-user integration coverage is limited to the synthetic
setup; production collection and general installation remain unverified.
