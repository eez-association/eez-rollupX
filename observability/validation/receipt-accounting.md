# Receipt arithmetic and saved audit validation

2026-09-16, Rust 1.94.0. The observer's receipt read model validates inclusion
identity/status independently of accounting completeness. Missing or invalid gas,
effective price, blob gas or blob price yields a typed gap. Known non-blob types
have no blob charge; unexpected nonzero blob fields are rejected. Supported
receipt transaction types are 0–4; other types remain accounting gaps.

Individual values are bounded U256; each multiplication widens to U512. Reverted
receipts pay the same gas/blob fees. Limits, caps and estimates are ignored by
paid-cost arithmetic. No floating-point currency conversions or bounded lifetime
sums are exposed by the model: PostgreSQL will sum exact unconstrained NUMERIC.

The runtime's existing `eez-l1::posting_fees::ReceiptCost` has narrower u64/u128
input fields and is tied to protocol dependencies. The read-only observer uses
the same fee formula and physical-blob gas rule with the proposal's full U256
input range, without changing runtime fee policy. The observer rejects non-blob
nonzero blob-price fields as inconsistent data; the native runtime currently
ignores that irrelevant price. This affects coverage, never protocol behavior.

Passed:

```bash
cargo +1.94.0 test --locked --offline -p eez-observer
cargo +1.94.0 clippy --locked --offline -p eez-observer --all-targets -- -D warnings
cargo +1.94.0 fmt -p eez-observer --check
git diff --check
```

Thirteen observer tests passed, including all nine exporter regressions. Four
new tests verify canonical-revert charge arithmetic, missing/invalid fields and
unsupported types, full-width multiplication beyond U256, and the complete saved
[accounting corpus](../tests/fixtures/README.md). Every post's explicit bundle
membership reconciles; totals match the independently saved audit exactly, with
L2 fees excluded. The extraction verified saved evidence checksums and canonical
references and retained no signing material or calldata.

This is not yet durable indexing. PostgreSQL migrations, atomic cursor storage,
fork/finality reconciliation, fee views, authenticated blob measurements and
SQL-backed dashboards remain open release gates.
