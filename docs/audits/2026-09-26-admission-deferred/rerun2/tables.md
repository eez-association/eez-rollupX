# Cutoffs and postBatch gas

All gas values are gas units. Estimates describe a whole postBatch; inbound source transaction gas is additional.

## Cutoffs

| Cutoff | All candidates | Canonical postings | Canonical sources | Meaning |
| --- | ---: | ---: | ---: | --- |
| `fifo_fairness_limit` | 5 | 4 | 143 | A count cap follows a prior selection that bypassed older eligible FIFO work. |
| `l1_bundle_gas_budget` | 4 | 2 | 73 | PostBatch plus ordered inbound companion allowance exceeds the parent block budget. |
| `l1_tx_gas_budget` | 8 | 7 | 262 | Next complete group exceeds the postBatch planning cap. |
| `pipeline_deadline_backoff` | 1 | 1 | 18 | Count cap after an actual shared-window failure. |
| `pipeline_time` | 15 | 7 | 264 | Predicted remaining preparation, proof and submission work no longer fits. |
| `snapshot_exhausted` | 1 | 1 | 34 | All sources in the finite snapshot were accepted; this is not a resource rejection. |
| `submission_backoff` | 9 | 5 | 76 | Count cap shrinks selection after conclusive submission failure/non-inclusion. |

## Gas by canonical posting

| L1 block | Sources | Admission | Final projection | Preflight required | Signed limit | Receipt used | Cutoff |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 23241627 | 18 | 4,128,724 | 4,169,296 | 2,414,622 | 2,535,354 | 1,931,698 | `pipeline_deadline_backoff` |
| 23241628 | 36 | 15,370,504 | 15,441,428 | 12,872,422 | 13,516,044 | 12,872,422 | `l1_tx_gas_budget` |
| 23241629 | 36 | 6,587,108 | 6,658,116 | 3,546,907 | 3,724,253 | 2,975,707 | `fifo_fairness_limit` |
| 23241630 | 36 | 15,370,480 | 15,441,428 | 12,872,482 | 13,516,107 | 12,872,482 | `l1_tx_gas_budget` |
| 23241631 | 36 | 4,989,096 | 5,060,092 | 2,225,121 | 2,336,378 | 2,225,121 | `fifo_fairness_limit` |
| 23241632 | 48 | 15,221,194 | 15,321,086 | 11,952,283 | 12,549,898 | 11,952,283 | `l1_tx_gas_budget` |
| 23241635 | 11 | 2,156,306 | 2,188,726 | 1,350,651 | 1,418,184 | 1,350,651 | `submission_backoff` |
| 23241636 | 40 | 14,288,818 | 14,390,578 | 12,068,791 | 12,672,231 | 12,068,791 | `pipeline_time` |
| 23241637 | 40 | 8,301,898 | 8,380,390 | 5,147,021 | 5,404,373 | 4,863,821 | `fifo_fairness_limit` |
| 23241638 | 31 | 15,000,494 | 15,083,714 | 13,412,867 | 14,083,511 | 13,403,267 | `fifo_fairness_limit` |
| 23241639 | 33 | 15,194,464 | 15,260,744 | 12,333,013 | 12,949,664 | 12,333,013 | `l1_tx_gas_budget` |
| 23241640 | 31 | 10,168,160 | 10,261,172 | 5,431,399 | 5,702,969 | 4,994,599 | `pipeline_time` |
| 23241641 | 36 | 15,292,668 | 15,363,820 | 12,590,564 | 13,220,093 | 12,590,564 | `l1_tx_gas_budget` |
| 23241642 | 18 | 8,696,728 | 8,794,816 | 4,193,489 | 4,403,164 | 4,006,289 | `pipeline_time` |
| 23241643 | 37 | 15,219,180 | 15,291,936 | 12,766,697 | 13,405,032 | 12,766,697 | `l1_tx_gas_budget` |
| 23241644 | 18 | 8,144,768 | 8,242,892 | 3,667,316 | 3,850,682 | 3,667,316 | `pipeline_time` |
| 23241645 | 36 | 15,370,936 | 15,441,908 | 13,173,070 | 13,831,724 | 13,173,070 | `l1_tx_gas_budget` |
| 23241648 | 13 | 6,305,596 | 6,369,780 | 3,710,803 | 3,896,344 | 3,374,803 | `submission_backoff` |
| 23241649 | 36 | 11,519,176 | 11,649,524 | 5,940,724 | 6,237,761 | 5,359,924 | `pipeline_time` |
| 23241652 | 13 | 2,687,376 | 2,731,220 | 1,126,220 | 1,182,531 | 1,126,220 | `submission_backoff` |
| 23241653 | 60 | 8,076,124 | 8,192,876 | 3,399,501 | 3,569,477 | 3,399,501 | `pipeline_time` |
| 23241655 | 29 | 4,065,604 | 4,125,096 | 1,883,624 | 1,977,806 | 1,883,624 | `submission_backoff` |
| 23241656 | 61 | 10,664,332 | 10,780,584 | 5,355,262 | 5,623,026 | 5,302,462 | `pipeline_time` |
| 23241659 | 10 | 2,432,844 | 2,461,020 | 1,515,506 | 1,591,282 | 1,462,706 | `submission_backoff` |
| 23241660 | 37 | 13,180,248 | 13,256,040 | 5,192,737 | 5,452,374 | 5,034,337 | `l1_bundle_gas_budget` |
| 23241661 | 36 | 13,660,600 | 13,732,028 | 5,934,484 | 6,231,209 | 5,281,684 | `l1_bundle_gas_budget` |
| 23241662 | 34 | 12,970,236 | 13,038,252 | 6,140,878 | 6,447,922 | 5,732,878 | `snapshot_exhausted` |
