# Engine profiles — handoff-receipts 0.3.0

New bundles seal `engine` and `interpretation_profile` in source and request.
An absent engine in an older bundle means Codex. Reads never migrate or rewrite
old bundles. Bundle version 1 and the Codex interpretation version 2 remain intact.
`codex-selected-lines-v2` uses the rules below unchanged. Claude uses the separate
`claude-selected-lines-v1` profile described here. Query results identify the engine;
identical native IDs in different engines are distinct. An unqualified ambiguous
native ID requires an engine selector. Explicit retry groups permit one engine only.

## Experimental Antigravity selected fields v1

`agy-selected-fields-v1` is experimental. The native map is verified for version
1.5.9 and the executable SHA-256 pinned in code. Validation includes the operator's
2026-11-15 native sample and synthetic tests. A new attempt seals
only its attempt ID; returned conversation IDs are checkpointed from child stdout.
Resume seals a readable matching DB's maximum step index, boundary-row hash and
trajectory ID. Empty steps use index -1 and SHA-256 of empty bytes. Capture/recovery
only open the exact requested/returned conversation DB, using SQLite mode=ro,
one short read transaction and the existing bounded observation budget. No DB
copy, checkpoint, journal-mode change, immutable read or other-history search occurs.

The minimal 1.5.9 map (all paths are numeric protobuf field numbers):

| Cell | Path | Meaning / wire type |
|---|---|---|
| steps.step_payload | 1 / 4 | Step type / status, varint; must agree with SQL |
| steps.step_payload | 5.12 | Metadata execution_id, UTF-8 string |
| steps.metadata | 12 | Duplicate execution_id, UTF-8 string; when present must agree |
| steps.step_payload | 19.2 | USER_INPUT user_response, UTF-8 string |
| steps.step_payload | 20.1 / 20.8 | PLANNER_RESPONSE response / modified_response, UTF-8 strings |
| executor_metadata.data | 1 / 9 | Termination reason varint / execution_id UTF-8 string |

USER_INPUT=14, PLANNER_RESPONSE=15; selected status values must agree with SQL.
Termination 2=USER_CANCELED and 4=NO_TOOL_CALL. Unknown fields are skipped, never
recursively interpreted as Any. Missing/duplicate required fields, invalid wire
types, malformed cells and SQL/protobuf disagreements make the dependent native
judgment unknown. Only relevant planner and same-execution executor fields are
retained. Settings, environment, generator metadata and tool payloads are excluded.
Selected decoded strings pass privacy checks.

Each retained leaf keeps its original tag/length/value wire bytes as wire_base64,
table/PK/column, field_path, cell-relative [cell_start,cell_end), cell_length,
cell_sha256, fragment sha256 and profile. Enclosing messages are not copied.
Step/executor summaries are explicitly marked derived_projection. Offline reads
rederive those summaries from fragments and reject inconsistencies. They cannot
authenticate original source membership, omitted fields or SQL values independently
of the omitted cell. Hash commitments are not signatures.

Input confirmation requires verified execution/recovery fingerprints, consistent
conversation and trajectory IDs, the saved boundary, and the USER_INPUT at this
child's unique preserved user_input.step_index with exact instruction bytes.
Resume requires idx greater than its saved boundary. Subsequent USER_INPUT rows
have separate indexes. Planner/executor records use the selected execution_id.
Missing or ambiguous stream input identification, agy_stream_incomplete, a missing
user-input conversation ID or an invalid index makes native attribution unknown
in capture and recovery. Full native identity is (engine, conversation_id,
trajectory_id, execution_id), with turn_key kind=execution. No conversation-level
wrapper advisory lock is added.

