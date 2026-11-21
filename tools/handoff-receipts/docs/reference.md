Korean: [Korean translation](ko/reference.md)

# Command and bundle reference

`handoff-receipts [--store DIRECTORY] COMMAND ...` uses
`$XDG_DATA_HOME/handoff-receipts` or `~/.local/share/handoff-receipts`. The explicit
store option precedes the subcommand. `--version` reports the package version. All
commands accept `--json`; machine results go to stdout. By default, attempt results use
a compact human view and other structured results use indented JSON. Operational errors
return a JSON object with an `error` field in machine mode; argument-parser errors use
stderr and exit 2. Diagnostics never include raw native stderr or unfiltered JSON event
payloads. list/inspect/dup/ which skip broken bundles, returning available results with
JSON warnings on stderr (`warning: broken_bundle_skipped`, attempt ID, error category).
They do not print corrupt contents. Explicit inspect of a skipped attempt returns an
empty array and its warning. Verify/export retain strict integrity failures.

## Commands

Antigravity is experimental, with **verified 1.5.9** native interpretation only.
See [its full contract](agy.md) for stream NDJSON input, mandatory resume DB boundary,
`agy:ID` selectors, the four allowed passthrough options, selected wire fragments,
version/hash downgrade and cancellation conflict rules.


| Command | Arguments and behavior |
|---|---|
| run FILE | One explicit dispatch; defaults to engine codex and via exec |
| inspect ID | Attempt ID or native turn/message ID; accepts ENGINE:ID and returns an array of matches |
| dup ID | Attempt or retry-group ID; duplicate and effective-attempt assessment |
| which SHA_OR_PATH | Exact instruction digest or captured path; never reads that path |
| list | All valid stored attempts |
| verify [ATTEMPT] | Check one or all bundles, checkpoints and selected-line interpretation |
| recover ATTEMPT | Explicit native reread; append a checkpoint, never resend |
| export [ATTEMPT] --format jsonl --output FILE | One JSONL record per attempt with all common E and derived receipt |
| export [ATTEMPT] --format polylogue --output NEW_DIR --polylogue-bridge BINARY | Native delivery-ledger import and readback; common E supplement |
| forget ATTEMPT | Delete even a corrupt bundle, including checkpoint history; leaves originals and exports |

`inspect`, `dup`, `which` and `list` accept `--engine codex|claude|agy` filters. An
unqualified native ID matching multiple engines is refused; use `inspect claude:ID` or
`inspect ID --engine claude`. A Claude turn ID is its confirmed user-message UUID, not a
verified execution-turn ID. Thread/session IDs alone are not inspect selectors. Verify,
recover and forget take attempt IDs.

Run options:

| Option | Meaning |
|---|---|
| --engine codex\|claude\|agy | Default codex; model names do not select an engine |
| --via exec\|daemon | Default exec; daemon is Codex-only and requires --thread |
| --resume ID, -r ID | Explicit Codex thread, Claude session or agy conversation UUID; never an implicit last session |
| --thread THREAD --socket PATH | Explicit daemon target; socket defaults to ~/.codex/app-server-control/app-server-control.sock |
| --retry-of ATTEMPT | New attempt using the same engine, instruction bytes and retry group |
| -m MODEL, --model MODEL | Native --model argument; otherwise selected engine default |
| -e EFFORT, --effort EFFORT | Codex model_reasoning_effort config; Claude --effort (low/medium/high/xhigh/max); agy --effort |
| -C DIR, --cwd DIR | Process cwd and native path scope; new Codex exec also passes -C; use the original cwd for Claude resume |
| -o PATH, --output PATH | Safe final message file; with -n or an existing directory, writes inside it |
| --child-stderr PATH | Raw target-process stderr file for Codex, Claude and agy exec runs |
| -n NAME, --name NAME | Human label and output basename (`NAME.md`) |
| --codex EXECUTABLE | Codex executable override; refused for Claude |
| --claude EXECUTABLE | Claude executable override; default claude |
| --agy EXECUTABLE | Experimental Antigravity executable override; default agy |
| --session-root DIRECTORY | Override native root; Codex: CODEX_HOME/sessions or ~/.codex/sessions; Claude: ~/.claude/projects; agy: ~/.gemini/antigravity-cli/conversations |
| --observation-timeout SECONDS | Native reread budget, 0..30 seconds; default 2; never retries dispatch |
| -- EXTRA_ARGS | Engine-specific exec passthrough; Claude uses an explicit allowlist |

