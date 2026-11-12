# Backport review handoff

Drafts for approval. Structural validation is not technical verification.

Manifest SHA-256: `054666c13c01671ec451a5541254cfd81e1d3ca6fce2ece97fa11e42678f72cc`

## Frozen scope

Issue #139 / original handoff N4 only: assess Hickory 0.26.3. This is a newly scoped investigation manifest, not a reconstruction or replacement of the original 174-commit audit. Original inventory digest and input provenance were not independently checked. Fresh observations use target main 20243f70839ec69df9312731e960cd311e060050 and do not replace the frozen baselines.

- Source: eez-association/eez-rollup0 at `1b66d9bda675e6f8e83f514153a97136909acc06`; protocol `143d87e62cade98bfc122d612638dcaaf1c85633`.
- Target: eez-association/eez-rollupX at `31e148f50dd79730b17d113ac78795eb377eb5c5`; protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Merge base: `d8a7547b311260fdc3262769429a68212fb18bf9`.

## Finding coverage

- **N4 — Assess the Hickory 0.26.3 patch update:** [Existing issue](<https://github.com/eez-association/eez-rollupX/issues/139>): Partially covered: shipping proto 0.26.2 already fixes the current RustSec proto findings, while net/resolver remain 0.26.1. Recommend a scoped 0.26.3 update after approval and runtime validation, including the separate vendored-Reth test lockfile. The repository assessment supplies compatibility, reachability, fresh scans, identity implications, residual risks and the implementation/check definition without changing existing completion criteria. Original input provenance was not independently checked. Evidence: [Frozen source lockfile](<https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/Cargo.lock#L4852>), [Frozen target lockfile](<https://github.com/eez-association/eez-rollupX/blob/31e148f50dd79730b17d113ac78795eb377eb5c5/Cargo.lock#L4577>), [Fresh target lockfile, not frozen baseline](<https://github.com/eez-association/eez-rollupX/blob/20243f70839ec69df9312731e960cd311e060050/Cargo.lock#L4577>) (not frozen baseline), [Current dependency and identity contract, not frozen baseline](<https://github.com/eez-association/eez-rollupX/blob/20243f70839ec69df9312731e960cd311e060050/crates/eez-control-rpc/VALIDATION_IDENTITY.md#L164>) (not frozen baseline)
