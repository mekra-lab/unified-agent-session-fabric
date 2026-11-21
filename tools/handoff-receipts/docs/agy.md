Korean: [Korean translation](ko/agy.md)

# Experimental Antigravity engine

Version 0.3.0 adds `--engine agy`. This profile is **experimental**. Its native field
map is verified for Antigravity CLI **1.5.9** with the executable SHA-256 pinned in
code. The operator exercised this adapter on 2026-11-15 with
`gemini-3.8-flash-low`, recording eight launch attempts comprising seven model
processes and one refusal before dispatch. The sample covers new conversations,
resume, Unicode input, interruption, timeout and recovery.
[Native sample and follow-up fixes](validation.md#antigravity-native-sample-and-follow-up-fixes--2026-11-15).

```sh
handoff-receipts run instruction.txt --engine agy --agy /path/to/agy -C "$PWD" -o answer.md
handoff-receipts run follow-up.txt --engine agy --resume CONVERSATION_UUID -C "$PWD"
handoff-receipts inspect agy:EXECUTION_UUID --json
handoff-receipts list --engine agy --json
handoff-receipts dup agy:ATTEMPT_ID --json
handoff-receipts which agy:/absolute/path/to/instruction.txt --json
```

Install and authenticate the native CLI separately. `--agy` selects its executable.
The wrapper resolves it once to an absolute path before dispatch. A name containing
a slash uses `-C` as its base. Other names use PATH, with relative and empty PATH
entries based on `-C`. Hashing, version checks, model execution and recovery all
use that saved absolute path.
Common `-m`, `-C`, `-e` (native `--effort`), output, explicit retry and observation
budget options apply. Daemon mode, `--thread`, and explicit options for other engines
are refused before dispatch. Retry groups use a single engine.

Only `--mode VALUE`, `--settings VALUE`, `--print-timeout VALUE`, and the zero-value
flag `--disable-slash-commands` are accepted after the wrapper's `--`. One-value
options also accept `--option=VALUE`, passed as one token. Values beginning with
`-` are refused in either form. Everything else, including `-p`, `--print`,
`--input-format`, `--output-format`, `--conversation`, and positional prompts, is
refused without launching a child. Tool settings are the caller's choice.

## Input, identity and recovery

One attempt sends one UTF-8 instruction to one model process, using
`--input-format stream-json --output-format stream-json`, without `-p`. Stdin is one
NDJSON record, followed by EOF:

```json
{"event":"user","message":{"content":"the exact instruction"}}
```

JSON decoding restores the original UTF-8 bytes, including whitespace and Unicode.
Non-UTF-8 instructions are refused. `num_turns` does not decide how many results are
acceptable: exactly one complete, valid result is required.

A new conversation has only an attempt ID sealed before dispatch. The conversation
ID is checkpointed as soon as it is received from this child's stdout. Init, step and
result IDs must agree. No ID means no native DB lookup, including during recovery.
Lookup is limited to `CONVERSATION_UUID.db` under the selected native root.
The default root is
`~/.gemini/antigravity-cli/conversations`; `--session-root DIRECTORY` overrides it.

Resume requires that exact DB before sending, matching `trajectory_meta.cascade_id`,
and a readable boundary: maximum `steps.idx`, the boundary record hash, and trajectory
ID. An empty steps table has boundary index -1 and the hash of empty bytes. A missing
or unreadable DB is refused. The saved boundary is checked on later reads. If the
child returns another ID, both requested and actual IDs are recorded, input/status
for the request stay unknown, and the wrapper returns 2 without resending.

Input attribution requires exactly one distinct `user_input.step_index` preserved
from this child's complete stdout observations. It selects the USER_INPUT row with
that exact `idx`, whose `user_response` UTF-8 bytes must equal the instruction.
Resume also requires the index to be greater than its saved boundary. Later inputs
in the conversation have their own indexes. Planner and executor observations use
the selected input's execution ID. Repeated stream updates for the same index are
allowed. Incomplete streams, missing user-input conversation IDs and invalid or
ambiguous indexes leave native attribution unknown in capture and recovery.

`recover ATTEMPT_ID` records the saved executable's current version and hash in an
`agy_version_recover` event, including an `agy --version` metadata check. It rereads
the same DB within the saved/overridden observation budget. A missing result remains
unknown even if native responses or termination reasons become readable. Unknown
agy status returns wrapper exit 2 on run and recover. Other nonzero child exits are
preserved for known outcomes. A child exit 0 with failed/interrupted outcome returns 2.
The child's actual exit code remains a separate fact.
Recovery rederives reported status and output hash from the preserved stream
control events. Native rereads update observed and auxiliary evidence. The saved
stream events supply the reported outcome even if a later native read fails.

## Facts and version limits

| Fact | Antigravity profile |
|---|---|
| attempted / dispatched / process | Existing local observed process rules |
| target_input_confirmed | Observed only with verified version/hash, IDs, boundary, unique stream index and exact native input |
| turn_started / completed_at | Always unknown |
| turn_status | Derived from the status and finished_reason table below, reported |
| final_output_sha256 | Exact UTF-8 `result.response` for selected completed outcome; reported |
| model_read_or_used | Always unknown |

| status | finished_reason | Outcome |
|---|---|---|
| SUCCESS | Absent or null | completed |
| SUCCESS | print_timeout | failed |
| SUCCESS | Any other value | unknown |
| ERROR | interrupted | interrupted |
| ERROR | Any other value, including absent or null | failed |
| Any other status | Any value | unknown |

Stream controls store an absent or null reason as JSON null and an unrecognized
reason string as `unknown`. A SUCCESS response that is not a string or cannot be
encoded as UTF-8 is malformed, making status and output unknown. ERROR results
follow the table even when response is absent, null or another non-string value.
A sensitive response leaves the completed report available with its output omitted.

The native turn key is `{kind: execution, value: execution_id}`. Full identity is
`(engine, conversation_id, trajectory_id, execution_id)`. These are distinct IDs.
Native planner response hashes are auxiliary; no trailing-LF correction is applied.
Native termination reasons NO_TOOL_CALL and USER_CANCELED are observed auxiliary
facts, never proof of success. A completed report conflicting with USER_CANCELED for
the same execution makes status and final output unknown, retains both sources, and
returns 2. Timeout/interrupted reports with USER_CANCELED are consistent.

Immediately before and after the model process, the wrapper records `agy --version`
and the executable SHA-256. `version_verified=true` requires a verified before record
and either a verified after record or, when after is absent, a verified latest recover
record. Every before, after and recover version and hash must match the pin. Any mismatch
keeps verification false, including one followed by a matching recover fingerprint.
Each fingerprint also records `stat.st_dev`, `stat.st_ino`, `stat.st_size`,
`stat.st_mtime_ns` and `stat.st_ctime_ns`. When after is absent, all five values
must be present and identical in before and the latest recover record. Missing
or differing values leave verification false. With an after record, the existing
version/hash rule applies.
Unverified checks emit a warning, keep valid stream outcomes reported, and make
native input, execution/trajectory keys and linked auxiliary facts unknown. Selected
byte matches appear under `unverified_interpretation`. Termination reasons retain
numeric values and cannot trigger the cancellation conflict rule. Supporting a new
version requires native validation and a code constant update.

The stream-json timeout sample returned SUCCESS, `finished_reason=print_timeout`,
an empty response and child exit 0. The corrected rule expects failed/reported and
wrapper exit 2 with either verified or unverified version evidence. A native
re-run after the correction recorded failed/reported with wrapper exit 2.

## Selected evidence and privacy

After child exit, SQLite is opened through `file:...?mode=ro` in one short read
transaction. Only selected columns from `trajectory_meta`, `steps` and
`executor_metadata` are read into memory, then the connection is closed. Busy or
incomplete state is reread within the common budget. There is no DB copy, checkpoint,
journal-mode change or `immutable` connection.
Auxiliary-field omission warnings and pending executor metadata are recorded
independently. Pending executor rereads continue within that budget even when
planner fields were omitted.

`agy-selected-fields-v1` retains original **leaf field wire bytes**: type/status,
execution IDs, exact input, needed planner responses and executor termination reason.
Every fragment includes table, primary key, column, numeric field path, cell-relative
`[cell_start,cell_end)` range, cell length/hash, fragment hash, profile and
`wire_base64`. Nested enclosing messages are excluded. Derived step/executor
projections are marked `kind=derived_projection`. Duplicate required fields, wrong
wire types and SQL/protobuf disagreement leave dependent native observations unknown.

Full BLOBs, generator metadata, settings/environment, tool payloads and protobuf Any
are excluded. Selected decoded strings receive the existing privacy checks. Raw
stdout/stderr, init account data and free-form error text are not saved. Filtering
receipts does not change the native CLI's own history.

Offline inspect/verify/export check selected wire fragments and derived facts for
consistency. Without the original cell, they cannot independently authenticate its
source membership or the omitted enclosing fields. Cell hashes are commitments,
not signatures. See [privacy](privacy.md) and [validation](validation.md).
