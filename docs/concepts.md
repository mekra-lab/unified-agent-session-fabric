Korean: [Korean translation](ko/concepts.md)

# Concepts

## Source and session identity

A source is a path or record you register explicitly. A session belongs to that
source. Equal native IDs, content hashes, paths or inodes across registrations
never automatically merge sessions. A submitted
API observation can use an application conversation reference; a provider session
ID, model name, local file and account field are optional.

A capture pins source, capture key, generation, length, SHA-256 and session.
`window`, `file_generation` and `native_session` express additional observations.
A capture generation identifies pinned bytes; file lineage can survive append.
Core IDs derive from canonical tuples of source, generation, capture and byte
range, with an entity-kind prefix. A native UUID alone is not a UASF event ID.
An unchanged byte range retains its identity when the parser version string
changes. A version string alone does not prove replacement-parser compatibility.

Codex window identity comes from explicit capture registration. Integrations
must provide `Capture.window`; core does not infer it from a `session_meta`
window field or the capture's file path. See
[Codex window registration](reference.md#codex-window-registration).

## Anchors and boundaries

An anchor identifies original bytes using `source`, `capture`, `generation`,
`file_generation`, `start`, `end` and `sha256`. Offsets are zero-based `[start,end)`;
a complete JSONL record includes its terminating LF. Range deletion and evidence
require a nonempty range and lowercase 64-digit SHA-256. Window plus ordinal and
native UUID references are auxiliary; they do not replace a verified byte anchor.

A query boundary is an ordered list of exact capture IDs. It fixes the response
observations, including explicit parent captures for a fork. It is independent of file
timestamps and never selects the latest file. The policy
dependency set can include ownership candidates outside that boundary to prevent a deleted parent from
being rediscovered through another capture. Policy dependencies do not silently
add those candidates to the response history.

## Observation and interpretation

Codex projections select a window chain and retain revert exclusions, compaction
references and bounded fork inheritance. Parent ordinal and byte boundaries must
agree. A missing fork boundary yields an incomplete result; inheritance requires an
explicit boundary.
Claude projections retain an observed graph: parent chains, logical parents,
preserved segments, request references and compression roots. A compression root
does not prove that earlier model input was recovered. UUID-less records remain
physical observations; duplicate UUID candidates are not silently collapsed.
The [18 recognized Claude types](reference.md#claude-record-types-and-format-rejection)
include `permission-mode` only in its exact three-field form. Its observed `plan`
and `default` values are uninterpreted session metadata; other string values are
preserved without assigning behavior. It contributes no turn, lineage, compression
or deletion dependency. Metadata bodies still follow the normal deletion rules.
Unknown Claude types make the capture unsupported (`unsupported_claude_record`):
history retains events and stops graph computation. Codex uses
`unsupported_codex_record`. This is not generic opaque-record support.

The supported API record represents a submitted call. `complete: true` can describe
that call while total conversation coverage remains unknown. Bytes submitted as
`app_bytes` and a caller's `sdk_json` serialization are different provenance claims;
neither is proof of provider network bytes or the model's complete input.

## Verification, access and evidence

Verification means returned bytes match a pinned length/hash and the applicable
receipt binding. It does not authenticate original capture time or prove model
exposure. Successful send, reference resolution, collection or resume must not
be promoted to evidence that a model used content.

Projection status and body access are independent. Status is `complete`,
`incomplete`, `unsupported`, `unknown` or `deleted`. Body access is `available`,
`blocked` or `deleted`; a blocked body is null in responses but can remain on disk.
There is no `masked` projection status. Human-readable labels are compatibility
fields, not authorization or verification decisions.

Evidence consists of separately submitted facts (`planned`, `attempted`,
`applied`, `received`, `exposed`, `used`, `outcome_unknown`) and assessments
(`reported`, `candidate`, `observed`, `unknown`). Facts are immutable under a
source-scoped evidence ID and never automatically promoted. Display names are
user metadata separate from session identity. See
[Reference](reference.md#core-registration)
and [Deletion](deletion.md) for persistence and erasure.