`run --child-stderr PATH` writes the target child's stderr bytes unchanged to a
caller-managed file outside the receipt store. The file is created with mode 0600.
An existing file is overwritten and its permissions are set to 0600. Its parent
directory must already exist and be writable. An unavailable destination is refused
before dispatch with exit 2 and no attempt created. Omitting the option discards
child stderr through DEVNULL.

The file is not evidence and may contain account information or other sensitive
data, so the caller is responsible for managing it. Neither its contents nor its
path are recorded in receipts or the store. The option is exclusive to `run` with
`--via exec`. Auxiliary process stderr, including fingerprint probes such as
`agy --version`, continues to be discarded.

Use complete top-level and run long-option names: abbreviations such as `--sock` or
`--cod` are rejected rather than bypassing engine guards. Explicit short aliases such as
`-r` remain supported. Claude refuses `--via daemon`, `--thread`, and explicit
Codex-only `--socket`/`--codex` options before dispatch.

The wrapper owns stdin prompt, session selection, JSON stream and final output. For
Codex, passthrough cannot replace `--json`, `-o`/`--output-last-message`, `-C`/`--cd`,
enable `--ephemeral`, or choose implicit `--last`/`--all`. Other Codex options are
passed as argv; the installed version determines validity. The body is sent on stdin
with `--skip-git-repo-check`. New exec sandbox example: `-- -s read-only`; resume
example: `-- -c sandbox_mode=read-only`.

Claude uses `claude -p --output-format stream-json --verbose` with either `--session-id
UUID` or `--resume UUID`, optional `--model` and `--effort`, then allowed extra
arguments. The new UUID is generated and sealed before launch. The body is stdin; the
wrapper's `--` separator is not forwarded as a positional prompt delimiter. No shell
expansion is used for either engine.

Claude extra flags: `--safe-mode`, `--disable-slash-commands`, `--strict-mcp-config`,
`--no-chrome`, `--include-partial-messages`. Allowed one-value options: `--tools`,
`--allowedTools`/`--allowed-tools`, `--disallowedTools`/`--disallowed-tools`,
`--permission-mode`, `--permission-prompts`, `--max-budget-usd`,
`--append-system-prompt`, `--system-prompt`, `--setting-sources`. For example, `--
--tools ""` explicitly disables tools; this is not a wrapper default. Native validation
still applies. Unknown flags, positional prompt overrides, abbreviations, short-option
bundles and wrapper-owned options are refused. In particular, extra session-id, resume,
continue, fork-session, output-format, input-format, print and no-session-persistence
options cannot override the sealed request.

Claude examines only the exact `<session UUID>.jsonl` in the recorded project folder
under its root. The observed folder encoding replaces `/` in the absolute cwd with `-`.
Resume seals the existing file's length and prefix SHA-256. A changed prefix is rejected
during observation. If the file was missing before resume, dispatch can still produce
reported facts, but native attribution remains unknown with
`claude_preflight_unavailable`, including in recovery. There is no search through other
project folders. Codex resume instead refuses a missing rollout before dispatch. Native
layouts are version-scoped observations, not a universal path contract.

Codex final output prefers a safe retained native final message (`observed`). Otherwise,
a privacy-checked `item.completed` agent message from the uniquely bound, completed exec
stream can supply it (`reported`). The receipt's `output` records assessment, source
(`target_rollout` or `exec_json_stream`), digest and destination. A later sensitive or
oversized stream candidate suppresses an earlier candidate. Claude final output comes
only from privacy-checked `result.result` when the selected status is completed. Its
source is `claude_json_result`, assessment reported. Native assistant text hashes are
auxiliary and do not supply the file. `is_error=false` plus `terminal_reason=completed`
determines completion; `subtype` is not used. Error/interruption results never supply
final output.

A missing or omitted final message leaves the requested output untouched; inspect the
final-output fact before using an existing output file. Output paths inside the receipt
store are refused.

