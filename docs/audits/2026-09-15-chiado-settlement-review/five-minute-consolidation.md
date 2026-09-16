# Five-minute Chiado consolidation

At **14:06:55 UTC on 2026-09-15**, the live composer was restarted with
`EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS=300`, previously `1800`. At the configured
one-second L2 cadence, this makes empty consolidation due after approximately
five minutes. `EEZ_SETTLEMENT_PURE_L2_MODE=always` remains active.

Only this value changed in the private runtime environment. The composer image,
executable, database bindings and other settings were preserved. The beacon and
proof signer were not restarted. The previous safe block, **142,815**, retained
its hash and state root. Repository defaults were not changed.

The new composer first consolidated **1,040 accumulated empty L2 blocks in four
posts**. This was history accumulated under the previous configuration. Once
that drain finished at L1 slot **14:07:25**, there were no additional posts before
the new general threshold became due.

The first subsequent idle cycle verified the configured behavior:

| Evidence | Result |
| --- | --- |
| Last decision below threshold | 295 unsettled blocks; settlement not due |
| First due decision | Exactly 300 unsettled blocks; reason `general` |
| Estimated stream at that decision | 229,483 bytes within two blobs' 253,952-byte capacity |
| Included postbatch | One post, two blobs, 315 empty L2 blocks |
| Canonical L2 range | 143,856–144,170; all 315 hashes checked against local canonical blocks |
| L1 inclusion | Block 23,069,571, slot 14:12:45 |
| Interval between successful L1 inclusions | 320 seconds |

The [successful postbatch](https://gnosis-chiado.blockscout.com/tx/0xf3738243f39b213e8816c5756fb91fac32d7e3d7445dc3a73e101af9ac43a2f7)
has a successful canonical receipt. Its two earlier attempts targeted skipped
Chiado slots at **14:12:25** and **14:12:35**. Adjacent canonical L1 timestamps
confirm both gaps. The composer retried and settled the accumulated range in one
post. The setting is a trigger; it does not guarantee L1 inclusion every exactly
five minutes or a particular bridge latency.

Final checks found an empty cross-chain queue, advancing L1/L2 heads, and a
fully validating beacon with **24 connected peers**. No test transactions were
sent. This was a runtime configuration change using the previously validated
executable; it did not introduce a source-code change.

[Compact evidence](five-minute-consolidation.json) records the exact executable,
settings, canonical receipt, retry events and health checks. Local deployment
and observation artifacts are under
`artifacts/chiado-five-minute-consolidation-20260915/`. The initial deployment
script stopped verification at a direct mount-list comparison after restart;
independent verification confirmed the unchanged configured bindings, preserved
canonical state and correct live environment.
