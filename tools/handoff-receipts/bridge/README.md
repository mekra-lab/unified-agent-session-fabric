# Optional Polylogue bridge

The Python CLI handles normal capture/query operations on its own.
Polylogue export requires an executable implementing the protocol in
[polylogue_import.rs](polylogue_import.rs). A missing executable makes export exit 2
with an explicit missing-bridge error; native ledger import requires the bridge.

This bridge targets Polylogue revision
`3fd060daad3b2893f8394eae7cfa3c4c15cb4840` (short `3fd060d`). Obtain that source
separately and create a Cargo project outside this package and its Cargo workspace.
Use an edition-2024 binary whose main file is the supplied source, with dependencies:

```toml
[package]
name = "handoff-receipts-polylogue"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "handoff-receipts-polylogue"
path = "polylogue_import.rs"

[dependencies]
polylogue-store = { path = "/path/to/polylogue/crates/store" }
polylogue-model = { path = "/path/to/polylogue/crates/model" }
serde_json = "1"
```

Build with `cargo build --release --manifest-path /path/to/bridge/Cargo.toml`, then:

```sh
handoff-receipts export ATTEMPT_ID --format polylogue --output new-export-dir \
  --polylogue-bridge /path/to/bridge/target/release/handoff-receipts-polylogue
```

The bridge accepts `INPUT_JSON ARCHIVE_DIRECTORY`, imports `UserWrite::Deliver`
and optional `DeliveryEvent`, then returns `Reader::delivery` results as JSON on
stdout. Input bytes and delivery count are checked after native readback. Stable
UUIDv5 delivery IDs map attempts. A delivered event means the retained native
input matched exactly; no used event is fabricated. Unknown thread IDs cannot be
represented as native recipients and cause a clear failure; use JSONL instead.
Process/RPC details, turn results, retry grouping and selective-line inventories
travel in `common-E.jsonl` beside the native archive. No HandoffGraph adapter is
included.

The caller supplies a trusted executable. Python records the targeted contract
revision but cannot prove the executable's source revision; the assessment
explicitly reports `bridge_revision_verified: false`. Native readback was tested
using an existing bridge built for the stated revision. Building the supplied
source provides a reproducible integration option. Binary distribution remains outside
the package's scope.