Recover accepts `--observation-timeout SECONDS` to override the saved/default reread
budget and `-o PATH` / `--output PATH` to override the saved destination without
rewriting the immutable request. Without `-o`, recovery uses the original output
destination. Recovery applies the same engine-specific output rules; a missing Claude
result cannot be reconstructed from native end_turn.

For Codex daemon dispatch, thread/read must report idle before mutation. The wrapper
records turn/start before sending, then waits for the exact returned turn ID, never for
general idle. A fresh thread may lack a rollout path/file; an empty or not-materialized
turn list allows a zero-byte preflight. The path is reread after the turn. Existing
threads with missing rollouts, other read failures, and busy or notLoaded threads are
refused before turn/start. The notLoaded message is: `thread not loaded in the daemon;
open it in the Codex app or use --via exec --resume`. No thread load or ownership
transfer is attempted. Refusals expose `facts.attempted=false`, `facts.dispatched=false`
and `rejection_reason`; human inspect/list show `not sent`. Intent alone does not mean a
request might have reached the server. A lost response can be attributed only by a
unique exact native input after the recorded boundary. Later `recover` may fill missing
facts. Model and effort are passed to turn/start; exec passthrough is unsupported here.

Queries with skipped-bundle warnings return 0. Other invalid arguments, strict integrity
failure or unavailable exports return 2. Run preserves the native process exit code when
possible; negative signal return codes are translated to shell convention. A zero native
exit returns 0 when the selected turn status is completed, either observed or reported.
Missing native evidence emits a `target_observation` warning on stderr with specific
diagnostic codes/descriptions. Observed failure or interruption overrides a stream
completion and returns 2 with native exit 0; unknown outcome also returns 2. For Claude,
a completed report conflicting with linked native error/interruption traces produces
`claude_status_conflict`, unknown status/final output and wrapper 2 after native exit 0.
Result loss stays unknown through recovery. Native traces never establish whole-turn
completion. Nonzero native exit codes remain nonzero even if completion is observed.
Missing final output emits `output_unavailable` but does not change this lifecycle-based
exit policy. Receipt facts keep process status, turn outcome and output availability
independent. Daemon non-success returns 2. Recover returns 0 for a successfully recorded
reread, including a reported/unknown result with warnings; it does not recreate a
historical exit code.

## Bundle version 1 / interpretation version 2

```text
STORE/ATTEMPT_ID/
  source.json              # engine/profile, source pointer, digest, retry link
  body.txt                 # exact UTF-8 instruction
  request.json             # argv or RPC intent, target pointer, preflight boundary
  INTERPRETATION.md         # packaged interpretation at initial capture
  manifest.json            # hashes of the four preceding files
  states/SHA256.json       # immutable {version, previous, state} checkpoints
  CURRENT                  # durable head checkpoint digest
  .lock                    # advisory capture/recovery lock, no private contents
```

Before publication, the entire initial bundle lives at `STORE/.pending-*`, including its
first checkpoint; fsync and one directory rename publish it. Hidden staging directories
are excluded from queries. They can remain after a forced initializer exit and contain
private intent; remove only when no initializer is running. Existing final-ID corrupt
directories can be removed with forget.

