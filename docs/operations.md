Korean: [Korean translation](ko/operations.md)

# Operations

## Deployment prerequisites

Use Linux, explicit absolute paths and collector-controlled private runtime roots.
The source tree is a Cargo workspace. Operators provision users, groups and
service management separately. Keep producers, collectors and consumers in
separate roles when testing the permission boundary. A consumer should receive a
gate socket, not archive files, raw bridge access, keys or collector state.

Gate needs an external archive bridge and an already configured archive collector.
The deletion coordinator needs trusted intake/archive commands and, for integrated
core/feeder completion, a trusted completion executor. The public tree does not
supply a general deployment of these dependencies. Command synopses in Reference
describe the component interfaces; installation remains a separate operator task.

Provision feeder key material separately as a private 32-byte file shared with
both trusted roles. Use a collector-owned drop directory writable by the producer
and a collector-owned policy directory readable by the producer, with role-private
state. Temporary process group grants can avoid permanent user-group membership;
OS permissions require separate configuration. Never grant a consumer
the drop or collector privileges. Validate permissions before starting a service.

## Registration and capture

Register exact source roots and permitted relative files or native JSONL patterns.
Do not scan default personal session directories implicitly. Original roots must
not overlap runtime storage. Feeder and staging walk directories without following
symlinks and compare two bounded reads to detect observed mutation. This does not
provide an atomic snapshot of a producer that can rewrite its own files.

Staging publishes only the last complete LF prefix. It holds a partial tail while
retaining full-source length/hash in the receipt. Append is classified by comparing
the entire prior prefix; replacement, shrink or rewrite starts a new file lineage.
A tail-only append can reuse the published generation with a new observation.
Feeder additionally requires every transmitted complete line to be a valid JSON
object with unique keys. It preserves source bytes exactly.

Core rebuild manifests explicitly bind pinned captures to byte suppliers. Rebuild
registers the supplied entries and processes registrations; omitting an existing
registration does not delete it. Missing bytes remain unavailable. An unchanged
prefix can be read after append; a changed pinned prefix withholds bytes. The local
provider is a trusted file/memory adapter, not the hostile-path safe-opening boundary.

## Restart and recovery

Retain durable state alongside copies. Do not resolve a failure by deleting a
lock, journal, binding, tombstone or generation record. Lock descriptors are
released when owners exit. Missing or malformed deletion state fails closed.

| Component | Recovery action and invariant |
|---|---|
| Core | Rebuild a new derived index against the retained persistent database and pinned sources; names, evidence and deletion policy survive |
| Staging | Reopen or inspect under its lock; pending copies are hash-checked before activation, interrupted suppression is resumed |
| Feeder | Retry a durably reserved generation; materialize an accepted generation after restart; policy rollback and untracked copies are refused/scrubbed |
| Intake | Retry with the original source-scoped key and exact envelope/payload; transactions retain receipt or deletion tombstone |
| Coordinator | Run `resume`, which takes no job ID and continues every unfinished job; acknowledged steps/scopes are skipped, failed steps stay pending, and a job whose steps all complete is finalized |
| Optional queue | Explicitly restart the service; volatile requests are not replayed, clients must observe failure |

Core rebuild is not a migration or repair tool for already corrupted durable
submissions. SQLite secure deletion is not a guarantee about media, filesystem
snapshots or backups. Preserve required policy metadata when retiring derived data.

## Stop and retire

Stop new work, stop services, and wait for their owned workers and leases to finish.
Unbind an experimental queue only after the service and guardians have released
their locks. Resolve pending deletion jobs, including offline producer purge,
before claiming integrated deletion complete. Previously delivered bytes remain
outside the store's control.

Remove only runtime resources owned by the deployment, after checking identity,
active users/processes and mount boundaries. Never recursively delete an unverified
path or cross a mounted filesystem during privileged cleanup. No privileged
teardown script is provided as a public product installer. The getting-started
example instead removes only its freshly created private synthetic directory.
