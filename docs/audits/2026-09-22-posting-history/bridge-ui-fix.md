# Bridge UI configuration repair — 2026-09-22

The bridge page repeatedly reported `Unable to check the bridge on L1. Retrying…`. The RPC was reachable, but `manager()` reverted: the UI had been pointed at the workload `BridgeSender` and `BridgeReceiver` contracts. These do not implement the wallet bridge interface. The former wallet bridge belonged to the previous registry and was not reusable.

Deployed the unchanged protocol `src/periphery/Bridge.sol` from pin `3c0cdf4955941a5c23639346932b55b33babebbd`, using an isolated deployer with nonce 0 on each chain. Both contracts are at `0xdbeAf9C179fEbb22b0A779623B4e070f2b388feD`. Initialized L1 with the current registry and rollup ID 0, and L2 with EEZL2 and rollup ID 1. Verified deployed runtime bytes exactly match the compiled artifact, and verified both manager and rollup bindings.

| Chain | Manager | Operation | Transaction |
| --- | --- | --- | --- |
| 10200 | `0x0ca34c68324dd5ef4bd6813af661d7817e4793b8` | fund | `0x34789d08dc3269e2fdf7f460168836572c2f8ba7d1c841b3d6c4e16e63af5638` |
| 10200 | `0x0ca34c68324dd5ef4bd6813af661d7817e4793b8` | deploy | `0x09aa03ae0bba7a18d62762d2bfff8b683065481d50a3f52922f3fabb4b689eb4` |
| 10200 | `0x0ca34c68324dd5ef4bd6813af661d7817e4793b8` | initialize | `0x35ed38f312788eeebf56516e86e4be5504eec6230e918e06b4f1f40b222c1958` |
| 6291 | `0x4200000000000000000000000000000000000007` | fund | `0x5ecb25d8d3535ba7ee354f27bbe526639170160cbf06a91da763399db0c00d6f` |
| 6291 | `0x4200000000000000000000000000000000000007` | deploy | `0x0d31ba21b08ddc4626226249db21957d302593c3b5267d6484d24a9c186c388b` |
| 6291 | `0x4200000000000000000000000000000000000007` | initialize | `0xdbcc93d68c4934001009f359e909e19df0409a19cefa57c1cabfd959562643ec` |

Updated only the UI bridge configuration and recreated the UI with its existing image. Updated its local Compose template and verification bindings so workload addresses are not reintroduced. Preserved workload deployment bindings, Composer settings, databases, gateway, monitor, and Grafana. The deployer key remains private on the deployment host.

Validation: 21 bridge unit tests and 3 bridge integration tests passed. All seven existing browser readiness scenarios passed with mocked RPCs and zero submitted transactions. A real Chromium session at the public bridge URL observed successful L1 and L2 `manager()` responses, no retry/missing-contract warning, and no page errors. Standard gas estimates for cross-chain transfers reverted on both fronts; they do not establish delivery. No live user bridge transfer was submitted by this repair. Canonical cross-chain transfer verification remains separate from the repaired readiness/configuration checks.

Local deployment receipts, configuration backups, browser report/screenshot, and failed estimation results are retained under `.runtime/chiado-observability-20260918/validation/ui-bridge-fix-20260922`. See the [wallet bridge runbook](../../runbooks/chiado-deployment.md#optional-wallet-bridge-ui) for configuration and diagnostic guidance.
