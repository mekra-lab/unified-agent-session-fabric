# Frozen synthetic conformance bytes

This bundle contains 34 synthetic scenarios and 863 CLI comparisons, all using
generated data. `manifest.json` fixes operation order, fixture writes,
index removal, exit codes and content addresses. `objects.jsonl` stores each
unique byte sequence once, keyed by its SHA-256. The manifest also pins the
pack's byte length and SHA-256. Both levels are verified before replay.

Each JSONL row contains `sha256`, original byte `length`, and either `utf8`
(an exactly reversible JSON string, escaped for ASCII storage) or `bytes`
(literal byte values for malformed UTF-8 fixtures only). These are original
wire bytes, preserved verbatim when creating the pack.
Whitespace, numeric spelling, line endings and invalid input bytes survive
unchanged. Neither compression nor hex encoding is used.

The integration test in `../golden.rs` restores these bytes and runs every
comparison locally in Rust using only the frozen fixtures. Response
comparison ignores JSON object key order and equivalent float spelling; array
order, missing fields, numeric types and signed floating zero remain significant.
Regeneration requires an explicit maintenance action separate from `cargo test`.
