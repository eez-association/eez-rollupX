# Mixed-call causal evidence

These are canonical calldata and receipt inputs captured read-only on 2026-09-24
from the existing Chiado events network, after the successful protocol staged run
`20260923-214411`. They are public chain data, with no wallet keys or RPC credentials.

Protocol: `820e50f749f63165be6a5797d4c5fe52cbbbbe34`.
L2 chain ID: 6291; rollup ID: 1 under registry
`0x5d6b8fc56e43528c219de7eed257564c05923e08`.
Both cases settled in L1 transaction
`0x19afc8880c7ca75b1969a11f258f3d0dbe401719d02abd47969b9efeac2d7b4d`.
The complete posted batch is retained, including other users' entries.

| Case | L2 user transaction | Canonical per-user table load |
| --- | --- | --- |
| Read, write, changed read | `0x78498831ca72cdb02507cb2f5b10fa10032a3d854f7bb1f3c20d53abc1d39ecd` | `0x48514184509909e6079a350f73dc7ab898e76129350fb5e43a3ae7a4a3089715` |
| Repeated reads before and after a write | `0xcdc42db1dabc178dd34cb7bc6210705372ac387be80cea237a32e28902be734c` | `0xa78aaf7eb6d525dc24cc2292ba2da9833279283dc3b7dfdbc0f5c071e955c54d` |

The staged log's first-load hint belongs to another user in the same block.
Selection used `scripts/ci/select-protocol-l2-transactions.py` with the scenario's
`EXPECTED_L2_HASHES` and the complete canonical block; the selected calldata and
receipts remain unfiltered. These inputs reproduce the baseline verifier's
`expected one L1 host ... found 0` error. `causal_evidence` tests the shipping
verifier against these inputs and rejects mutated tables and receipt events.

This offline evidence tests table/result/event binding. It does not rerun EVM
execution, authenticate a Beacon blob, or establish current chain canonicality.