Stdin is one UTF-8 NDJSON user message plus EOF, with input/output-format stream-json
and no -p. Exactly one valid result is required and num_turns is ignored.
SUCCESS with an absent/null finished_reason gives completed/reported, with
print_timeout gives failed/reported, and with any other reason gives unknown.
ERROR with interrupted gives interrupted/reported and with any other reason,
including absent/null, gives failed/reported. Other statuses give unknown.
An absent/null reason is stored as JSON null and an unrecognized string as unknown.
A SUCCESS response that cannot be encoded as UTF-8 is malformed, making status
and output unknown. Sensitive content exclusion omits the output while retaining
the completed report. Unknown combinations, missing,
malformed, multiple or identity-conflicting results yield unknown status/output.
Sanitized observed reports remain in control history. A requested/actual conversation
mismatch retains both IDs, leaves request input/status unknown, and returns 2.
Recovery rederives reported status and output hashes from those preserved stream
events while native rereads update observed and auxiliary evidence. A native read
failure leaves the saved report available. Auxiliary-field omission warnings and
pending executor metadata are independent. The common observation budget continues
executor rereads when planner fields are omitted.

Only the selected completed outcome may use exact result.response UTF-8 as
final output/reported hash. Planner text hashes are auxiliary without LF correction.
Native NO_TOOL_CALL never establishes success. Same-execution USER_CANCELED plus
a completed report makes status/output unknown and returns 2 with both sources
retained. Cancellation accompanying timeout/interrupted is consistent. Turn start,
completed_at and model_read_or_used are always unknown. Unknown run/recover status
returns wrapper 2; known outcomes preserve nonzero child exits, or return 2 for
failed/interrupted child exit 0. Process exit remains a separate observed fact.

Verification requires one verified before fingerprint and a verified after
fingerprint, or the latest verified recover fingerprint when after is absent.
Every recorded before, after and recover version/hash must match the pin. Any
mismatch keeps verification false through later recoveries. Unverified checks warn,
keep valid stream results reported, and make native input, trajectory/execution
keys and linked auxiliary facts unknown.
Byte matches are only unverified_interpretation observations; executor reasons keep
numeric values without enum names and do not participate in conflict rules.
Recovery invokes the saved executable's --version metadata check and records its
current executable hash and stat in agy_version_recover before the native reread.
Each fingerprint records stat.st_dev, st_ino, st_size, st_mtime_ns and st_ctime_ns.
When after is absent, all five values must exist and match between before and the
latest recover. With an after record, the existing version/hash rule applies.
The executable is resolved once to an absolute argv[0] using child cwd for paths
containing a slash and for relative/empty PATH entries. Hashing, version checks,
model execution and recovery all use that saved path. Missing
result observations remain unknown. Verified pins expand only after native validation
and a code constant update.

## Claude selected lines v1

Duplicate execution for Claude requires distinct confirmed message pairs with
terminal reports; two input records alone never establish it. The message keys
are not verified execution-turn IDs. Native completion ordering remains unknown.

The session UUID is generated and sealed before a new dispatch and passed through
`--session-id`. Explicit resume preserves its session UUID. Only the recorded cwd
project directory and exact UUID filename are inspected. No other project/session
is searched. Resume captures the preflight byte length and hash. An unavailable
preflight keeps native attribution unknown, including during later recovery.
Session mismatches in init/result invalidate target attribution and stream claims.

An eligible user after the preflight boundary must uniquely match the preserved
instruction's decoded UTF-8 bytes. Its native UUID is the `turn_key`, explicitly
`kind=user_message`; `turn_id` exposes the same key for query compatibility. It is
not claimed to be a native execution-turn ID. Queue/init/message_start/api-request
are never promoted into `turn_started`, which remains unknown. `completed_at` also
remains unknown; neither assistant timestamps nor local stream receipt times fill it.

New observations declare `association=parent-chain-v1`. Copy the matching input
user and assistant/interruption lines whose parentUuid chain reaches exactly that
user UUID within the captured append span after preflight. Another actual input
user (excluding an interruption marker), a missing/out-of-span link, duplicate ID,
or cycle breaks association. An explicitly conflicting promptId also prevents it.
Physical adjacency is never a binding. Existing observations without this marker
keep the original explicit equal-promptId interpretation without rewriting bytes.

Attachments, API payloads, environment/account metadata and tool-result users
remain omitted. When an associated chain traverses an omitted row, a
`connection_projections` entry stores only its uuid/parentUuid field values,
`kind=derived_connection`, and source line start/end/SHA-256. These are derived
connection data, not retained original bytes. No other original fields are copied.
Only links used by an associated chain are projected; actual other-input users
are never graph bridges. Queries and verify rederive associations using retained
bytes and these projections alone, validating their shape and omitted-line
provenance. As with omitted-line commitments, the original values cannot be
independently authenticated without the omitted source; hashes are not signatures.

