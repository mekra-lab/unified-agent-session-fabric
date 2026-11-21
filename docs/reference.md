Korean: [Korean translation](ko/reference.md)

# Interface reference

This is the current product contract for the implemented interfaces. Examples in
[Getting started](getting-started.md) run without an archive or socket. The
synopses below describe arguments; uppercase placeholders are not executable
installation instructions. Each CLI accepts one operation per invocation.

## JSON and equality

All components decode raw JSON through `uasf-json`. Ordinary object keys including
`$serde_json::private::Number` and `$serde_json::private::RawValue` remain keys.
Integers remain exact, including large integers. Decimal/exponent numbers are
finite binary64 floats: `1e0`, `1.00` and `1.0` are equal floats, distinct from integer
`1` and boolean `true`. Floating `-0.0` differs from `0.0`; integer `-0` equals `0`.
Canonical immutable-registration comparison ignores object-key order and preserves
array order, types, null versus absence and signed floating zero. Original spelling
is retained by original bytes, not reconstructed canonical JSON.

The standard decoder uses the last duplicate object key. The unique-key decoder
used for feeder metadata and complete object lines rejects duplicates, including
escaped aliases. Invalid UTF-8, Unicode, trailing input, nonfinite values, float
overflow and depth above 128 are refused. Consumers must not use a JSON decoder
that turns private marker keys into numbers. Gate additionally has its own archive
value-comparison compatibility rules: equal integer/float values compare equal,
booleans compare numerically as zero/one, and floating signed zeros compare equal.
Structural lengths, ordinals and offsets still require unsigned integers; booleans
and floats are not accepted in those fields. These rules do not change core
immutable-registration equality.

## Core registration

```text
uasf-core --index INDEX --persistent PERSISTENT rebuild|query|name|evidence|delete
```

Send JSON on stdin, or use `--json JSON` (`--json -` selects stdin). Index and
persistent SQLite files must be different. Rebuild input is an object with
`sources` and `captures` arrays. Each capture entry contains a `capture` object
and a local `fixture_path` binding. Paths are adapter configuration, not identity.
Existing registrations remain when omitted. Immutable identity/digest conflicts
are rejected; rebuild completion is not a history-completeness assertion.

| Object | Required fields | Defaults and constraints |
|---|---|---|
| Source | `id`, `engine` | Engine `codex`, `claude` or `api`; `observation` defaults to `fixture` |
| Capture | `source`, `capture`, `generation`, `length`, `sha256`, `session` | Length is an unsigned integer; SHA-256 is lowercase hex; `window` and `file_generation` default null; `native_session` defaults true |
| Anchor | `source`, `capture`, `generation`, `start`, `end`, `sha256` | `file_generation` is optional/null; byte range `[start,end)`; evidence/deletion require nonempty range |
| Name request | `source`, `session`, `name` | Trimmed string; empty clears the user name |
| Evidence | `evidence_id`, `source`, `session`, `fact`, `assessment`, `method`, `observed_at`, `anchors` | Nonempty anchors; `body`, `target_turn`, `target_call` default null |

