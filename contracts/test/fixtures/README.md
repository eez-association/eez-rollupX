`l1-storage-eip1186.json` was captured from an isolated Foundry Anvil 1.7.1,
chain 1337, Osaka active, genesis timestamp 1700000000. Anvil's first public
development account sent the recorded `initCode` as a CREATE transaction at
timestamp 1700000001, gas limit 0x300000. Its constructor stores 64 values:
slot 7 holds 45, every other slot `i` in 0..63 holds `i + 100`; runtime is STOP.

The file contains the actual `eth_getProof` results at the mined block for that
contract's slots 7 and 999 and for the absent address 0xdead. `cast block --raw`
provided the canonical header; the recorded block hash independently checks it.
The tests use these bytes directly, without building their expected trie with
the verifier implementation. The header/proofs authenticate values and absence;
the fixtures do not test L1 consensus or the history import mechanism.

`rollupx-baseline-init.hex` is the unmodified `OfflineRollup` creation bytecode
from fork `6215df1a4395f58bb58bdb78bf4dc22f907c4169`, protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`, built with Foundry 1.7.1 and
`contracts/foundry.toml` (Solidity 0.8.34, optimizer runs 1, via IR, relative
remappings). It contains no constructor arguments. SHA-256 of the hex file:
`0dece45fd60d80ac59c4d27f8e51cdd6e121651d34708de9c3239f7e6dba6382`.
Reproduce at that baseline with `forge inspect --root contracts OfflineRollup bytecode`.
The matching unpatched runtime is retained as
[`rollupx-baseline.hex`](../../../crates/eez-composer/src/composer/estimate/testdata/rollupx-baseline.hex).
Upgrade tests deploy and initialize these frozen bytes, then upgrade the same proxy
to `RollupX`; replacing the fixture with current output would invalidate that test.
The [manager audit](../../../docs/audits/2026-10-28-rollupx-manager/README.md)
records compatibility and gas comparisons.