Each retained line is unchanged original JSONL bytes; line ranges/hashes, a full
span inventory, snapshot/span hashes and copy offsets are preserved. Required
lines failing shape/privacy checks are omitted. If the input cannot be retained,
dependent native bindings are unavailable. Association supplies only auxiliary
native output hashes/traces, never a new whole-turn terminal rule.

A retained same-prompt synthetic assistant error, interruption user, or assistant
end_turn supplies only an observed auxiliary native_traces entry, with its source
line/UUID and timestamp. None establishes a whole-turn terminal status. Native
assistant text hash, stop reason and timestamp remain auxiliary observations;
a later omitted assistant prevents reuse of an earlier assistant as final.

One complete, session-bound stream result with boolean is_error=false and
terminal_reason=completed establishes completed/reported. is_error=true establishes
failed/reported, or interrupted/reported for an abort/interruption reason. Subtype
never decides success. Missing, duplicated, malformed or identity-conflicting result
observations remain unknown. A reported completion conflicting with a native error/interruption trace yields
unknown status and a conflict diagnostic, while retaining the original report. Raw stdout/stderr and account metadata are not persisted.

Only a completed selected outcome may use the privacy-checked result.result text
as final output and final_output_sha256, at reported level. Native assistant output
is auxiliary, with an explicit hash comparison against the reported output. Error
text and interrupted partial responses never become final output. Process exit 0
plus completed/reported returns wrapper 0; failed/interrupted/unknown returns 2
after native exit 0. Nonzero native exit codes are preserved. A missing native
observation emits a warning. Recovery rereads within the same budget and boundary;
it cannot reconstruct a missing stream completion from native end_turn. Output
provenance is claude_json_result/reported. model_read_or_used is always unknown.

## Codex profile (unchanged interpretation)

# E interpretation v2 — handoff-receipts 0.1.1

This product profile inherits the prototype's selected-lines-v2 interpretation.
Bundle version 1 describes packaging, independently of interpretation version 2.

The five logical E components are source pointer (`source.json`), exact instruction
(`body.txt`), pre-dispatch request (`request.json`), control observations
(checkpoint `state.control`), and target observations (`state.target`). The source
includes the instruction path, byte length, SHA-256, mtime, attempt ID, timestamp,
and explicit retry link/group. A path is a pointer, not proof of current content.

Only task_started, the exact user message, terminal events (task_complete,
task_failed, turn_aborted), and final-output decision lines are copied. Every
selected line has its original [start,end) byte range, SHA-256, role labels and
copy offsets. Other lines, including sensitive necessary lines, appear only as
range/hash/role/reason entries in `omitted`. The span and snapshot retain hashes;
the selective copy has a separate hash. Copy bytes are UTF-8 in `copy_text`.
No account/session metadata body is copied. Preflight records length/hash only;
subsequent capture checks that this original prefix still matches.
Context compaction is not a lifecycle boundary. Exact start/input binding survives
a `compacted` row, whose contents remain omitted, until the next task start or the
attributable terminal. A terminal with a different explicit turn ID is not attached.
A partial trailing native line is outside the observation. Rollouts over 32 MiB
are unsupported and yield unknown observation rather than truncated evidence.

Facts are rederived from retained bytes, not trusted summary flags. Omitting a
required line makes its dependent fact unknown. Exact user input after the
identified start establishes target input confirmation. Terminal events refer
to that exact turn, or are within its explicit start boundary. In Codex 0.163.0,
a task_complete with non-null error is a failed turn, not successful completion. A matching daemon
turn-list ID also establishes its reported terminal state independently of input
confirmation. Process exit and stream completion without an exact turn ID do not
establish native turn completion. No fact proves model exposure or semantic use.

