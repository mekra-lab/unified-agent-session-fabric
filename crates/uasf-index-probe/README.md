# Disk index feasibility probe

Product status and restrictions: [documentation](../../docs/README.md#implementation-status).
Build/test from the repository root with `cargo build --offline --locked -p uasf-index-probe`
and `cargo test --offline --locked -p uasf-index-probe`. Use a disk-backed `TMPDIR`.

This disposable, synthetic-only experiment loads
parsed observations from a disposable core database, normalizes references and
membership into SQLite, and assembles complete history JSON using disk tables,
indexed joins, ordering and a file spool. It does not cache complete responses.
Use `--synthetic-only load INDEX PERSISTENT PROBE`, then
`--synthetic-only query PROBE REQUEST_JSON_FILE`. The destination must be new.

There is no scheduler, queue, ingestion, deletion API, authorization boundary or
production activation. Stores containing tombstones or deleted observations are
rejected. Codex inputs must have verified provenance. The experiment is excluded
from operational installation and isolated from the production query path.

Supported experiments are complete Codex window chains (including bounded forks
and reverts), and single-capture Claude graphs (including duplicate references
and cycles). Requests require explicit capture boundaries. Cross-capture Claude
resolution, malformed captures, duplicate Codex windows/active turn identities,
missing parents, cyclic Codex window chains and policy-bearing stores are outside this probe's scope. Scope
exclusions must be reported separately from successful byte comparisons.

Observation rows retain JSON order and numeric representations through the shared
JSON decoder. Reference candidates are restricted to the selected capture;
resolution, visited sets and active-path assembly use request-local SQLite
tables, so the active path is never held as an in-memory event array. One stored turn can still contain a large
membership string; this experiment measures that remaining allocation. SQLite
uses file temporary storage and a 32 MiB page cache, with mmap disabled. These
settings are
not a hard aggregate memory guarantee.

Serialization completes in an exclusive scratch file before delivery. Errors
before delivery leave stdout empty and remove the scratch file. Delivery failures
can leave partial transport bytes, as with ordinary file copying; there is no
success suffix after failure. The database remains in one read transaction until
serialization finishes. Fixture directories must be private and disposable.
The CLI reports operation duration and process peak RSS on stderr; load cost,
cold/warm query cost and spool/temp usage must be measured separately.