Unknown fields in typed registrations, name and evidence requests are rejected.
Evidence facts and assessments are enumerated in [Concepts](concepts.md#verification-access-and-evidence).
Same source-scoped evidence ID cannot be changed by re-registration. Erased evidence
cannot be resurrected by replay. Names are independent metadata; a label may fall
back to an observed title or engine plus session suffix. Labels do not establish
identity; host information is not inferred.

The local provider supports files and in-memory byte bindings through
`ByteProvider::read(capture,start,end)`. It returns optional bytes, digest,
verification and reason. It verifies the complete pinned prefix before slicing;
append can preserve the prefix while a changed prefix withholds bytes. It does
not authenticate the capture time. The parser checks returned length/hash again.

Supported submitted API lines have `type: "api-record"`,
`schema: "uasf.synthetic-api/1"`, `conversation`, `call_id`, `request`, `response`
and `complete`. They are complete UTF-8 JSONL records. Submitted scope is not
proof of total conversation completeness.

### Codex window registration

For Codex, the parser uses the explicitly registered `Capture.window` as the
window identity and checks `session_meta.payload.session_id` against
`Capture.session`. A missing registered window yields `window_id_required`;
this reason does not require a `window_id` field in `session_meta`. Core does not
derive the window from that field, `payload.id`, or a file path.

An integration must supply the window before rebuild and use the registered
window when selecting a history `leaf`. A validated original rollout filename
can supply it for known filename formats; a renamed staging copy cannot stand in
for the original name. Filename extraction is an adapter rule for an observed
format, not a permanent native-engine contract. Session IDs and window IDs must
not be conflated. Registration does not bypass ordinal, history-mode, parent or
fork-boundary validation.

## Core queries

Requests have `op` and the fields below. The response envelope is
`{schema, op, status, reasons, data}` with schema `uasf-core.query/1`.
`reasons` is sorted and deduplicated. `data` is operation-specific.

| Operation | Fields beyond `op` | Returned data |
|---|---|---|
| `sources` | None | `sources` |
| `sessions` | Optional `source` | `sessions` with `capture_ids`, anchors and labels |
| `history` | `source`, `session`, `boundary`, optional `leaf` | Engine-specific selected projection |
| `events` | `source`, `session`, `boundary` | Physical `events`, including invalid/UUID-less records |
| `turn` | History fields and full `turn_id` | Selected/excluded turn and original anchors |
| `lineage` | History fields, optional `target`, `kind` | Reference edges and resolution |
| `evidence` | `source`, `session` | `facts`, `automatic_promotion: false`, `absent_facts` |
| `status` | History fields | Projection and access diagnostics without event bodies |

`boundary` is ordered, nonempty and contains same-source capture IDs, obtained
from `sessions`; explicit parent-session captures may be included. Codex requires
an explicit leaf window. Missing boundaries are incomplete, never latest-file
selection. Queries reject extra operation fields. Comparison ignores object-key
order and preserves array order and the
distinction between null and absence.

Physical events follow boundary order, then byte position. Codex history follows
the selected parent-to-child window chain, preserving active/excluded distinctions
and first-occurrence turn ordering. Claude returns physical observations plus the
selected parent chain; logical parents, preserved segments and request references
remain graph edges outside that chain. Reference resolution does not imply exposure.
Lists use deterministic object ordering; clients must not resort history by timestamp
or native UUID. Byte-range identity and original anchors remain primary.

### Claude record types and format rejection

The 18 recognized Claude types are `queue-operation`, `user`, `assistant`,
`system`, `attachment`, `file-history-snapshot`, `atis-latch`, `api-request-shape`,
`api-request-blob`, `api-request`, `last-prompt`, `cost-state`, `mode`,
`custom-title`, `ai-title`, `progress`, `summary`, and `permission-mode`.
Recognized types must also pass their field-shape checks.

`permission-mode` is supported only for an object with **exactly** `type`,
`permissionMode`, and `sessionId`: `permissionMode` must be a string and
`sessionId` must equal the registered capture session. Missing or additional
keys, including UUID/parent/reference fields, a nonstring mode or a mismatched
session are rejected with `unsupported_claude_record`. Observed producer values
include `plan` and `default`; the value set is not closed. Values are preserved
as uninterpreted session metadata, not authorization instructions. These records
create no turns, lineage edges, compression roots or deletion dependencies.
Their bytes, anchors and bodies remain subject to normal deletion policy.

An unknown Claude `type` marks the capture `unsupported` with reason
`unsupported_claude_record`. Claude history returns the physical events but stops
graph computation; an empty graph in this response does not prove absent
relationships. Codex rejects an unknown type with `unsupported_codex_record`.
No generic opaque-record interpretation is implemented.

The default parser preserves `m1.2` output versions for existing inputs. A Claude
capture containing `permission-mode` (including rejected shapes) uses
`claude/m1.3` on its derived objects; its session uses that revision as well.
A rebuild containing such a capture reports `parser_version: "m1.3"`; other
captures retain their own versions. Explicit library `rebuild_version` overrides
remain caller labels, not compatibility proof. Installing a new binary does not
reinterpret stored records: rebuild the derived index from verified bytes to
apply this extension. Existing unsupported captures stay unsupported until then.
Persistent registrations, evidence, names and deletion records must be retained;
byte anchor identities do not change merely because the parser changes.

### Status and reasons

Status is `complete`, `incomplete`, `unsupported`, `unknown` or `deleted`; aggregate
severity is deleted, unsupported, incomplete, unknown, complete. `body_access` is
independent: `available`, `blocked`, `deleted`. Withheld bodies are null and have
`body_reasons`. There is no `masked` status. Common reasons include:

| Reason | Meaning |
|---|---|
| `query_fields_or_operation`, `query_field_type` | Unsupported fields/operation or wrong field type |
| `source_and_session_required` | A scoped operation lacks its identity |
| `explicit_capture_boundary_required` | No explicit boundary was supplied |
| `duplicate_boundary_capture` | Boundary repeats a capture |
| `capture_boundary_source_or_id_mismatch` | Boundary has an unknown or foreign-source capture |
| `session_capture_missing_at_boundary` | Boundary lacks this session's capture |
| `session_not_indexed` | Session is unknown in the index |
| `native_authority_not_compared`, `model_input_unknown` | Observed graph is not authoritative model-input reconstruction |
| `submitted_scope_only_total_conversation_unknown` | Only submitted API observations are known |
| `index_requires_rebuild`, `query_scope_unavailable` | Relevant derived readiness/policy metadata cannot support the query |

This list covers common reasons; clients must also handle other returned reasons.
Scope budgets and their exact refusal codes are in [Security and limits](security-and-limits.md#core-limits).

## Responses and errors

Core handled semantic queries exit 0 even with `incomplete`, `unsupported`,
`unknown` or `deleted`; inspect status and reasons, not only the exit code. Scope
limit refusals have empty `data`. Invalid input or store failure exits 2 with the
minimal envelope `{"status":"unsupported","reasons":["invalid_input_or_unavailable_store"]}`.
Admission and output errors also use minimal envelopes with their specific reason,
such as `query_capacity_exceeded`, `query_admission_unavailable`,
`query_request_limit`, `query_spool_limit` or `query_output_limit`.

An optional bound deletion barrier returns the minimal
`{"status":"incomplete","reasons":["pending_deletion"]}` as a handled result (exit 0).
It does not fabricate the full query schema or empty data field. Normal rebuild
returns `complete`, `captures_indexed`, `parser_version` and a `meaning` explicitly
separating rebuild success from history completeness. Name returns `complete`;
evidence also returns its ID. Core deletion returns `deleted`, requested count,
`body_policy: "explicit_erasure_and_reversible_dependency_blocking"` and
`scope: "core_index_and_durable_evidence_only"`.

The writer path finishes projection, exact serialization-length validation and
spooling before destination delivery. Internal failures expose no partial JSON.
A transport failure may leave an already delivered prefix; exit 2 does not append
a second JSON error to that prefix. Consumers must reject incomplete JSON or
failed transport. This rule is not a promise to retract bytes already sent.

## Intake

```text
uasf-intake --database DATABASE --registry REGISTRY --principal PRINCIPAL
  [--deletion-root ROOT] [--collector-uid UID] [--coordinator-lock-fd FD]
  [--unmanaged-test-mode]
```

The registry maps **source IDs to allowed principals**. One JSON request on stdin
produces one JSON response. Principal is trusted local configuration. Binding is
required except for explicit unbound synthetic tests.

| Operation | Request fields | Response |
|---|---|---|
| `submit` (default) | `source`, `session`, `payload_base64`; optional `key`, `representation`, `kind`, `stream`, `sequence` | `accepted`, `replayed`, receipt; or `conflict` with original receipt ID |
| `get` | `source`, `receipt_id` | `ok`, receipt and exact `payload_base64`; or `not_found`/`deleted` |
| `stream_state` | `source`, `session`, `stream` | `ok`, chunks, terminal, contiguous and stream state |
| `erase` | `receipt_ids` | Trusted local erasure with deleted receipt/key tombstones |

Representation defaults to `app_bytes`; `sdk_json` is also allowed. Kind defaults
to `record`. Chunks require a stream and nonnegative integer sequence; terminal
`complete`/`interrupted` entries use empty payload and null sequence. A complete
stream needs contiguous chunks from zero and a complete terminal. This says nothing
about completeness of the entire conversation. Unknown request fields are refused.

A source-scoped key fixed before the first send makes exact retries idempotent.
Without a key, equal bytes are separate observations and deduplication is unknown.
Receipts bind source/session, representation/kind/stream/sequence, payload length
and SHA-256, observation basis, coverage limits and durable storage reference.
Receipt ID and receive time vary. Accepted/ok and successful erase exit 0; all
other statuses, including deleted get/submit, exit 2. Parse errors are
`invalid_request`; unavailable admission/storage does not acknowledge success.

## Staging

```text
uasf-staging --root ROOT --registry REGISTRY [--deletion-root ROOT]
  [--collector-uid UID] [--unmanaged-test-mode] capture SOURCE RELATIVE
uasf-staging [same options] scan SOURCE
uasf-staging [same options] delete SOURCE RELATIVE
uasf-staging [same options] inspect
```

Registry version 1 has `sources` mapping each ID to `kind` (`codex`/`claude`),
absolute `root` and optional exact relative `files`. Otherwise patterns are
`sessions/**/rollout-*.jsonl` and `projects/**/<uuid>.jsonl`, respectively.
Capture returns `published` with a receipt or `unchanged` with an observation;
empty/partial-only input is `not_captured`. Scan returns an array. Published,
unchanged and local delete success exit 0; scan requires all entries to succeed.
Inspect returns recovered state with exit 0. Failures exit 2.

Version-1 receipts bind source/kind, captured-input path and device/inode facts, generation
and file lineage, staging-relative path, full-source and published-prefix lengths/
hashes, withheld tail and status. Publication is pending before rename/fsync, then
active. A tombstone retains only status, generation and staging-relative path.
Bound direct delete returns `coordinator_required`. An unavailable deletion
inspection returns `not_captured/deletion_inspection_unavailable`, creates no new
copy/receipt and does not manufacture a tombstone.

## Feeder

```text
uasf-feeder --config CONFIG --key KEY --state STATE --policy POLICY --inbox INBOX
  --role producer|collector COMMAND
```

Commands are `init`, `emit SOURCE RELATIVE`, `retry GENERATION`, `accept RECORD_NAME`,
`materialize GENERATION`, `delete SOURCE RELATIVE`, `purge`, and collector
`publish GENERATION --staging ROOT [--unmanaged-synthetic]`. Configuration version 1
binds a 32-hex stream ID, producer/collector UIDs, sources with absolute roots and
kind, and either explicit `files` or `pattern: "native-jsonl"`.

The inbox record is canonical ASCII JSON metadata, LF, then exact prefix bytes.
HMAC-SHA256 binds the unsigned header, LF and payload. Metadata includes configuration
digest, stream/source/path/kind, generation/predecessor/sequence, policy revision,
original device/inode/times and lengths/hashes/tail. A header alone is not proof
of successful downstream publication. Collector publication validates the copy
again and preserves original provenance while calling staging.

One JSON object is returned. Success exits 0; rejected or unavailable capture
exits 2. Collector deletion reports `deleted`, `policy_revision`,
`producer_purge: "required"`, `downstream_deletion: "required"`. Producer purge
reports `purged` and still requires downstream deletion. See [Deletion](deletion.md).

## Gate

```text
uasf-gate serve --socket SOCKET [--socket-mode MODE] --bridge BRIDGE
  --registry REGISTRY --sources SOURCES --staging STAGING --source SOURCE
uasf-gate query --socket SOCKET
uasf-gate check-local --bridge BRIDGE --registry REGISTRY --sources SOURCES
  --staging STAGING --source SOURCE
```

Query/check-local take a JSON object. Operations `read`, `resolve`, `anchor` require
`generation` (at most 100 characters), with optional `reference`; resolve supplies
`expected_sha256`, anchor supplies integer `ordinal`. Socket requests are JSON
terminated by write EOF, not a persistent multiplexed protocol. Bridge is a trusted
external executable providing list, capture verification, read, anchor, excisions
and reference resolution; it is not included in this workspace.

The gate verifies receipt binding, complete hashes and current deletion policy
before releasing content. Statuses are `verified`, `unverified`, `mismatch`,
`deleted`, `out_of_scope`, `unsupported`. Verified output includes generation,
reference, length, SHA-256 and `original_provenance`; read carries bytes as `hex`.
Despite its name, gate `original_provenance` contains only the staging receipt fields
`source`, `generation`, `file_generation`, `relation`, `length` and `sha256`. It does
not return original device/inode metadata or the feeder's full
`staging_link.original_provenance`.
It explicitly sets `capture_time_verified: false` and
`deletion_check: "read_snapshot_not_delivery_lease"`. Denials omit content/reference.
Query/check-local exit 0 only for verified, otherwise 2. A pending deletion can
appear as deleted/unverified to this disclosure interface; it is not a purge
completion acknowledgement. The external adapter's own raw/debug facilities are
not consumer access interfaces.

## Deletion coordinator

```text
uasf-del --config CONFIG
```

Configuration includes absolute `root`, `staging`, `intake`, source `registry`,
`intake_command`, `archive_command` and optional `command_timeout_ms`. Commands are
arrays with an absolute executable, never shell text. Archive command arguments
must include the exact `{scope}` placeholder. The external archive adapter must
report completed, non-dry-run excision, not merely queued acceptance.

| JSON `op` | Input/result |
|---|---|
| `init` | Create/bind durable state; `ok` |
| `state` | Version-1 state object, no synthetic status added |
| `request` | `selector`, `input_map`, optional `request_id`; fixed pending job |
| `resume` | No additional fields; complete or pending jobs |
| `link_receipt` | `receipt_id`, `source`, `relative`; JSON null acknowledgement |
| `put_cache` | `key`, `hex`; ok or disabled after deletion |
| `read_cache` | `key`; ok, not found, pending or deleted disclosure result |

Selectors cover a registered file, native session, text or explicit receipt.
See the [coordinator crate](../crates/uasf-del/README.md#selector-shapes) for exact
shapes. `input_map` maps absolute staged paths to verified archive input-ID lists.
Request/resume pending status exits 2; complete exits 0. Invalid operations/errors
return unsupported with a reason and exit 2. Other successful operations exit 0,
including JSON null from link_receipt and cache disclosure statuses; check the
response as well as the exit code.

Optional integration has `core: {index,persistent}` and `completion_command`.
Binding is initialized on a fresh journal, retained in core persistent state and
not silently upgraded on an old journal. The completion request is
`{"op":"complete_deletion","job":...}` with the frozen job. Only exact
`{"status":"complete"}` and exit 0 acknowledge completion. The required executor
is trusted deployment integration and is not supplied as a general product tool.

## Optional queue

Binding is explicit; unbound stores use ordinary admission. Core and gate workers
preserve successful response bytes and recheck access at execution time. Queue
refusal reasons include `query_queue_full`, `query_source_queue_full`,
`query_deadline_exceeded`, `query_duplicate_request`, `query_service_unavailable`,
`query_service_stopping`, `query_worker_failed`, `query_memory_limit`.
Core uses unsupported/reasons, gate unsupported/reason. Waiting requests hold no
deletion lease. Service death does not replay requests or silently fall back.
Use the same build for service and clients; experimental IPC is not version-negotiated.
See [queue configuration and lifecycle](../crates/uasf-queue/README.md#configuration-and-lifecycle).
Performance acceptance remains unmet; this is not a recommended operational path.
