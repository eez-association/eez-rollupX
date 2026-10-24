# Native profile engineering closure, 2026-10-23

This is an addendum to the [divergence review](README.md). It records completion
of specification items E1/E2, not implementation of the new profile or new runtime
test coverage. The owner's scope decisions are incorporated in the
[branch proposal](../../proposals/l1-origin-and-offline-proving.md).

## Source and result

| Item | Identity/result |
| --- | --- |
| Worktree | `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving` |
| Branch | `codex/l1-history-offline-proving` |
| HEAD | `3e23955dc477c9e675364776f118b1cecda75774`, with existing local recorded-history changes preserved |
| Core gitlink/checkout | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, `feature/e2e_edge_cases`; no submodule edits |
| Native specification SHA-256 | `9ec6e14de8fe76581ab06126b53ea000d10f73cb032960f4471a829f8d5359fa` |
| Reference corpus SHA-256 | `70372893c734ba73d8f3838354ade316585b5e2783be44440b8559e5f47b88e1` |
| Encoder oracle | Foundry cast 1.7.1, commit `4072e48705af9d93e3c0f6e29e93b5e9a40caed8`, dist profile |
| E1/E2 | Complete deterministic target specification; no additional product decision |
| E3 | Runtime implementation, generated execution fixtures, combined integration validation and PR review pending |

No runtime source, protocol pin, service, deployment, wallet or existing root
checkout was changed for this specification closure. Existing dirty implementation
files predate this work. The collector's actual address remains a required input
for each deployment; no production recipient was chosen.

## Definitions and review

The [native specification](../../proposals/native-system-transactions.md) now
fixes version 1 of type 0x45, all source preimages/indexes, no system nonce,
gas/RPC/receipt behavior, required collector configuration, manager ProfileV3
and portable identity schema 6/specification 3. Proving wire v2 is unchanged.

The independent read-only review traced the design through the pinned call hash,
semantic reconstruction/materialization, queue/table indexing, gas/nonce execution,
partial-consumption reproof, profile/context encoding and the L1/L2 factory split.
Its corrections are incorporated:

- Root ordinals count failed/static/reverted roots and differ from mutable-table
  source indexes. Keep the outbound-first stream rule and original positions
  after partial consumption.
- Callbacks execute inline. Reject the generic helper's standalone callback
  representation; preserve nested funding for both delivery and load.
- No acceptedRoot/B0 field is needed in sourceHash. The proof authenticates state;
  the source identifies an occurrence and must survive reproof of a consumed prefix.
- History binds the current L2 block's parent at n−1, independently of settlement.
- Compare the serialized gas limit to the original position's remaining budget;
  disabling nonce validation alone does not remove the EVM's nonce increment.
- Credit the collector per ordinary L2 transaction before checkpoints, including
  aliasing and initially absent accounts. Do not apply this policy to L1 simulation.
- Bind the collector explicitly in the profile, because genesis header hashing
  alone does not cover arbitrary chain configuration.

The reviewer reported no remaining design blocker or product choice. This was
static review, not a successful execution of the future implementation.

## Encoding checks

The [27 reference vectors](../../../crates/eez-control-rpc/conformance/native-profile-v1.json)
were constructed with manual Python ABI/RLP encoding and cross-checked against
Foundry. Ethereum Keccak was checked against its known empty-input digest.
A separate replay from the stored public arguments passed all 27 encodings and
13 equality/distinction assertions. The corpus covers domains, the pinned call
hash, source positions/context, partial-prefix identity, recorded history,
ProfileV3, portable identity, native envelopes and a receipt.

These are **synthetic encoding oracles**. They do not certify a valid genesis,
signed origin, execution, L1 ancestry, settlement, rejection path or Nethermind
implementation. Some inputs intentionally isolate a formula without satisfying
the complete runtime schedule. Existing v2 process recordings still correspond
to the previous native/fee profile and must be regenerated for E3.

Replay from the worktree root, using installed Foundry 1.7.1:

```bash
python3 - <<'PY'
import json, subprocess
from pathlib import Path
def cast(*args):
    return subprocess.check_output(["cast", *args], text=True).strip().lower()
p = Path("crates/eez-control-rpc/conformance/native-profile-v1.json")
corpus = json.loads(p.read_text())
assert len(corpus["vectors"]) == 27
index = {}
for v in corpus["vectors"]:
    assert v["id"] not in index
    if v["encoding"] == "abi":
        encoded = cast("abi-encode", v["signature"], *v["arguments"])
    elif v["encoding"] == "calldata":
        encoded = cast("calldata", v["signature"], *v["arguments"])
    else:
        assert v["encoding"] == "rlp"
        encoded = v["prefix"] + cast("to-rlp", json.dumps(v["fields"]))[2:]
    assert encoded == v["expectedBytes"], v["id"]
    assert cast("keccak", encoded) == v["expectedKeccak256"], v["id"]
    index[v["id"]] = v
for r in corpus["relationships"]:
    equal = index[r["left"]]["expectedKeccak256"] == index[r["right"]]["expectedKeccak256"]
    assert equal == (r["comparison"] == "equal"), r
print(f'{len(index)} encodings and {len(corpus["relationships"])} relationships: PASS')
PY
git diff --check
```

Both commands passed. Edited local documentation links/section targets were also
checked. No Cargo, Solidity execution, E2E, live network or Nethermind tests were
run: this closure changed specification/reference data only, and the new runtime
does not exist yet. Runtime validation remains the owning proposal's acceptance
matrix; the DELEGATECALL/CALLCODE code-context gap also requires its stated
focused regression.
