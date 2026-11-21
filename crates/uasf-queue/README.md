# Finite local query queue (experimental, opt-in)

**Performance acceptance has not been met.** This is not a recommended operational
path. Shared [limits](../../docs/security-and-limits.md#other-component-limits),
[response rules](../../docs/reference.md#optional-queue) and [deletion semantics](../../docs/deletion.md)
are maintained in the product documentation. From the repository root, build/test
with `cargo build --offline --locked -p uasf-queue` and
`cargo test --offline --locked -p uasf-queue`; use a disk-backed `TMPDIR`.

An explicitly bound store bundle has one owner-operated scheduler, two execution
slots, at most 32 waiting requests and at most 16 waiting requests per source.
Ready requests use source FIFO and round-robin between sources. Unavailable
requests do not block ready requests. Admission is not an access authorization:
workers perform the existing core or receipt-gate checks at execution time.

Enabling this path requires an explicit binding. Socket, concurrent-load, restart,
and permission-boundary acceptance must be established before deployment.
Unbound stores keep their previous admission behavior. Direct library calls
remain the embedding application's responsibility.

## Configuration and lifecycle

The following is schematic configuration, not a runnable deployment example.
The external archive bridge is not supplied in this workspace. A configuration
requires a private, owner-only bundle
directory and a mode-0600 JSON file:

```json
{
  "version": 1,
  "bundle": "/private/runtime/bundle",
  "routes": [
    {"id":"core","kind":"core","binary":"/private/bin/uasf-core",
     "root":"/private/store",
     "args":["--index","/private/store/index.sqlite",
             "--persistent","/private/store/persistent.sqlite"]},
    {"id":"gate","kind":"gate","binary":"/private/bin/uasf-gate",
     "root":"/private/staging",
     "args":["--bridge","/private/bin/archive-bridge",
             "--registry","/private/registry.json",
             "--sources","/private/sources.json",
             "--staging","/private/staging","--source","archive"]}
  ]
}
```

Paths are explicit. The core root is its persistent database's parent; the gate
root is its staging directory. A root cannot be bound twice, including aliases
with the same device/inode. A binding records the bundle's canonical identity,
configuration digest, and fixed route. The service accepts no arbitrary commands.
Configuration changes require shutdown, unbind, and explicit bind again.

```sh
uasf-queue bind --config /private/queue.json
uasf-queue serve --config /private/queue.json
# After stopping the service and reaping all request guardians:
uasf-queue unbind --config /private/queue.json
```

A single lifetime lock prevents duplicate services. The internal socket is mode
0600 and checks peer UID. A cross-UID consumer
uses the existing permission-controlled gate socket; the collector forwards its
request. Service startup is explicit; clients never spawn or retry services.
An absent service fails closed for a bound store. SIGTERM cancels pending work
and waits for cleanup. Unbind refuses while a service or guardian owns a lock.

Each request has a random nonce bound to peer PID/start time. Active duplicate
nonces are rejected; identical independent queries are not deduplicated or cached.
The queue is volatile. A service restart does not replay requests.

## Time, process, and output boundaries

The eight-second monotonic deadline includes input, queueing, lock waits,
execution, and output acknowledgment. Only a read-only availability probe runs
while queued: a temporarily locked core index is unready. Missing indexes and
gate policy/configuration errors are dispatched so the worker preserves its
existing refusal or rebuild-required response.
Workers retain responsibility for incomplete projections, unknown references
and deletion decisions.

Each execution guardian owns a slot descriptor until the worker and adopted
children are reaped and delivery has completed or failed. The control pipe's EOF
cancels work when the scheduler dies. A guardian is a subreaper and also collects
children that leave the original process group. Unexpected guardian death stops
the service, which cancels and reaps its adopted children before exiting.

Request and response spools are anonymous disk files; no named pending body
survives process death. The worker's complete output is delivered before the
client acknowledges completion. Internal failure cannot expose an incomplete
JSON document. Transport failure during delivery cannot retract an already sent
prefix and must not be treated as successful JSON or followed by another JSON
error document. The existing in-flight snapshot exception remains bounded by
the deadline. Waiting requests hold no deletion lease.

The scheduler samples combined process RSS, including guardians, worker trees,
and connected clients, and cancels work above 1 GiB. This is sampled protection,
not a hard kernel memory cap; address-space limits are not RSS measurements.
Acceptance must separately measure the gate frontend, consumer clients, buffers,
and child processes, including rejected requests and brief peaks.

Admission reasons include `query_queue_full`, `query_source_queue_full`,
`query_deadline_exceeded`, `query_duplicate_request`, `query_service_unavailable`,
`query_service_stopping`, `query_worker_failed`, and `query_memory_limit`.
Core uses `status: unsupported` with `reasons`; gate uses `reason`.
Successful response bytes are the existing worker JSON bytes.

## Test scope

Pure scheduling tests cover FIFO, fairness, unready heads, duplicate admission,
expiry and hard queue counts. Pipe/process tests cover canceled parent control,
escaped-child reaping, delivery acknowledgment, and slow input/output deadlines.
Full socket cancellation, concurrent load, deletion races and cross-UID behavior
require the external integration harness. Synthetic fixtures only.

## Terminal refusals and delivery acknowledgements

The private queue IPC response header is 13 bytes: an unsigned big-endian
64-bit body length, signed big-endian 32-bit exit code (0 or 2), and a boolean
acknowledgement byte (0 or 1). Deploy the service and clients from the same
build; this experimental IPC has no mixed-version negotiation. Public JSON
response bodies remain unchanged.

A refusal without a guardian has flag 0 and exit 2. Its complete body is terminal;
the client must not send an ACK to a connection that may already be closed.
A guardian-backed response has flag 1, including worker errors: the client sends
ACK only after complete delivery, and the guardian retains its slot until ACK
and descendant reaping. ACK failure here is still a delivery failure. A complete
terminal refusal is not a delivery failure merely because the peer has closed.
Invalid or truncated frames fail without emitting an incomplete JSON body.
The absolute eight-second deadline is unchanged; no ACK grace period is added.
A peer that cannot receive within the deadline has no guaranteed delivery, and
partial transport output must never be followed by another JSON document.
