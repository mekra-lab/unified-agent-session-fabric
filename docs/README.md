Korean: [Korean translation](ko/README.md)

# UASF documentation

UASF preserves observable agent records and derives byte-anchored session views.
These documents are the current public product contract. The implementation is
experimental, with verification limited to the scopes described below.

## Read in order

1. [Workflows](workflow.md): plain-language summary, conceptual flows and tagged implementation paths.
2. [Concepts](concepts.md): identity, observations, verification and evidence.
3. [Architecture](architecture.md): component boundaries and durable state.
4. [Getting started](getting-started.md): source build and executable synthetic examples.
5. [Operations](operations.md): registration, restart, recovery and retirement.
6. [Reference](reference.md): current interfaces, JSON and response rules.
7. [Deletion](deletion.md): erasure, withholding and coordinated completion.
8. [Security and limits](security-and-limits.md): trust boundaries and enforced limits.

The independent [handoff-receipts CLI](../tools/handoff-receipts/README.md) provides
local Codex, Claude and experimental Antigravity headless dispatch evidence; its package documents its own verification scope.

## Implementation status

| Component | Implemented scope | Qualification |
|---|---|---|
| [Core](../crates/uasf-core/README.md) | Registered Codex windows, Claude observation graphs, submitted API calls; durable evidence and deletion | Offline projection of explicitly registered bytes |
| [Feeder](../crates/uasf-feeder/README.md) | Authenticated registered-source snapshots and collector validation | Explicit OS roles and provisioned key required |
| [Staging](../crates/uasf-staging/README.md) | Immutable complete-line prefixes and receipts | External archive ingestion required |
| [Intake](../crates/uasf-intake/README.md) | Source-scoped application bytes, retries, streams and tombstones | Does not create native transcripts |
| [Gate](../crates/uasf-gate/README.md) | Receipt-bound reads and deletion checks | External archive bridge executable required |
| [Deletion coordinator](../crates/uasf-del/README.md) | Durable intent, retryable steps and optional downstream completion | General deployment completion executor is not supplied |
| [JSON](../crates/uasf-json/README.md) | Shared bounded decoder and canonical value handling | See the numeric contract in Reference |
| [Queue](../crates/uasf-queue/README.md) | Opt-in local admission service | Experimental; performance acceptance has **not** been met |
| [Index probe](../crates/uasf-index-probe/README.md), [page probe](../crates/uasf-page-probe/README.md) | Synthetic feasibility tools | Excluded from operational installation |

**Do not use the page probe with real records or expose it to consumers.** Its
policy overlay can return bodies that core access policy blocks. Neither probe
defines a supported product paging or asynchronous preparation contract.

Fixed synthetic conformance data and separate-OS-user integration checks exercise
component connections and deletion. A limited integration trial also passed with
two explicitly registered, closed native sessions: one Claude and one Codex.
It covered collection, receipt-verified reads, core interpretation, deletion of
trial copies, re-ingestion refusal and teardown, with original files unchanged
and no queue binding. This is evidence for those two sessions only; it does not
establish a complete installer, general collection readiness, arbitrary
parser-version compatibility, or performance SLAs. Archive deletion on real
sessions has one unexplained, non-reproduced failure; treat archive deletion
as requiring confirmation of completion.
UASF runs independently of a central UI or Hub. UASF is derived from UASS; see
[Relation to UASS](../README.md#relation-to-uass) for scope and conformance status.
The project is licensed under Apache-2.0 (see [LICENSE](../LICENSE)); release packaging and
contribution policy remain undecided.
