Korean: [Korean translation](ko/concepts.md)

# Concepts and limits

A receipt records what this wrapper could observe about one explicit dispatch. It does
not transfer ownership of a session or certify that a model used the instruction. Each
recorded intent gets a random attempt ID even if process launch fails. `--retry-of`
links a new attempt with identical instruction bytes to an explicit retry group within
the same engine. Cross-engine retry groups are unsupported. Nothing automatically
retries a process or mutating RPC.

## Independent facts

The common fact keys are shared; the available evidence differs by engine. Absent
evidence is `unknown`, except for a known unsent intent or launch failure.

| Fact | Codex | Claude headless |
|---|---|---|
| attempted | Daemon turn/start send boundary or a possibly launched process | Possibly launched process; intent alone is false |
| dispatched | Process created or daemon turn/start about to be sent | Process created; false before dispatch, unknown in the launch-record gap |
| process_started | Child creation observed; unknown for daemon mode | Child creation observed |
| process_exit_code | Observed when the wrapper reaps its child | Same; a relay executable makes this a relay observation |
| target_input_confirmed | Exact instruction bytes in the identified turn's retained user message, observed | Unique eligible user after preflight whose decoded UTF-8 exactly matches the instruction, observed |
| turn_started | Retained task_started observed, otherwise uniquely bound exec stream start reported | Always unknown: no independent task-start event in the supported profile |
| turn_status | Exact native terminal or exact-ID daemon status observed; otherwise stream terminal reported | One session-bound stream result, reported; native traces cannot establish whole-turn status |
| completed_at | Retained successful terminal timestamp, observed | Always unknown: no native task-completion timestamp; assistant and result-receipt times do not substitute |
| final_output_sha256 | Safe native final output observed, otherwise safe completed-stream message reported | Safe result.result only for selected completed status, reported; native text hash is auxiliary |
| model_read_or_used | Always unknown | Always unknown |

Saved intent alone is not an attempt. `attempted=true` means the request may have
reached the server, not that delivery was confirmed. A durable write-ahead record must
precede the send itself, so even `dispatched=true` is not proof of receipt. Old initial
`attempted` events are interpreted as intent, using subsequent control records to decide
dispatch. Rejected runs include `rejection_reason` and display `not sent` in
inspect/list. Legacy generic errors retain an unavailable-reason message instead of
inventing a historical diagnosis.

`task_complete` is a lifecycle event, not necessarily success: Codex 0.163.0 uses it
with a non-null `error` for failure. Process exit 0 is also insufficient. A process can
finish while the target observation remains unknown, or the native turn can complete
while the wrapper's exit observation is lost.

## Assessment levels

Preserved source bytes are the instruction, selected native JSONL lines, or selected
Antigravity leaf wire fields. Hashes,
original byte ranges and omissions describe their capture provenance. Receipts and query
answers are deterministic derived interpretations of these bytes plus explicitly
recorded control observations. Wrapper intent and process creation are local
observations; they do not establish delivery. Native exact input confirmation is
stronger but still does not establish model exposure or use. No cryptographic source
authentication, UASS conformance or general native-engine compatibility is claimed.

Since 0.1.1 each fact has a `fact_levels` entry: `observed`, `reported` or `unknown`.
Observed process facts concern the wrapper's spawned child; observed turn facts concern
native records (or the existing exact-ID daemon status observation). Exec JSON stream
lifecycle claims are reported, not confirmation of target input. The summary prefers
observed facts and retains the separate `reported` and `observed_facts` views. Human
output labels the status, such as `completed (reported)`. `identity_levels`
distinguishes requested IDs from stream reports and native bindings. A stream without a
turn ID never creates one; its local receipt time does not supply native `completed_at`.

