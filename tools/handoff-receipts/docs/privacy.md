Korean: [Korean translation](ko/privacy.md)

# Privacy

Instructions are checked before any process launch or daemon connection. The
conservative scanner rejects email-like strings, recognized token prefixes, bearer-style
credentials and common authentication/account field assignments. Source paths, stored
argv and all persisted JSON are also checked. This heuristic is not a comprehensive
secret detector; review instruction files and passthrough options before sending. Do not
pass credentials as command-line arguments.

Each engine retains its own native history under its own policy. For Codex, the product
reads bounded snapshots transiently, but never copies session/account metadata. It
copies only required safe task-start, exact-input, terminal and final-output lines.
Unneeded or sensitive lines leave only their original ranges, hashes, role labels and
omission reasons. If a necessary line is sensitive, its dependent facts remain unknown.
Even decoded JSON is checked so Unicode escaping cannot hide a recognized sensitive
pattern. The first-turn injected context is not copied. Receipt bundles retain
allowlisted stream control fields and classified errors. Since 0.1.1, text
from `item.completed` agent-message candidates is retained only after the same privacy
check, with a digest. Raw event envelopes are never saved. Sensitive candidates leave
only an omission reason. Stream lines exceeding 4 MiB are discarded through their
newline and cannot supply a final response; native JSONL lines have no separate limit
within the 32 MiB snapshot limit. Diagnostic descriptions are fixed safe text, with
counts/offsets where useful, not native exception strings or rejected content. Reported
output uses the same private file permissions as observed output.

`run --child-stderr PATH` optionally writes the target Codex, Claude or agy child's
stderr bytes unchanged, without privacy filtering, to a caller-managed file outside
the receipt store. The file is not evidence and may contain account information or
other sensitive data, so the caller is responsible for managing it. Neither the file
contents nor its path are recorded in receipts or the store.

The file is created with mode 0600. An existing file is overwritten and its permissions
are set to 0600. Its parent directory must already exist and be writable. An unavailable
destination is refused before dispatch with exit 2 and no attempt created. Omitted
stderr capture and auxiliary probes such as `agy --version` use DEVNULL. The option
is exclusive to `run --via exec`.

Claude Code 2.1.310 native records included account, organization and environment
information inside attachment and api-request-blob rows. They are excluded from
original-line retention, along with other API/context metadata. The Claude profile also
rejects recognized organization/authentication field assignments before persistence.
Only the exact input user and safely connected assistant/interruption lines are eligible
for original-byte retention. Full lines are checked both as raw text and decoded JSON; a
rejected line is omitted, not redacted and called an original. Tool-result users are not
extra instruction inputs.

A required parent chain can traverse an omitted attachment or tool-result row. Its
separately marked **derived connection projection** stores only uuid and parentUuid
field values plus the source line byte range and SHA-256. It never stores the omitted
row's type, attachment payload, account fields or environment. Other actual input users
are not projected as bridges. The retained bytes and projections support offline
association checks; the omitted originals cannot be reconstructed or authenticated from
a hash alone. These identifiers still reveal connections and remain private local data.

Claude streams retain only selected init/result fields and privacy-checked successful
result text. Subtype, raw event envelopes and account/organization metadata are not
retained. The native executable may still write sensitive data to its own history;
filtering receipts does not alter or sanitize that history.

Hashes and timestamps can still reveal equality and timing. Instruction content, paths,
thread/turn identifiers, model arguments and safe final responses are private local
data. Directories use 0700 and files 0600. Symlink store paths and Codex/Claude native capture paths
are refused. This does not protect against the same OS user, root, compromised tools,
backups, filesystem snapshots or an attacker who can replace data together with its
hashes. The native executable and optional Rust bridge are trusted local programs, not
sandboxed adversaries.

`forget` removes one published bundle, even if corrupt, and every saved checkpoint.
Hidden `.pending-*` directories left by a killed initializer are unpublished private
intent; remove such leftovers only when no initializer is running. It is not secure
erasure and does not touch native engine logs, instruction files, answer files,
child stderr files, exports or backups. Delete those independently when needed.
Forgetting evidence narrows the scope of later duplicate and mapping answers.
JSONL and Polylogue exports contain
instruction text and common E; handle them with the same care as the source store. The
adapter supplies omitted receipt concepts in `common-E.jsonl`.

No network service, account handoff, cross-account lookup or automatic resend is
provided. The CLI runs the requested native Codex, Claude or Antigravity executable, which may use
its configured network credentials. The wrapper does not read authentication stores or
print account identity. Ordinary inspect/dup/which/list/verify/export commands access
only local bundles; native rereads require capture or explicit recover.

## Experimental Antigravity in 0.3.0

The [Antigravity profile](agy.md) adds stream-NDJSON exec/new/resume and selected
SQLite protobuf fields. Its native interpretation is limited to verified 1.5.9;
version/hash mismatches warn and make native facts unknown. Input confirmation also
requires this child's unique stream user-input index. Whole-turn status and final
output come from one valid result at reported level; start, completion time and model
use remain unknown. Native cancellation can invalidate a completed report.
The operator recorded a native sample on 2026-11-15.
[Validation record](validation.md#antigravity-native-sample-and-follow-up-fixes--2026-11-15).
