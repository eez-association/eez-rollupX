# Captured protocol lookup logs

Scenario logs from the homelab Kurtosis run (trailing whitespace normalized):
https://github.com/eduadiez/rollupX/actions/runs/34220104933

- PR head: `6cd52b12316b1ed1fbb0ff166579888c27dd64f1`
- Tested merge revision: `1f6bf42cf398a5249aff7bdcf74b147fe5307e76`
- Protocol revision: `3c0cdf4955941a5c23639346932b55b33babebbd`
- Artifact paths: `protocol-e2e/topLevelStaticCounter.log` and
  `protocol-e2e/revertCounter.log`

The collector tests substitute transaction hashes and contract addresses to
match mocked RPC responses, but retain upstream's real verification messages.
Static lookups use a static-table marker. The reverting lookup does not emit a
rolling-hash replay marker; the collector must verify that hash independently.

## Events-protocol proxy traces

`topLevelStaticCounter-events-trace.json` and `revertCounter-events-trace.json`
contain actual Chiado `debug_traceTransaction` callTracer results captured
read-only on 2026-09-24 at protocol revision
`820e50f749f63165be6a5797d4c5fe52cbbbbe34`. Their expected calldata/results are
encoded from the scenario's source address and `counter()`/`increment()` call:
returned counter 1, or `Error("always reverts")` respectively.

The static trigger is
`0xfec44349e5767840521cee2f9d96e03ef36dccdf7e26e4d34a651c711fe4458f`.
The revert trigger is
`0x9e5babbc1194cffedcdeb31d920f227893584038aa9669fb0be370418143c5c9`.
Both traces include the pinned proxy's `STATIC_CHECK_GAS()` registry read,
self-probe, and actual registry lookup. The old one-registry-frame assertion
rejects both. These are public transaction traces without operator credentials.

## Static round-trip logs

`topLevelStaticReentrantCounter.txt` and `topLevelStaticReentrantCounterL2.txt`
are the upstream scenario logs from the Kurtosis run
https://github.com/eduadiez/rollupX/actions/runs/37822814018 (trailing
whitespace normalized):

- PR 77 head: `434bfda07ed0bcc183bd8211bbb45cb2e747feb0`
- Protocol revision: `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` (v0.1.0-rc.2)
- Artifact paths: `protocol-e2e/network.*/topLevelStaticReentrantCounter.log`
  and `protocol-e2e/network.*/topLevelStaticReentrantCounterL2.log`

Each static entry carries one static callback. The collector must bind it in
the posted or loaded table, in the semantic blob and, for the L1 read, in the
traced lookup.

`topLevelStaticReentrantCounter-trace.json` is the actual `callTracer` result
of that scenario's user transaction, captured during review on fresh local
Anvil fixture chains at protocol rc.2 (SHA-256
`d7c6a16681b88ea984df5cd1b7950f5264f43718fa3a9f5bc5d655e3f8218dad`). Before
the read, its reader queries the registry for the callback source proxy's
address (`computeCrossChainProxyAddress`), so the trace has three registry
frames. Addresses are local fixture identities, not operator credentials.

## Static reads around L1 local writes

`staticLocalWrite.txt` and `staticLocalWrite-trace.json` were captured from the
isolated Kurtosis network on 2026-10-24, using RollupX PR #101's R03 runtime and
protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f`. The log is the original
upstream success whose independent collector initially failed because this
scenario had no lookup oracle. Its transaction is
`0xbdcb4a911b1b2a2657a0164fdbf9c903ef0eb85a78dec481e216fbbf3b2bd1bc`.

The captured trace contains three `quote()` lookups returning 1, 2, 1. Their
`rate()` callback attempts return 1; 2, 2; 1: the middle lookup rejects candidate
A before selecting B, and the last lookup reuses A. The posted table contains
A/B/A, including an identical repeated row; two distinct outcomes do not imply
exactly two serialized rows. The regression uses independently encoded expected
calldata, and mutations must fail the real shell/jq consumer. RPC, KZG and Forge
boundaries are mocked in unit tests; the audit records their separate live
read-only validation.
