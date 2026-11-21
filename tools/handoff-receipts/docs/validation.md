Korean: [Korean translation](ko/validation.md)

# Validation record — 2026-11-01

## Current status

This summary describes **0.2.0**; the 0.3.0 experimental Antigravity engine is recorded in later sections and in [agy.md](agy.md). Historical measurements follow unchanged;
their original scope and wording describe those releases. **I** means implemented,
**S** means synthetic-tested, **N** means native-verified only within the stated
sample, and **N-copy** means native-source verification by rereading a preserved
receipt copy. These are not general compatibility or semantic-use claims. See
[workflow tags](workflow.md#implemented-workflow) for step-level scope.

| Capability | Implemented | Synthetic-tested | Native evidence and limits |
|---|---|---|---|
| Two engines and offline queries | I: Codex default, Claude headless exec | S: 81 tests on Python 3.14.4 / 3.10.22, including mixed stores and native-ID collisions | N: [0.2.0 campaign and direct supplements](#version-020--two-engine-headless-receipts), one host |
| CLI guards and compatibility | I: owned argument guards; top-level/run abbreviation refusal; immutable legacy bundles | S: unsupported/owned/abbreviated flags, legacy Codex fixture and engine-specific retry groups | The --sock/--cod regression is synthetic evidence, not a native dispatch test |
| Codex exec, resume and daemon | I: existing fact and exit rules retained | S: existing regression cases remain | N: Codex 0.166.0 new/resume and direct supplement; historical Codex 0.163.0 daemon fresh-thread/notLoaded evidence |
| Claude new/resume, input and result | I: presealed session UUID, prefix boundary, user-message key, reported result | S: normal, multi-assistant, error, interruption, result loss and identity mismatch | N: Claude Code 2.1.310; 14-case relay sample plus neutral/delegation cases and one direct new execution |
| Claude parent connections and privacy | I: conditional parentUuid chains and derived projections | S: attachment bridges, other-user barriers, broken/cyclic/out-of-span chains, omitted sensitive lines and native-free rederivation | N: independent native/ledger comparison, differences=[]; selected fields/ranges/hashes checked without copying private native rows |
| Integrity, publication, permissions and forget | I: atomic initial publication and checkpoint chain | S: crash-before-rename, corruption, locks, 0700/0600, deletion | Native samples do not establish crash atomicity; synthetic tests are the evidence |
| Codex compaction and stream fallback | I: preserve turn binding; observed-first facts/output | S: large compaction, mismatched IDs, safe/sensitive/oversized stream candidates | N-copy: Codex 0.163.0 copied recovery; Codex stream-only output fallback remains synthetic-tested |
| Delayed observation and recovery | I: bounded rereads, no resend | S: 250 ms delayed writer and output overrides | N-copy: historical Codex recovery; N: Claude wrapper-kill/loss recovery confirms input but cannot reconstruct a lost result |
| Installation and export | I: installable Python package; JSONL and optional Polylogue | S: JSONL and missing-bridge cases | Historical uv install and Polylogue readback; 0.2.0 was exercised from source, not globally reinstalled |
| Usefulness and model use | I: model use always unknown | Tests cannot prove semantic use | Historical 12 Codex delegations and two duplicate decisions; the 0.2.0 campaign is functional evidence, not a new utility assessment |

Current native evidence is a single-host sample of Claude Code **2.1.310** and
Codex CLI **0.166.0**, with earlier Codex **0.163.0** evidence retained separately.
The 19-run campaign used a relay executable: product process facts concern that
relay child. Two later direct executions narrow this limitation for normal new
sessions only. This documentation update is limited to source and stored-evidence
review. No broad version/host compatibility is established.

## Installation and synthetic checks

From this package directory, with all environment paths pointing to fresh external
data directories:

```sh
UV_TOOL_DIR="$INSTALL/tools" UV_TOOL_BIN_DIR="$INSTALL/bin" \
  UV_CACHE_DIR="$INSTALL/cache" uv tool install . --python /usr/bin/python3
"$INSTALL/bin/handoff-receipts" --help
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

Installation exited 0: `Installed 1 package`, `handoff-receipts==0.1.0`,
`Installed 1 executable: handoff-receipts`. Help exited 0 and listed
run/inspect/dup/which/recover/forget/list/verify/export. A later reinstall validated
the finalized package. Synthetic checks cover success, both failure-terminal
formats, signals, lost exec and daemon responses, explicit retry/resume, missing
observations, sensitive-line omission, deterministic queries, byte tampering,
permissions, deletion, native-free queries, CLI formats and missing Rust bridge.
The initial suite passed 29 tests (exit 0). All fixtures are synthetic; no real daemon was contacted.

## Dedicated native smoke

All successful native model calls used `gpt-6-luna`, low effort and read-only
sandbox, in a new dedicated smoke directory. The failure case deliberately used
an invalid model. Only a thread created by this smoke was resumed. The CLI was
invoked from its isolated installation. No existing user thread or real daemon
was used. `--ignore-rules --ignore-user-config` bounded native smoke context.

Equivalent commands, with each instruction containing only its short marker or
an instruction to wait using `sleep 30` before a marker:

```sh
handoff-receipts --store "$SMOKE/receipts" run "$SMOKE/new.txt" \
  -m gpt-6-luna -e low -C "$SMOKE" -o "$SMOKE/new-answer.md" --json \
  -- -s read-only --ignore-rules --ignore-user-config
handoff-receipts --store "$SMOKE/receipts" run "$SMOKE/resume.txt" \
  -r "$SMOKE_THREAD" -m gpt-6-luna -e low -C "$SMOKE" --json \
  -- -c sandbox_mode=read-only --ignore-rules --ignore-user-config
handoff-receipts --store "$SMOKE/receipts" run "$SMOKE/fail.txt" \
  -m prd1-intentionally-invalid-model -e low -C "$SMOKE" --json \
  -- -s read-only --ignore-rules --ignore-user-config
handoff-receipts --store "$SMOKE/receipts" run "$SMOKE/interrupt.txt" \
  -m gpt-6-luna -e low -C "$SMOKE" --json \
  -- -s read-only --ignore-rules --ignore-user-config
```

The signal harness waited for the stream's turn-start event, then signalled only
the dedicated wrapper/native group. SIGINT was forwarded by the wrapper. In a
separate run, SIGKILL terminated the wrapper and its own native child group; then
`recover ATTEMPT --json` ran without dispatch. One additional new execution checked
a trailing-newline instruction. Six total attempts were observed.

| Scenario | Process / CLI result | Final offline inspect facts |
|---|---|---|
| New exec | 0 | input=true, turn_started=true, turn_status=completed |
| Resume smoke thread | 0 | same thread, different exact turn; input=true, completed |
| Invalid model | 1 | input=true, turn_started=true, turn_status=failed |
| SIGINT | Native exit 1 | input=true, turn_started=true, turn_status=interrupted |
| SIGKILL + recover | Harness observed -9; recover 0 | input=true, turn_started=true; process exit and turn status unknown |
| New exec, trailing newline | 0 | exact input=true, turn_status=completed |

Each inspect result included the instruction digest and concrete thread/turn IDs.
The invalid-model observation exposed a version-specific parser requirement:
`task_complete` with a non-null `error` means failure. A regression test covers it;
the preserved native lines were reinterpreted without rerunning that failed call.
SIGKILL had no terminal record; unknown was the expected result, not inferred
completion or failure. `model_read_or_used` remained unknown in every scenario.

Native Polylogue export/readback of one complete smoke receipt also exited 0:
`exported=1`, `native_readback=true`, with `common-E.jsonl` accompanying the archive.
It used an existing Rust bridge built against the fixed contract revision; the
Python caller does not attest the executable's source. Missing bridge failure is
covered synthetically. The supplied Rust source was retained from that adapter.

## Remaining scope

There is no known functional blocker in the tested scope. The real daemon product
path remains for a separate operator to run; this implementation used only the
fake Unix WebSocket daemon. General engine-version compatibility, native bridge
binary distribution and ten real delegation tasks for usefulness assessment remain
unverified or future work. Hashes provide local integrity, not authenticity, and
privacy checks are conservative heuristics rather than comprehensive secret DLP.

## Revision 1 — atomic publication and daemon preflight

The revised suite passes **40 tests on Python 3.14.4 and Python 3.10.22**. Python
3.10 support is retained. UTC `Z` timestamps are normalized to `+00:00` before
`datetime.fromisoformat`, including duplicate ordering and adapter timestamps.

Commands from this package directory (exit 0 for each):

```sh
PYTHONPATH=src python3 -m unittest discover -s tests -v
PYTHONPATH=src uv run --no-project --python 3.10 python -m unittest discover -s tests -v
uv tool install . --python /usr/bin/python3 --force --reinstall
handoff-receipts --help
handoff-receipts run --help
```

The Python 3.10 interpreter was installed with `uv python install 3.10`; test
interpreter/cache directories and captured logs were kept outside the source
checkout. Test output for both interpreters ended with `Ran 40 tests` and `OK`.
Reinstallation reported `Installed 1 executable: handoff-receipts`. Run help shows
the default socket `~/.codex/app-server-control/app-server-control.sock`.

New regression evidence:

- Publication is one directory rename after the manifest and initial checkpoint
  are complete and durable. A subprocess forcibly exiting immediately before
  rename leaves only a hidden staging directory; list remains successful and empty.
  A regular initialization error cleans up staging without publishing an ID.
- Missing/partial and tampered bundles are skipped by list/inspect/dup/which,
  with categorized stderr warnings. Good receipts remain available. Duplicate
  absence/effective-attempt assessments are conservative when evidence was skipped.
  Verify still rejects corruption. Forget deletes corrupt bundles, while an active
  writer remains protected by its lock.
- The fake Unix WebSocket daemon exercises a first idle thread with no rollout
  file, both with and without an advertised path. A not-materialized turn list
  permits zero-byte preflight; exactly one turn/start is sent, and the subsequently
  materialized path, exact input and completed turn are observed. A missing rollout
  on a thread with prior turns is still refused.
- notLoaded sends no turn/start or thread/resume. inspect/list show `not sent`,
  `attempted=false`, `dispatched=false`, and the exact actionable notLoaded message.
  Recovery of rejected intent performs no native reread. Known process-launch
  failure also reports no dispatch. The default socket is tested without connecting.
- Two operator-supplied historical refusal bundles were inspected read-only.
  Both contain no turn/start request, and now derive attempted=false/dispatched=false
  despite their legacy initial attempted event. Their generic ValueError records
  cannot distinguish the historical causes, so the specific reason stays unavailable.
  All file hashes before and after inspection matched for both bundles.

This revision made **no actual Codex exec or daemon calls**. Native daemon behavior
remains for operator verification; the earlier native smoke table is historical
and is not presented as a new daemon measurement. No Git operation was used.

## Operator verification after revision 1 (2026-11-01, Codex 0.163.0)

- Test suite re-run by the operator: 40 tests OK; `uv tool install` reinstall exit 0.
- Native daemon, newly created dedicated thread (not yet materialized): dispatched,
  target input confirmed, turn started, `completed` with a completion timestamp.
- Native daemon, `notLoaded` dedicated thread: refused before `turn/start`;
  the receipt shows `attempted=false`, `dispatched=false`.

## Dogfooding status

As of 2026-11-01, 7 real delegations between a director agent and Codex were sent
through the CLI (smoke and test dispatches excluded).

- 6 recorded the exact instruction hash, thread and turn, confirmed target input
  and a completion time.
- 1 resumed dispatch to a thread with a large native record ended with the target
  observation unavailable: summary facts stayed `unknown`, the wrapper exited 2 and
  no final-message file was written. The receipt's control events still recorded
  stream-reported turn start, stream-reported turn completion and process exit 0.
  The operator used those events to decide **not** to resend (a resend would have
  duplicated the completed turn) and recovered the reply from the native record.
  This is the first real duplicate decision informed by a receipt.

Historical v0.1 gaps exposed by that case (addressed in 0.1.1 below):

- `inspect` does not surface stream-reported start/completion as reported-level facts.
- The cause of a failed target observation is not recorded.
- The final-message file is not written when target observation fails.

Update after 0.1.1 (operator, 2026-11-01): real delegations reached 9.

- The 0.1.1 fix request itself (8th) hit the same failure on another long thread;
  the operator again used stream-reported completion in the control events to
  avoid a resend. This is the second real duplicate decision informed by a receipt.
- Cause, identified for 0.1.1: a compaction inside the turn reset the parser's
  link to the in-progress turn.
- With 0.1.1, `recover` on both real failed attempts restored observed turn IDs,
  confirmed input, completion times and the final-message files.
- The 9th delegation (a review sent to a long thread through 0.1.1) was observed
  normally.

Update (operator, 2026-11-01, later): real delegations reached 12, meeting the
target of at least 10. All 12 are now observed as completed, including the two
recovered attempts. Utility remains unevaluated beyond the two duplicate decisions
above, both of which arose from the tool's own 0.1.0 defect.

## Version 0.1.1 — compaction, reported facts and delayed observations

The revised suite passes **51 tests on Python 3.14.4 and Python 3.10.22**.
Both runs exited 0 (`Ran 51 tests`, `OK`; 8.225 s and 12.757 s respectively).
Commands from this package directory, with uv interpreter/cache locations outside
the checkout:

```sh
PYTHONPATH=src python3 -m unittest discover -s tests -v
PYTHONPATH=src uv run --no-project --python 3.10 python -m unittest discover -s tests -v
uv tool install . --python /usr/bin/python3 --force --reinstall
handoff-receipts --version
handoff-receipts --help
handoff-receipts recover --help
handoff-receipts --store "$RECOVERY_COPY" recover "$ATTEMPT" --output "$RECOVERED_ANSWER" --json
handoff-receipts --store "$RECOVERY_COPY" verify --json
```

Reinstallation exited 0: `handoff-receipts==0.1.1`,
`Installed 1 executable: handoff-receipts`. Version reports `0.1.1`; help and
copied-bundle verification exit 0. There were no new actual Codex exec or daemon
calls. The native evidence below is a read-only investigation of an operator's
existing Codex 0.163.0 run, not a new dispatch or a compatibility claim.

### Confirmed cause of the large-rollout failure

Only event types, offsets, lengths and timestamps were exposed during diagnosis;
native bodies and account identifiers were not printed or copied wholesale.
The failing turn spans these original one-based native line numbers:

| Line | Record | Original byte range |
|---|---|---|
| 531 | task_started | [3291316,3291814) |
| 537 | Exact input message | [3312644,3313954) |
| 570 | compacted | [3514585,4666805) |
| 608 | task_complete | [4871361,4873343) |

The start and completion carry the same exact turn ID. The old `rollout.segments`
cleared its active start/input binding on `compacted`. The stream control events
contained no exact turn ID, so the terminal-only fallback could not reattach it.
The unchanged old parser found zero candidates even against the fully flushed
record; excluding only the compaction row yielded exactly one matching candidate.
The corrected parser preserves the lifecycle binding across compaction and still
omits the compaction body. A different explicit terminal turn ID is rejected.

Other candidate explanations were checked:

- The 3,289,468-byte preflight prefix still matches its saved hash.
- The longest line is 1,152,220 bytes, within the 32 MiB snapshot limit. Native
  lines have no separate size limit; the 4 MiB cap belongs to the exec JSON stream.
- None of the four required selected lines fails the privacy scanner.
- Later turns start after line 608; no competing start intervenes in this turn.
- Control recorded stream completion, process exit 0, then an unsuccessful
  recovery about 11 seconds later. The repeatable parser failure explains both;
  a transient initial flush delay alone cannot explain the retained failure.
  The exact original read snapshot was not retained, so initial flush timing
  cannot be reconstructed independently.

### Recovery with original evidence preserved

All original bundle files were hashed, then copied into a separate store.
Recovery ran only on that copy, with an explicit output override outside both
stores. The original's hashes were compared again afterward and all matched.
The native source was only read. The copied bundle passes installed-CLI verify.

| View | Turn status | Exact input | Final output |
|---|---|---|---|
| New offline interpretation, before reread | completed (reported) | unknown | unavailable in old stream capture |
| Copy after recover | completed (observed) | true | 1,635 bytes, observed |

Recovery retains four safe lines and 74 omissions; current issues are empty.
The native completion timestamp is recovered. Existing immutable checkpoints and
the original interpretation/request bytes are preserved. Historical stream text
cannot be recovered from an old bundle that never saved it; in this case native
observation supplies the answer instead.

### New synthetic regression coverage

- Large sensitive compaction within a turn preserves exact input/completion
  binding, while its contents never enter the bundle; mismatched terminal IDs
  cannot attach across compaction.
- Missing/malformed native data has specific safe diagnostics shown by inspect.
  Stream-only completion is reported, never observed input or native completion
  time. R2/R3 continue to require observed evidence; contradictory observed status
  wins over reported status.
- A successful process plus reported completion returns CLI 0 with warnings.
  A safe final stream message writes a private output file with reported provenance.
  Native final output wins when available. Sensitive and oversized later stream
  candidates cannot cause an earlier candidate to be written as final.
- A fake exec exits before a detached synthetic writer flushes its rollout 250 ms
  later. Multiple post-exit rereads reach observed completion within the configured
  budget; the history records the initial missing-file reason and later success.
- Recovery upgrades reported facts after native evidence appears and honors an
  output override without modifying the immutable request or original answer.

No known blocker remains in this tested scope. Late writes beyond the configured
budget require explicit recovery. General native version compatibility and the
ten-delegation usefulness assessment remain outside this revision's validation.

## Version 0.2.0 — two-engine headless receipts

### Synthetic scope

The operator's final rerun reports **81 tests OK on Python 3.14.4 and Python
3.10.22**, both exit 0. This includes the existing 51 Codex regressions, mixed-engine
stores, ID collisions, unchanged legacy bundles, same-engine retry guards, Claude
normal/multiple-assistant/synthetic-error/interruption/result-loss fixtures,
sensitive omissions, parent-chain projections and offline inspect/verify without
native files. The last added case rejects abbreviated run options such as --sock
and --cod. Fixtures use invented values, not copied native account metadata.

```sh
PYTHONPATH=src python3 -m unittest discover -s tests -q
PYTHONPATH=src uv run --no-project --python 3.10 python -m unittest discover -s tests -q
```

The preceding implementation run recorded 80 tests passing on both interpreters;
the 81-test result includes the operator's abbreviation guard and test addition.
This documentation pass checked source and stored evidence; it is not a new
runtime-suite or installation measurement. Existing globally installed tooling
was left unchanged.

### Native sample and independent comparison

The initial campaign ran **19 native executions** through one deterministic
controller and an instrumented relay, with dedicated cwd and receipt store.
Claude used claude-haiku-5-5 except for two deliberately invalid model names;
Codex used gpt-6-astra low with read-only sandbox. All other Claude cases disabled
tools; the two tool cases allowed only Read of a harmless local sample file.

| Cases | Count | Observed result |
|---|---:|---|
| Claude new/resume, two pairs | 4 | Exact whitespace, Unicode, long input and identical repeated body retained; same session file and sealed resume boundary; completed/reported |
| Claude Read with multiple assistant records | 2 | One Read call and two native assistants each; native auxiliary and result output hashes match |
| Claude invalid model | 2 | Failed/reported, native/wrapper exit 1; subtype success did not imply success |
| Claude SIGINT at init / first text delta | 2 | Interrupted/reported; native exit 0, wrapper exit 2; final output unknown |
| Wrapper SIGKILL at message start / first text delta, then recover | 2 | Wrapper -9; relay drains surviving engine; recover confirms input and auxiliary output, status/result and process exit remain unknown in receipt |
| Relay stdout closed at init / assistant, then recover | 2 | Real EOF to wrapper; independent relay drains engine; wrapper exit 2 and result stays unknown despite native end_turn |
| Codex new/resume regression | 2 | Exact input, turn, completion and final output observed; wrapper exit 0 |
| Same-instruction neutral-controller comparison | 2 | One run per engine; same fact keys and output hash, engine-specific evidence levels |
| Codex-to-Claude delegation | 1 | Completed/reported, input observed, wrapper exit 0 |
| Total | 19 | All planned interventions triggered; no automatic resends |

The relay forwarded native argv and input, kept raw streams in memory, and saved
only allowlisted independent observations. **Product process_started and
process_exit_code in this campaign describe the relay child.** The relay also
observed the engine process independently, but that ledger is not a substitute for
a product direct-process observation. Signal/EOF cases therefore validate these
relay-mediated scenarios, not every direct-binary interruption path.

The independent checker imports no product code. Native JSONL plus the execution
ledger independently supply run count, presealed session ID, input SHA, user
UUID/turn ID and byte ranges, result interpretation and output SHA. Derived
projection fields and their source ranges/hashes are compared against native
records. Codex file paths are receipt-provided pointers checked against stream
IDs; path discovery itself is not independently reproduced. Claude native JSONL
has no whole-task result: the independent relay result is diagnostic when the
wrapper never received it. Missing wrapper results correctly remain unknown.

Actual comparison summary:

```json
{"runs":19,"native_launches":19,"differences":[]}
```

All 19 bundles passed offline verify. No native account/organization values were
found in the campaign artifacts. One initial controller postprocessing error left
the first wrapper exit unrecorded; it remains unknown, while native exit 0 and the
saved receipt/output were preserved. A second controller restart failed before
launch. Both harness errors were fixed without redispatching that attempt.
The first native upper bound was reconstructed from the next resume's preflight;
this is a measurement limitation, not a new completion claim.

### Direct execution supplements

The operator then ran **two additional executions without the relay**: a
Claude-directed Codex dispatch and a direct Claude binary dispatch, using the same
instruction. Stored requests identify the native executables, and both receipts
show process exit 0, exact input observed and matching final-output SHA-256.
Codex completion/output are observed; Claude completion/output are reported.
The operator also checked which and verify. This supplements the relay campaign
for direct normal launches and both delegation directions; it does not establish
direct signal/recovery coverage for all engines.

### Model-neutral contract and measured differences

Model neutrality means shared commands and common fact keys, with engine-specific
assessment levels, including unknown. The two neutral-controller runs returned
the same output hash. Neither equal evidence capability nor equal answer quality
is claimed. In particular, the values of turn_started and completed_at differ,
not merely their labels.

| Fact | Codex value / level | Claude value / level |
|---|---|---|
| attempted | true / observed | true / observed |
| dispatched | true / observed | true / observed |
| process_started | true / observed | true / observed |
| process_exit_code | 0 / observed | 0 / observed |
| target_input_confirmed | true / observed | true / observed |
| turn_started | true / observed | unknown / unknown |
| turn_status | completed / observed | completed / reported |
| completed_at | Native completion timestamp / observed | unknown / unknown |
| final_output_sha256 | Same digest / observed | Same digest / reported |
| model_read_or_used | unknown / unknown | unknown / unknown |

Claude's identity is a session plus a user-message UUID (`kind=user_message`),
not a verified task-turn ID. Its linked native traces/output hash remain auxiliary;
a lost stream result cannot be restored by recover. Cross-engine retry groups,
Claude IDE/desktop capture and the Claude Agent tool remain unsupported. This
single-host functional sample does not establish broad usefulness or compatibility.

## Version 0.3.0 — experimental Antigravity, synthetic validation

The Antigravity native field map is pinned to **verified 1.5.9** and the executable
hash documented in the [engine guide](agy.md). This implementation run made **zero
actual agy executions and zero model calls**, and did not open the native credential
or history directory. The operator subsequently ran the native sample recorded below.
Synthetic version tests replace the production pin in test memory with the fake
producer's hash; there is no runtime verification bypass.

The existing 81 tests were retained and 40 tests added, for **121 total**. The new
standalone module reports `Ran 40 tests ... OK` (exit 0, Python 3.14.4). Full discovery
on Python **3.14.4 / SQLite 3.46.1** and Python **3.10.22 / SQLite 3.53.1** both
reports `Ran 121 tests ... FAILED (errors=7)` (exit 1). All seven errors come from
six existing daemon test methods (one has two subcases): the sandbox refuses Unix
socket bind with `PermissionError: [Errno 1] Operation not permitted`. The unchanged
81-test baseline had the same seven errors. No tests were removed, skipped or weakened.
**The full-suite pass requirement remains blocked by that environment constraint.**

Commands, from this package directory:

```sh
PYTHONPATH=src python3 -m unittest discover -s tests -p test_agy.py
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src UV_CACHE_DIR=/tmp/handoff-receipts-ag1-uv uv run --no-project --python 3.10 python -m unittest discover -s tests
PYTHONPATH=src python3 -m handoff_receipts run --help
PYTHONPATH=src python3 -m handoff_receipts --version
```

Help exits 0 and lists `--engine {codex,claude,agy}` and `--agy AGY`; version exits 0
with `0.3.0`. Forbidden passthrough CLI tests return 2 with no fake process dispatch.
The new tests cover normal/exact long Unicode input, identical-body resume, missing
or invalid resume preflight, returned/stream ID conflicts, no-ID error, multiple/
missing/malformed results, real signal forwarding to the fake, timeout, completion/
cancellation conflict, LF differences, corrupt/duplicate/mistyped wire fields,
SQL disagreement, unknown fields, WAL, real SQLite busy, delayed commit/recovery and late executor cancellation commit,
version/hash mismatch and an in-run hash change, sensitive-field exclusion, offline
verification and projection/fragment inconsistency. A handwritten wire fixture is
independent of the fake's encoder. These tests validate interpretation policy and
local mechanics; they do not validate a current native binary or model behavior.


## Antigravity native sample and follow-up fixes — 2026-11-15

The operator reported eight launch attempts using Antigravity CLI **1.5.9** with
its pinned executable hash and model **gemini-3.8-flash-low** on **2026-11-15**.
They comprise seven model processes and one refusal before dispatch.

| Case | Recorded native result and follow-up interpretation |
|---|---|
| New conversation | Input observed, completion reported, native NO_TOOL_CALL |
| Resume in the same conversation | Input observed, completion reported, native NO_TOOL_CALL |
| Approximately 10 KB Unicode input | Input observed, completion reported, native NO_TOOL_CALL |
| Invalid model | No conversation ID, status unknown, wrapper exit 2 |
| SIGINT sent to the wrapper | Interruption reported, native USER_CANCELED, input observed, child exit 1 |
| Print timeout with stream-json input | SUCCESS, finished_reason=print_timeout, empty response, child exit 0. The original verified-version cancellation conflict yielded unknown. After the correction, failed/reported and wrapper exit 2 are expected; the post-correction re-run below confirmed failed/reported and wrapper exit 2. |
| Wrapper SIGKILL followed by recover | Conversation ID preserved, status unknown with no result. The corrected recovery path records the saved executable's current fingerprint for native input confirmation. |
| Missing resume target | Refused before launch, nothing sent |

For the three completed cases, native and reported output hashes differed by the
trailing LF, as expected. The earlier argv-mode timeout probe returned ERROR with
child exit 3. The stream-json input sample returned the SUCCESS shape above.

The current result rule uses the status and reason together. SUCCESS with an
absent/null reason gives completed, with print_timeout failed, and with any other
reason unknown. ERROR with interrupted gives interrupted and with any other
reason, including absent/null, gives failed. Other statuses give unknown.
Controls distinguish absent/null reasons from unrecognized strings.

Recovery records agy_version_recover from the same saved executable, including
its --version metadata check and current hash. Verification requires a verified
before and either a verified after or, if after is absent, the latest verified
recover. Every recorded version/hash must match the pin. Any mismatch keeps
verification false even after a later matching recover. When after is absent,
before and the latest recover must also have identical executable st_dev, st_ino,
st_size, st_mtime_ns and st_ctime_ns. Missing metadata leaves verification false.
A lost result remains unknown while the recover-time fingerprint can restore
native input confirmation under those conditions.

The follow-up correction run used the fake producer. It adds six regression tests,
including the exact SUCCESS timeout shape at verified and unverified versions,
missing-after recovery, mismatched recovery fingerprints and consistent history.
The existing result-shape test also covers absent, null, empty, invalid and unknown
finished_reason values. Synthetic suite results and commands are recorded below.


The correction run retained the original 121 tests and added six, for 127 total.
The agy module passes all **46 tests** on Python **3.14.4** and **3.10.22**.
Full discovery on Python 3.14.4 reports `Ran 127 tests ... FAILED (errors=7)`.
The seven errors are the known Unix socket bind PermissionErrors in existing daemon
tests. The operator reports that those daemon tests pass in their environment.

| Command from the package directory | Exit and result |
|---|---|
| PYTHONPATH=src python3 -m unittest discover -s tests -p test_agy.py | 0, 46 tests OK |
| PYTHONPATH=src UV_CACHE_DIR=/tmp/handoff-receipts-ag1-uv uv run --no-project --python 3.10 python -m unittest discover -s tests -p test_agy.py | 0, 46 tests OK |
| PYTHONPATH=src python3 -m unittest discover -s tests | 1, 127 tests, 7 known socket PermissionErrors |

The correction run executed the fake producer and metadata checks against that
fake. The native re-run after the review regressions is recorded below.


## Antigravity review regressions — 2026-11-15

Eight synthetic regression methods were added before changing the adapter. Running
those eight against the preceding code exited 1 with `Ran 8 tests` and
`FAILED (failures=25, errors=3)`, including subcases. Each method exposed a failing
expectation. After the corrections the same eight methods exited 0 with
`Ran 8 tests ... OK`.

All methods below belong to `AG1Test` in `tests/test_agy.py`.

| Item | Test method | Failure reproduced before correction |
|---|---|---|
| R1 | test_r1_recover_pins_input_after_same_body_resume | First-attempt recovery reported ambiguous_target_match after identical-body resume |
| R2 | test_r2_recover_preserves_stream_report_and_output | The null-reason case yielded unknown after recovery. The normal absent-reason case already preserved its report. Both now preserve control events and completed output across a failed native reread. |
| B1 | test_b1_recover_requires_unchanged_executable_stat | Changed or missing stat fields still verified, and fingerprints lacked stat. Coverage also includes restoring file bytes and mtime while ctime changes. |
| B2 | test_b2_status_reason_matrix | SUCCESS/null, SUCCESS/interrupted, ERROR/other and unknown-status cases violated the decision table |
| B3 | test_b3_incomplete_input_stream_blocks_capture_and_recover | Missing user-input conversation ID or malformed stream still confirmed native input |
| B4 | test_b4_omitted_planner_does_not_stop_executor_rereads | Omitted sensitive planner stopped rereads before a delayed executor cancellation |
| B5 | test_b5_invalid_utf8_success_is_malformed | A non-UTF-8 SUCCESS response still yielded completed |
| B6 | test_b6_relative_path_uses_child_cwd_for_every_execution_check | Executable argv stayed relative with child cwd and relative PATH entries |

The current suite retains all 127 preceding tests and adds eight, for **135 total**.
The agy module passes **54 tests** on Python **3.14.4** and **3.10.22**. Full discovery
on Python 3.14.4 exits 1 with `Ran 135 tests ... FAILED (errors=7)`. These are the
known Unix socket bind PermissionErrors in existing daemon tests.

| Command from the package directory | Exit and result |
|---|---|
| PYTHONPATH=src python3 -m unittest discover -s tests -p test_agy.py | 0, 54 tests OK |
| PYTHONPATH=src UV_CACHE_DIR=/tmp/handoff-receipts-ag1-uv uv run --no-project --python 3.10 python -m unittest discover -s tests -p test_agy.py | 0, 54 tests OK |
| PYTHONPATH=src python3 -m unittest discover -s tests | 1, 135 tests, 7 known socket PermissionErrors |

Executable fingerprints now include the five stat fields. Recovery can substitute
for an absent after check only with matching before/recover stat and verified
version/hash. Executable path selection is fixed once in argv using the child cwd
for explicit paths and relative PATH entries. The saved stream controls supply
reported outcomes during recovery, with absent/null and unknown reasons stored
distinctly. Native attribution uses this attempt's complete stream index evidence.
Auxiliary omissions allow pending executor rereads to continue within budget, and
non-UTF-8 SUCCESS responses are malformed.

In an unrestricted local run, full discovery on Python 3.14.4 exited 0 with
`Ran 135 tests ... OK`.

## Antigravity native re-run after corrections — 2026-11-15

Antigravity CLI 1.5.9 with the pinned executable hash, model `gemini-3.8-flash-low`,
four model processes, no automatic retry. Receipts written before the corrections
use the earlier reason and fingerprint encoding and were not used as evidence here.

| Case | Result |
|---|---|
| New conversation | input observed, completed reported, native NO_TOOL_CALL |
| Same-body resume of that conversation | input observed, completed reported, a new execution ID |
| Recover of the first attempt after that resume | same execution as before, completed and output hash preserved, no ambiguity |
| Print timeout with stream-json input | failed reported, native USER_CANCELED, child exit 0, wrapper exit 2 |
| Wrapper SIGKILL, then recover | version verified through unchanged executable stat, input observed, status unknown with no result even though native later showed NO_TOOL_CALL |

## Continuous integration finding (2026-11-16)

The first CI run (Ubuntu 24.04, Python 3.10 and 3.14.10) exposed two issues; both are
fixed in the tree without a version change, before the first public release.

- A test assumed that a deeply nested JSON line always raises `RecursionError`.
  Python 3.14.10 decoded it instead, while 3.14.4 raised. The test now accepts
  either outcome and requires the line to be recorded as unrecognized.
- The Codex stream path accepted any JSON value and then failed with
  `AttributeError` on a valid non-object line such as `[0]`, `5` or `null`. It
  now records such a line as `stream_unrecognized` and capture continues, as the
  Claude and Antigravity paths already did. A new synthetic test covers this case.

- The second CI run, and repeated local runs on Python 3.10, showed two timing-
  dependent Antigravity tests (`busy` and `delayed_commit`). The fake writer
  released its lock or committed after a fixed 0.3 s, so on a slow host the first
  observation could miss the state under test. The fake writer now waits for an
  explicit release from the test after that first observation.

After the fixes, the two timing tests passed 10 of 10 runs each on Python 3.10.22
and 3.14.4, and 10 of 10 on 3.10 under CPU load. The full suite then ran 160 tests
with a plain `OK` three times on each version.
