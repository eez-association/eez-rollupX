"""Characterize the pinned upstream defect, not a repaired runtime test.

Only HTTP transport is replaced. The real helper returns the first optimistic
receipt; a later canonical receipt can have a different block identity.
"""
from pathlib import Path
import subprocess
import unittest

REPO = Path(__file__).resolve().parents[3]


class OptimisticReceiptAssumptionTest(unittest.TestCase):
    def test_real_pinned_helper_does_not_establish_receipt_stability(self):
        script = r'''
set -euo pipefail
source eez-core-protocol/script/e2e/lib/E2EBase.sh
phase=optimistic
curl() {
    [[ $(jq -r .method <<<"${!#}") == eth_getTransactionReceipt ]] || return 91
    local height=0x38a hash=old
    if [[ $phase == settled ]]; then height=0x390; hash=new; fi
    jq -cn --arg height "$height" --arg hash "$hash" \
      '{jsonrpc:"2.0",id:1,result:{transactionHash:"same-tx",blockNumber:$height,blockHash:$hash,status:"0x1"}}'
}
first=$(_poll_receipt fixture same-tx "$(( $(date +%s) + 5 ))")
phase=settled
last=$(_poll_receipt fixture same-tx "$(( $(date +%s) + 5 ))")
[[ $(jq -r .transactionHash <<<"$first") == $(jq -r .transactionHash <<<"$last") ]]
[[ $(jq -r .blockNumber <<<"$first") == 0x38a ]]
[[ $(jq -r .blockNumber <<<"$last") == 0x390 ]]
if [[ $(jq -r .blockHash <<<"$first") != $(jq -r .blockHash <<<"$last") ]]; then
    echo 'EXPECTED BASELINE FAILURE: cached receipt is not stable'
    exit 17
fi
'''
        result = subprocess.run(['bash', '-c', script], cwd=REPO, text=True,
                                capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
        self.assertIn('EXPECTED BASELINE FAILURE', result.stdout)


if __name__ == '__main__':
    unittest.main()
