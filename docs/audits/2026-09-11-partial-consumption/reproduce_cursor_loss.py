#!/usr/bin/env python3
"""Reproduce the original audited ledger's loss of a partial-settlement tail.

Compiles the production optimistic.rs module, without its existing tests, with
minimal stand-ins for external hash/header/HeldTx types. The ledger methods are
unchanged. This exercises ledger transitions, not L1, EVM execution, or networking.
Success means the defect was reproduced; this is not a regression acceptance test.
"""

import hashlib
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[3]
SOURCE = "crates/eez-composer/src/optimistic.rs"
BASELINE = "c2e7b647593217876094090f85bc0f7502ef2492"

PRELUDE = r"""
#![allow(dead_code)]
extern crate self as alloy_primitives;
extern crate self as alloy_consensus;
extern crate self as reth_primitives_traits;

pub type TxHash = u64;
#[derive(Debug, Clone)]
pub struct Header;
#[derive(Debug, Clone)]
pub struct SealedHeader<T>(pub T);
mod held_pool {
    #[derive(Debug, Clone)]
    pub struct HeldTx { pub hash: u64 }
}
mod optimistic {
"""

DRIVER = r"""
}
fn main() {
    use held_pool::HeldTx;
    use optimistic::OptimisticallyIncluded;

    // Scenario supplied to the diagnostic: the original optimistic batch has
    // U1,U2,U3; L1 settles only U1,U2, and the deriver replaces the Sync block
    // at the same height. The ledger API receives only that height.
    for observer_first in [true, false] {
        let ledger = OptimisticallyIncluded::new();
        ledger.begin(
            10,
            100,
            SealedHeader(Header),
            vec![HeldTx { hash: 1 }, HeldTx { hash: 2 }, HeldTx { hash: 3 }],
        );
        if observer_first {
            ledger.mark_failed(10, false);
        }
        let released = ledger.resolve_below_cursor(10);
        if !observer_first {
            ledger.mark_failed(10, false);
        }
        let released_hashes: Vec<u64> = released.iter().map(|tx| tx.hash).collect();
        let recoverable = ledger.take_failed_for_recovery(10);
        assert_eq!(released_hashes, [1, 2, 3]);
        assert!(recoverable.is_none());
        assert_eq!(ledger.blocking_height(10), None);
        assert!(ledger.resolve_below_cursor(10).is_empty());
        println!(
            "REPRODUCED: observer_first={observer_first}; released={released_hashes:?}; \
             unconsumed U3 has no recovery result; next emission is unblocked"
        );
    }
}
"""


def main():
    raw = subprocess.check_output(["git", "show", f"{BASELINE}:{SOURCE}"], cwd=ROOT)
    source = raw.decode()
    production, separator, _ = source.partition("\n#[cfg(test)]\n")
    if not separator:
        raise RuntimeError("production/test boundary changed; review this diagnostic")
    print(f"source: {BASELINE}:{SOURCE}", flush=True)
    print(f"sha256: {hashlib.sha256(raw).hexdigest()}", flush=True)
    with tempfile.TemporaryDirectory(prefix="eez-partial-consumption-") as work:
        harness = Path(work) / "ledger.rs"
        binary = Path(work) / "ledger"
        harness.write_text(PRELUDE + production + DRIVER)
        subprocess.run(
            ["rustc", "--edition=2024", str(harness), "-o", str(binary)],
            check=True,
        )
        subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    main()
