Korean: [Korean translation](ko/security-and-limits.md)

# Security and limits

## Trust boundaries

The collector, configured executables, source registrations and runtime state are
trusted. A principal supplied as an intake CLI argument is not authentication of a
network caller. A source owner can alter their own source and a signing owner can
sign misleading content. Verification establishes byte and receipt agreement,
not capture-time truth or model exposure. The synthetic examples use only generated data.

Feeder/staging descriptor-relative opening rejects symlinks and non-regular files.
Feeder also checks inbox ownership, modes, hard links, signature, normalized paths,
sequence and predecessor. Gate checks trusted directory ownership/modes, socket
mode and receipt binding; use a private `TMPDIR` for external bridge processes.
The core local provider uses explicit trusted file paths; it does not replace
feeder/staging safe opening. Private SQLite files and persistent policy must not
be edited by consumers.

Gate accepts socket modes 0600 or 0660. The socket parent belongs to the gate UID
and must not be group-writable; group delivery uses a suitably configured setgid
directory. Source, staging and deletion admission checks precede disclosure.
Gate's read snapshot is not an indefinite delivery lease. A previously delivered
response cannot be recalled.

## Core limits

MiB and GiB are binary units. These are enforced bounds, **not latency, RSS or
throughput guarantees**. Query scope includes the response set and its policy
dependency closure; splitting dependencies across captures does not evade limits.

| Bound | Applied scope | Refusal |
|---|---|---|
| 32 MiB | One local provider pinned prefix | Bytes unavailable/unsupported verification |
| 10,000 records | Physical records needed by the query, including invalid/unfinished records; an empty capture counts one, unavailable data uses a conservative length bound | `query_record_limit` |
| 20,000 edges and traversal steps | Stored/reserved edges and dependency traversal | `query_edge_limit` |
| 128 parent hops | Capture lineage including fork parents | `query_depth_limit` |
| 64 MiB | Stored JSON materialization budget | `query_materialization_limit` |
| 10,000 candidate/policy visits | Scoped ownership and policy lookup | `query_policy_limit` |
| 256 MiB | Serialized writer JSON response, excluding CLI trailing LF; also each anonymous spool | `query_output_limit` or `query_spool_limit` |
| Two admitted queries | Unbound core CLI, per persistent-store parent directory | `query_capacity_exceeded` |
| 1.5 GiB address space | Each admitted unbound core CLI query process | Kernel allocation failure is not a successful refusal |
| 1 MiB request | Unbound core query stdin path | `query_request_limit` |

The exact serialization-length check after projection counts only emitted fields,
including every repeated anchor. It stops on the first addition exceeding 256 MiB;
exactly 256 MiB is allowed. It does not rerun the query. The final spool independently
checks its hard cap and equality with the completed count before delivery. This
check happens after constructing the projection and is not cheap admission or a
bound on projection memory. Linux `O_TMPFILE` support is required in the index
directory. Direct library callers own their admission policy; address-space limits
are not RSS caps or host-wide limits. On the default unbound core path, inline
`--json` does not use the stdin size guard. The opt-in queue client separately
checks both stdin and inline input against its 1 MiB request limit.

Sources list their complete domain; sessions list the whole requested source or
store; evidence lists the requested source/session. Lists may exceed the budget.
No operation silently truncates into a successful partial answer. Missing scoped
readiness yields `index_requires_rebuild` or `query_scope_unavailable`, with
`incomplete` and empty data. See [response rules](reference.md#responses-and-errors).

## Other component limits

| Component | Limit and behavior |
|---|---|
| Staging | Whole source read at most 32 MiB; excess is unsupported, not a truncated capture |
| Feeder | Source and entire inbox record at most 32 MiB; header at most 16 KiB. Header overhead means a 32 MiB source need not fit a record |
| Shared JSON | Depth at most 128; nonfinite floats, overflow, invalid UTF-8/Unicode and trailing input rejected |
| Gate I/O | 1 MiB request, 65 MiB response; ordinary `serve` uses 5-second socket I/O, `query` uses 300-second client I/O; bridge execution has a 60-second deadline |
| Gate address space | `serve` and `check-local` set a 768 MiB address-space limit before ordinary or scheduled dispatch; bridge children inherit it per process. The `query` client returns before this limit is set |
| Ordinary gate admission | Two slots per staging root. Only ordinary `serve` holds a slot for its server lifetime; ordinary `check-local` holds one through its response |
| Scheduled gate admission | The frontend forwards each request to the optional queue instead of holding a server-lifetime execution slot; waiting requests hold no execution slot or deletion lock. Queue admission/deadlines apply, with worker checks at execution |
| Coordinator | 1 MiB request and bounded external output; external timeout defaults to 90 seconds, configurable from 1 ms to 3,600,000 ms |
| Optional queue | Two running, 32 waiting, 16 waiting per source; eight-second absolute input-to-delivery deadline; sampled combined RSS cancellation above 1 GiB |

Queue sampling is not a hard kernel cap. Queue performance acceptance has not
been met. Native chains, duplicate references and large repeated anchor arrays can
exhaust query budgets below a source-byte ceiling. A workload that fit in a timing
trial is not a promised supported maximum.

## Failure and experimental restrictions

Deletion inspection has three outcomes: match, nonmatch, unavailable. An applicable
text/session rule plus undecodable data withholds capture/submission; parse failure
never proves nonmatch. Staging reports `deletion_inspection_unavailable`, intake
fails closed with `deletion_state_unavailable`, and coordinator target planning
refuses inspection failure. This includes escaped strings alongside overflowing
numbers or deeply nested fields. Ordinary JSON marker keys stay ordinary objects.

The page probe can expose bodies blocked by core policy. It and the index probe
are synthetic-only tools excluded from operational installation. Do not expose
them to consumers or use them with real records. Current core limits remain in
force; the probes do not define an authorized
paging service.
