# Strict FIFO admission correction

Status: implementation and validation in progress. This record supersedes no
older test result; final candidate evidence is recorded below as it completes.

## Contract and baseline

- User correction: older inbounds must not wait behind younger outbounds;
  restore FIFO admission. [Accepted scope](../../proposals/cross-chain-admission-throughput.md#september-27-correction-strict-fifo-admission).
- Branch / PR: `feat/cross-chain-admission-throughput`,
  [#42](https://github.com/eduadiez/rollupX/pull/42), base `main` at
  `fd77a2ebc147850333171045dbd263ec74156268`.
- Source baseline: `f6e49735f8d4289c896c205080b664fdcd5a0315`.
- Protocol: clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`;
  no protocol, dependency or wire changes.
- Runtime baseline: recorded shipping binaries from `62b8e06`, whose production
  source equals `f6e4973` (intervening commits change tests/evidence only).
  Composer SHA-256 `b8bf1f034211cfccc03fdf10750bd51d938584f4b4cb4883908832f5c390bc76`.
  Full [previous identity](../2026-09-27-admission-main-alignment/README.md) is dated evidence,
  not validation of this correction.
- Operator Chiado runbook edits and unrelated audit directories were preserved.

## Reproduction and correction

Canonical composition processes outbound groups before inbound deliveries. The
previous selector treated capacity refusals as local source-chain blocks, allowing
later cheap sources through and allowing younger outbounds to consume the budget
before older inbounds. The diagnostic `fifo_bypassed` did not prevent submission.

`gnosis_gas_cut_preserves_fifo_even_for_cheaper_independent_work` fails against the
recorded baseline at its explicit `fifo_bypassed == false` assertion: the observed
value is true. All four sources completed and the follower converged before the
assertion. First native failure logs/datadir and executable hashes are retained
locally in `artifacts/admission-strict-fifo/baseline-gas-native/`.
The initial sandbox invocation could not bind local ports and did not reach the
node; its separate `baseline-gas/` record is an environment failure.

The correction stops younger selection at capacity refusals. Mixed candidates
that omit an older eligible source are discarded and recomposed from a smaller
FIFO snapshot before proof generation, with the same deadline and target. The
pool remembers that source boundary across unfinished attempts and restores
snapshot order without losing queued replacements or nonce predecessors.
Classified source conditions remain eligibility exceptions. Fitting mixed batches
retain the canonical outbound-first profile and require no repair.

The first unit run caught a restoration bug: reverse front insertion could move
an independent younger source before a newly recovered predecessor. Restoration
now seeds the original snapshot order and inserts concurrent arrivals using the
pool's nonce rule. The first failure is retained in `composer-unit.log`; the
corrected run has a separate log.

## Validation and operational limits

Pending completion. Local-only artifacts under `artifacts/admission-strict-fifo/`
are ignored by Git and unavailable in another checkout. The existing live Chiado
node/signer and writable datadirs are unchanged. The main-branch Reth migration's
existing-network compatibility limitations remain in effect.
