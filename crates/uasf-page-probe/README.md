# Synthetic paging probe

Product status and restrictions: [documentation](../../docs/README.md#implementation-status).
Build/test from the repository root with `cargo build --offline --locked -p uasf-page-probe`
and `cargo test --offline --locked -p uasf-page-probe`. Use a disk-backed `TMPDIR`.

This isolated experiment is separate from the core single-response query path,
whose limits remain in force. Run it only against disposable synthetic
stores. It is deliberately excluded from the service installation allowlist.

```
uasf-page-probe --synthetic-only prepare PROBE PERSISTENT REQUEST DESTINATION
uasf-page-probe --synthetic-only page PREPARED REQUEST [TEST_CLOCK]
```

`PROBE` is a disk-index-probe observation database. Offline preparation resolves
one exact history (or events/lineage projection), writes a temporary serialization,
and normalizes it into ranks, source-object references, scalar relationship
metadata, and a segment tree of record sizes. The temporary full serialization is
removed before publication. Preparation measurements include its time and space.
No completed page cache is stored. Atomic events reference
the observation table; aggregate arrays are separate structural parts.

Page requests add `page.manifest` from preparation and optional `max_records`
(default 256, maximum 1024), `max_bytes` (default 1 MiB, maximum 4 MiB), and `cursor`.
The entire response including metadata, cursor and newline must fit. A 4 MiB
atomic element does not fit a 4 MiB page. `range` supports `all` and half-open
`positions` with `collection`, `start`, and `end`. Range count uses prepared
lengths; range maximum uses logarithmic segment-tree lookups, including rejection
of an oversized element after the requested first page.

A fresh traversal has a random ID exposed alongside domain, manifest, projection,
normalized request digest, and epochs. Reassembly must partition on this entire
identity before deduplicating paths. HMAC-SHA256 authenticates cursors, normal
retries are allowed, and absolute expiry is 15 minutes without renewal. Optional
`TEST_CLOCK` exists only for synthetic tests. Production time/boot validation is
not implemented. A separate random key file is created with mode 0600.

Every request checks the attached persistent store's durable change generation.
A change through a second index invalidates the prepared projection, including
new null-cursor requests. Continuous append can prevent completion indefinitely;
expiry is not a promise of availability. The probe has no deletion completion API.

## Known experimental gaps

The paging golden experiment found a blocking correctness failure: the events
projection can expose synthetic bodies that the core marks blocked because of
missing/conflicting ancestry or reversible policy. The inherited resolver does
not reproduce the core access overlay. This prototype fails acceptance and must
not be connected to native data, consumers, or services. Successful timing samples
leave that verdict unchanged.

- Supported disk resolver shapes inherit the index probe's restrictions. Targeted
  lineage, anchor/turn ranges, API projections, and policy-bearing stores are not
  supported. They must remain explicit acceptance failures.
- A durable generation check is not the common authorization/deletion/pin protocol.
  All-entrypoint deletion serialization, policy-specific error precedence,
  publication races, timed delivery cancellation, and FD reclamation remain absent.
- The database and signing key must be trusted synthetic files. There is no
  authorization service or key rotation service.
- SQLite counters cover data statements; row counts count returned/joined rows,
  not all internal B-tree visits. Initialization/attachment statements are not
  instrumented. This is insufficient to certify the complete work-budget contract.
- Temporary disk use is observed; a reservation ledger/hard cap is not implemented.
- Error envelopes expose a reduced page field set in this probe. They are not a
  conforming implementation of every draft wire field.

Tests use synthetic bytes, shared safe JSON decoding, full projection reassembly,
replay/expiry/tampering, two-index invalidation, repeated append intents, and
independent oversized-tail/count-summary checks. Tests run directly as an
unprivileged user. Set `TMPDIR` to a disposable disk-backed directory before tests.