A successful process plus reported completion returns wrapper exit 0, with a warning if
native observation is unavailable. Codex observed failure/interruption wins over
reported completion. Claude uses the conflict rule below. Missing both sources of
completion returns 2 after native exit 0. This avoids treating missing evidence as a
reason to resend. Output availability is independent: a safe stream final message may
supply the requested file, with `reported` provenance, even when target confirmation
failed. That file is evidence of output only; native delivery, completion time and
semantic use stay separate facts.

## Engine identity and Claude result interpretation

The engine is selected explicitly (`codex` by default), independently of the model name.
New Claude session UUIDs are sealed in the request before launch and passed with `--session-id`;
explicit resume keeps the supplied UUID. Init/result identity mismatches invalidate
stream claims and native attribution. A confirmed native user UUID becomes
`turn_key={kind: user_message, value: UUID}` and the query's `turn_id`. This is a
message identifier, not a verified execution-turn ID. Engine qualifiers prevent equal
native IDs in different engines from colliding. Older bundles with omitted engine
metadata are interpreted as Codex and kept unchanged.

Claude completion requires exactly one valid session-bound result with `is_error=false`
and `terminal_reason=completed`. `is_error=true` means failed, or interrupted for an
abort/interruption reason. `subtype` never decides success: a result can have subtype
success and still report an API error. Missing, duplicated, malformed or
identity-conflicting results leave status unknown.

`native_traces` preserves linked synthetic error assistants, interruption users and
assistant end_turn markers as auxiliary observed facts. These are not whole-turn
terminal events. A reported completion conflicting with an error or interruption trace
yields unknown status and final output, with `claude_status_conflict`; the original
report remains available. Native exit 0 then becomes wrapper exit 2. Other nonzero
native exits are preserved.

Only a safe completed `result.result` supplies Claude's final output file and reported
hash. The last eligible native assistant text hash, stop reason and timestamp are
auxiliary, with `matches_reported_output` when comparable. An omitted later assistant
prevents reusing an earlier native hash as final. Recovery can add input confirmation or
native traces but cannot reconstruct a lost result or promote native end_turn to
whole-turn completion.

### Parent chains and derived connections

For new observations, an assistant or interruption marker must have a parentUuid chain
reaching exactly the matching input user within the selected append span after
preflight. Another actual input user (not an interruption marker), a broken or
out-of-span link, duplicate UUID or cycle makes association unavailable. An explicitly
conflicting promptId also prevents it; association requires the parent chain above.

