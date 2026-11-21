# UASF core

An offline library and JSON CLI for explicitly registered Codex windows, Claude
observation graphs and submitted API calls. It projects supplied bytes; it does
not discover sources or communicate with native runtimes.

Shared contract: [concepts](../../docs/concepts.md),
[core registration and queries](../../docs/reference.md#core-registration),
[deletion](../../docs/deletion.md), [limits](../../docs/security-and-limits.md#core-limits).
The [synthetic workflow](../../docs/getting-started.md) invokes the CLI directly.

## Library interface

`Core::open(index, persistent)` uses distinct SQLite files. `Source` and `Capture`
are exported models. `ByteProvider::read(capture,start,end)` returns a `ByteResult`
with optional data/hash, verification and reason. `LocalProvider` supports explicit
file or memory `Binding`s. It verifies the entire pinned prefix, then the requested
slice; it is a trusted adapter, not safe discovery of hostile filesystem paths.
The crate is independent of archive-engine crates.

`Core::query` returns a materialized JSON value. `Core::query_to_writer` uses a
compact projection and anonymous spools before delivery. Callers must handle
delivery errors without adding another JSON document. Library users own their
admission policy. Optional `Core::bind_deletion` ties persistent state to a trusted
coordinator domain; this survives replacement of the derived index.

## Build and tests

From the repository root, run `cargo build --offline --locked -p uasf-core` and
`cargo test --offline --locked -p uasf-core`. Use a separate `CARGO_TARGET_DIR`
and disk-backed `TMPDIR`; the lockfile and cached dependencies support offline use.

The [fixed synthetic conformance bundle](tests/golden/README.md) runs directly
in Rust. Native tests also exercise source separation, byte hashes, fork and
compaction boundaries, missing/duplicate references, scoped readiness, exact output
limits, durable evidence, explicit erasure, reversible withholding and restart.
Passing these checks is not a claim about arbitrary parser replacement, production
performance, or physical erasure of provider data and backups.