`state.control` is the process/stream/RPC history; only explicitly selected fields are
persisted, including safe stream final-message candidates. `state.target` holds IDs,
selected target segments, optional exact RPC status and observation issues.
`diagnostics` contains fixed safe code/description pairs and optional counts/offsets,
never raw native content or exception text. `issues` retains code strings for
compatibility. `observation_attempts` records each reread's timestamp and codes; a later
successful read clears current issues, preserving earlier attempts and historical
checkpoints. Each segment stores copied JSONL bytes as a UTF-8 `copy_text`,
retained-line original/copy offsets and hashes, omitted-line range/hash/reason
inventory, whole-span/snapshot hashes and derived summary fields. Queries rederive facts
from the retained bytes instead of trusting summary flags. New source/request files seal
`engine` and `interpretation_profile`: `codex-selected-lines-v2` or
`claude-selected-lines-v1`, or experimental `agy-selected-fields-v1`. The JSONL segment
description applies to Codex/Claude; agy uses [wire fragments](agy.md#selected-evidence-and-privacy). Missing engine metadata means Codex; old bundles and their
interpretation files are not migrated. The full rule is [engine
interpretations](../src/handoff_receipts/INTERPRETATION.md).

New Claude segments declare `association=parent-chain-v1`, with `turn_key={kind:
user_message, value: UUID}`. The selected span starts at the unique matching input after
preflight and ends at the complete snapshot boundary. Connected assistant/interruption
rows require a parent chain reaching that input; other actual inputs,
missing/out-of-span links, duplicate IDs and cycles prevent association. A conflicting
explicit promptId prevents it too. Legacy segments without the marker retain their
explicit promptId interpretation.

`connection_projections` entries have exactly `kind=derived_connection`, `fields`
containing only `uuid`/`parentUuid`, and `start`, `end`, `sha256` matching an omitted
source line. They are derived links, not original JSONL rows. Offline verification
checks their fields/provenance and rederives bindings with the retained bytes. Without
omitted originals it cannot authenticate that those values came from the claimed source
hash. Only auxiliary hashes/traces use these bindings.

Derived receipts expose `facts`, per-field `fact_levels`, `observed_facts`, a separate
`reported` summary, `identity_levels`, `output`, `observation_issues` and
`observation_attempts`. Human inspect/list displays, for example, `completed (reported)`
and the diagnostic description. Reported lifecycle does not confirm input, a missing
exact turn ID or native completion time. R2/R3 still require observed input evidence.
Claude `dup` also requires terminal reports for the distinct session/user-message pairs;
input pairs alone do not count. Offline queries preserve legacy generic issues as
historical unknown causes.

Claude additionally exposes `turn_key`, `native_traces` and `auxiliary` (native output
hash, message stop reason/timestamp, comparison to reported output). Its turn_started
and completed_at remain unknown. A native error/interruption trace conflicting with a
completed report makes status unknown while preserving the original `reported` view. See
the [fact table](concepts.md#independent-facts).

Common observation diagnostics:

| Code | Meaning |
|---|---|
| rollout_not_found / rollout_unreadable | Missing path/file or filesystem read failure |
| rollout_unsafe_path / rollout_too_large | Unsafe path or snapshot above 32 MiB |
| rollout_incomplete / rollout_invalid_json / rollout_invalid_shape | Incomplete or malformed native data |
| rollout_metadata_missing / rollout_thread_mismatch | Missing or mismatched native identity |
| preflight_prefix_changed | Previously hashed prefix no longer matches |
| ambiguous_rollout_path / ambiguous_target_match | Multiple possible source files or target segments |
| no_matching_turn | No attributable native turn after the preflight boundary |
| input_binding_omitted / target_input_missing | Safe exact input binding unavailable |
| sensitive_required_lines | Required lines were omitted by the privacy scanner |
| turn_terminal_missing / final_output_missing | Attributable terminal or safe final output unavailable |
| native_selection_invalid | Remaining unsupported selection structure; no raw exception persisted |
| claude_preflight_unavailable | Missing original resume boundary; native attribution cannot be reconstructed |
| claude_identity_mismatch | Init/result session disagrees with the sealed target |
| claude_result_unavailable | No usable unique result; native traces do not replace it |
| claude_status_conflict | Completed report conflicts with linked native error/interruption traces |

Transient absence/incompleteness and missing turn/input/terminal/output retry up to the
configured budget, at 100 ms intervals. Privacy, ambiguity, prefix changes and other
permanent errors stop immediately. A budget of zero still reads once; the budget limits
scheduling further reads, not filesystem operation duration.

All private product directories are 0700 and files 0600 at creation. Explicit exports
and final-message files are also 0600; newly created export/output directories are 0700.
Existing caller-owned parent directories are not modified. Changing permissions
externally is not cryptographic tampering detection.

Verification checks immutable core hashes, every complete checkpoint hash and head
chain, source/body binding, selected-copy hashes, line roles and ranges. Unexpected
bundle files fail verification. Partial `.writing-*` files cannot supply facts. The
omitted original bytes cannot be verified without originals; whole-range hashes remain
capture commitments. No native source reads occur.