If a chain crosses omitted rows, `connection_projections` contains only their
uuid/parentUuid values and source line ranges/hashes, marked as derived data. These
projections are not retained original rows. Inspect and verify rederive bindings from
retained bytes plus projections without opening native records. The links support only
auxiliary hashes/traces; the result rule above remains in effect. Legacy observations
without the association marker keep their explicit promptId interpretation.
[Reference](reference.md#bundle-version-1--interpretation-version-2) describes the
stored format and its integrity limits.

## Questions and retry groups

`inspect` answers R1 for attempts or engine-qualified turn/message IDs. `dup` answers R2
within an explicit same-engine retry group. Codex retains its existing rule: two
distinct input-confirmed thread/turn pairs establish duplication. Claude additionally
requires terminal reported outcomes for the distinct confirmed session/user-message
pairs; two input records alone are insufficient.

The effective attempt is the uniquely earliest successful completion only when all group
members are known unsent or confirmed and terminal. Missing times, unresolved attempts
and ties yield unknown. A resolved group without a success yields null. Claude
successful groups cannot establish that ordering because completed_at remains unknown.
This does not identify which business effect mattered. `which` answers R3 by instruction
SHA-256 or captured path, returning confirmed target mappings and separately listing
unconfirmed attempts. Unknown remains unknown outside the retained observation scope,
including forgotten attempts.

## Recovery and boundaries

An immutable intent manifest and first checkpoint are completed in a hidden `.pending-*`
directory, fsynced, then atomically renamed to the final attempt ID. A killed
initializer cannot publish a half-written bundle. Hidden crash remnants are ignored and
may be removed when no initializer is running. Query scans skip broken published bundles
and emit categorized warnings on stderr; verify/export still report integrity failures.
A skipped bundle makes negative or effective-attempt duplicate conclusions unknown. Forget accepts corrupt bundles without loading them, but still
refuses an active writer.

Observation checkpoints are immutable hash-addressed JSON, linked to their predecessor;
CURRENT is switched atomically after a checkpoint reaches disk. After a crash, an older
complete checkpoint is safe to read. A complete orphan is checked but not interpreted.
Partial temporary files are ignored. Recovery appends new observations and preserves
the historical checkpoints. Hashes are local integrity commitments, not signatures;
replacing bytes and all commitments can evade this mechanism.

Ordinary queries and verification use only stored bundles. Explicit `recover` may read
the original native rollout and recover a unique exact input match. Known unsent intent
is never matched to a native turn during recovery. It never resends, reconnects to a
daemon, or kills a process. If a wrapper is killed, its native child might still be
running. Recovery can observe a partial turn; repeat recovery after it finishes if
needed. A per-attempt lock prevents recovery concurrent with a surviving wrapper.
SIGINT/SIGTERM are forwarded only to the spawned native process group. SIGKILL cannot be
handled by the wrapper.

Rollout discovery supports the local Codex `sessions/**/rollout-*.jsonl` layout. Context
compaction does not end a turn: exact start/input binding survives a `compacted` record
until the next task start or matching terminal. The compaction body is still omitted.
Capture and recovery reread transiently missing evidence for up to two seconds by
default, without resending; permanent failures stop immediately. Specific safe
diagnostics and reread history explain what is missing. Earlier successful observations
remain in the bundle if a later reread fails; the current diagnostics describe that
later failure. A Codex exec resume requires its rollout to exist before dispatch so its
preflight boundary is known. Codex recovery without a
stream thread ID considers only paths not present before dispatch and requires a
matching working directory, exact input, and a single candidate. Ambiguous candidates
remain unknown. Files larger than 32 MiB, malformed JSON, unfamiliar event formats and
unavailable paths produce unknown observations. Instruction files are limited to 1 MiB.
Daemon turn-list pagination is unsupported.
For a fresh idle daemon thread, a missing rollout path/file and an empty or
not-materialized turn list establish a zero-byte preflight. The path is refreshed after
the first turn. A missing rollout for a thread with prior turns is refused. `notLoaded`
threads are refused with instructions to open them in the app or use exec resume; this
CLI does not load or take ownership of them.

Claude reads only the exact session file in the recorded cwd project folder, using the
observed slash-to-hyphen path encoding. Resume checks the preflight length and prefix
hash. If that file is missing before dispatch, reported execution may still succeed, but
native attribution stays unknown even after recovery; there is no safe original boundary
to reconstruct.

Implemented and tested behavior is described in [validation](validation.md). Real
delegations have reached the target of ten (see validation), but usefulness remains
unevaluated beyond two duplicate decisions. Broad engine/version support, cross-engine
retries, Claude IDE/desktop capture, the Claude Agent tool, automatic source discovery
services, signatures, graph storage, a UI and a Hub integration are not implemented by
this product.

Native verification is a single-host sample, not a portability or semantic-use
guarantee.

## Experimental Antigravity in 0.3.0

The [Antigravity profile](agy.md) adds stream-NDJSON exec/new/resume and selected
SQLite protobuf fields. Its native interpretation is limited to verified 1.5.9;
version/hash mismatches warn and make native facts unknown. Input confirmation also
requires this child's unique stream user-input index. Whole-turn status and final
output come from one valid result at reported level; start, completion time and model
use remain unknown. Native cancellation can invalidate a completed report.
The operator recorded a native sample on 2026-11-15.
[Validation record](validation.md#antigravity-native-sample-and-follow-up-fixes--2026-11-15).