The 0.1.1 derived view adds per-fact assessment levels. Local/native observations
take precedence over exec stream reports. A uniquely bound stream thread and one
turn start/terminal may supply `reported` lifecycle facts independently of native
observation. Conflicting IDs or multiple lifecycle candidates cannot supply a
completed report. Missing exact turn IDs and native completion timestamps remain
unknown. Safe `item.completed` agent-message text/digest is allowlisted into control
history; the last eligible message before successful stream completion may supply
reported final output. A later sensitive/oversized candidate invalidates an earlier
message candidate. Output events record assessment and source. These additions do
not change selected-lines-v2 bytes or the observed input requirements for R2/R3.
Existing bundle interpretation files remain immutable; current derived views can
expose legacy control reports without rewriting them.

Attempts, process starts, target inputs, turn starts and turn outcomes are
independent. A saved intent is not an attempted send. New bundles record
`intent_recorded`; legacy initial `attempted` events are also interpreted only as
intent. `attempted=true` means dispatch crossed a write-ahead boundary at which
the request may have reached the server: a logged daemon turn/start, or an exec
launch boundary not followed by a known launch failure. `dispatched=true` means
turn/start was about to be sent or a native process was created; this does not
prove server receipt. An exec crash between the launch boundary and process-start
record yields attempted=true, dispatched=unknown. Preflight refusal, unsent intent
and known launch failure yield attempted=false, dispatched=false. A recorded
`dispatch_rejected.reason` explains refusal. Legacy generic errors cannot reveal
a more specific reason. Capture/recover never attaches native turns to unsent
intent. Other missing observations remain unknown. An explicit retry
must preserve exact instruction bytes. Duplicate execution requires two distinct
confirmed (thread, turn) pairs. Its absence requires all grouped attempts resolved.
The effective attempt is the earliest observed completion only when all grouped
attempts are either known unsent or confirmed and terminal, all completion times exist and the first is
unique. No completed attempt in a fully resolved group gives null; uncertainty
or a tie gives unknown. This rule does not identify a semantic business effect.

The initial manifest seals source/request/body/this interpretation before dispatch.
The complete initial bundle and checkpoint are written and fsynced under a hidden
`.pending-*` staging directory, then renamed to the final ID in one operation and
the store directory is fsynced. A crash before publication leaves no visible ID.
Abandoned hidden staging directories are ignored by queries.
Immutable checkpoints are named by SHA-256 and reference the previous checkpoint.
An atomically replaced CURRENT selects the durable head. All checkpoints and the
head chain are checked. Fully written orphan checkpoints after a crash are checked
but do not supply facts. Incomplete `.writing-*` files never supply facts. Hashes
detect changes relative to stored commitments; they are not signatures and cannot
prevent an attacker replacing data together with all commitments. Omitted bytes
and full native snapshots cannot be reconstructed or independently rehashed.

inspect/list/dup/which/verify/export use only bundle files. Query scans skip corrupt
bundles with a categorized stderr warning. Duplicate assessments remain unknown
for absence/effective-attempt claims when skipped evidence could matter; positive
observed duplicates remain reportable. Verify/export stay strict. Forget can delete
a corrupt bundle without loading it, while retaining the active-writer lock guard.
Fresh idle daemon threads without a rollout use a zero-byte preflight only if
there are no prior turns (including a not-materialized turn list). Other read
errors or a missing rollout with prior turns cause pre-dispatch refusal. The
rollout path is refreshed after dispatch. notLoaded is refused without loading or
transferring a thread. `recover` explicitly
reads the recorded native location (or a uniquely matching newly created rollout)
and appends a checkpoint. It never resends or interrupts work. Concurrent active
capture/recovery is rejected by a per-attempt advisory lock. An absent exit event
stays unknown even if later native completion is recovered. Without an observed
stream thread ID, recovery excludes pre-existing paths and requires an exact body,
a matching working directory and exactly one new candidate. Ambiguity stays unknown.

Capture and recovery retry transient native-read absence/incompleteness for a
bounded interval (default 2 seconds; configurable 0..30). They record safe fixed
diagnostic code/description pairs and per-read timestamps/codes. Privacy failures,
prefix changes, ambiguity and other permanent errors stop retries. No dispatch is
retried. Successful prior observations survive a later failed reread, whose issues
describe the latest attempt. Recovery may use an explicit final-output destination
override without altering the immutable request. Ordinary offline queries neither
reread nor retroactively diagnose missing native sources.
