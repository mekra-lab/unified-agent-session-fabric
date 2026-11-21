# UASF JSON

Shared token-aware JSON decoding and canonicalization for the Rust components.
See the [JSON contract](../../docs/reference.md#json-and-equality) and
[fail-closed deletion rules](../../docs/security-and-limits.md#failure-and-experimental-restrictions).

## API boundary

Use `from_slice` or `from_str` for wire bytes, source records, journals and stored
JSON. `from_slice_unique` adds duplicate-key rejection for signed feeder metadata
and complete object lines. Containers are built from raw tokens, so ordinary
marker keys remain objects. Only this crate enables `arbitrary_precision`, for
exact integer representation; consumers must not directly deserialize untrusted
objects through that representation's reserved-key mechanism.

`FromValue` inspects already decoded trees. Value conversion is identity; no
generic deserializer is exported. `canonical` recursively preserves numeric type,
signed floating zero and array order while sorting object keys. Gate keeps its
separate archive equality rules and explicit structural integer checks.

## Tests

Run `cargo test --offline --locked -p uasf-json` from the repository root.
`tests/inspection.json` is a shared synthetic corpus for consumer regression tests:
escaped marker objects, numeric overflow and deep text/session fields must not
bypass deletion inspection. The tests run directly in Rust.
