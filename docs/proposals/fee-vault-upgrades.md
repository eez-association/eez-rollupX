# Upgradeable fee treasury and explorer accounting

| Field | Value |
| --- | --- |
| Design status | User accepted upgradeability, withdrawal accounting and two-step recipient changes on 2026-10-26, and confirmed deployment on a new network. Implemented and locally validated; [evidence and deployment limits](../audits/2026-10-26-fee-vault-upgrades/README.md). |
| Source baseline | `641e768f2e6605f7563c808a00937d374e98e8fb`, dependent on PR #102; isolated branch `codex/fee-vault-upgradeable` |
| Protocol baseline | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |

## Scope and requirements

The existing fee execution rule remains unchanged: ordinary transactions credit
the authenticated collector balance without invoking its code. Genesis installs
the treasury behind OpenZeppelin's pinned transparent proxy, with atomic
initialization and a locked implementation. This follows the manager's proxy
administration pattern, while supplying the initialization data the treasury needs.

| ID | Behavior | Acceptance |
| --- | --- | --- |
| F1 | The fixed collector is a proxy; only its ProxyAdmin owner can upgrade. | Reject unauthorized upgrades; preserve balance, owner, recipient, pending changes, threshold and total withdrawals across a compatible upgrade. |
| F2 | Initialization cannot be repeated or performed directly on the implementation. | Constructor/initializer rejection tests; exact genesis code and storage checks. |
| F3 | The treasury owner proposes/cancels a recipient; only that recipient accepts. | Old recipient remains active until acceptance; superseded/cancelled proposals cannot accept. Contract recipients must support calling acceptance. |
| F4 | Anyone can withdraw the whole balance to the active recipient; `totalWithdrawn` records successful payments only. | Exact payments, counter and events; empty/failed/reentrant payments; rollback. This counter includes donated funds, not only fees. |
| F5 | Owner controls a minimum withdrawal in wei, initially zero. | Boundary and authorization tests; zero still rejects an empty vault. |
| F6 | Genesis authenticates proxy, implementation, ProxyAdmin and initial storage. | Regenerate all fixtures, exact-byte checks and independent state-root computation. |
| F7 | Followers replay recipient changes, upgrades, withdrawals and their events from published data. | Existing treasury source-process test extended for the new ABI and accounting. |
| F8 | Explorer does not label collected base fees as burned. | Configure the installed frontend's existing hidden-field options for blocks and transactions; document the unchanged backend API limitation. |

## Authority and events

`EEZ_L2_TREASURY_OWNER` initially owns both the treasury and its dedicated
ProxyAdmin. These are separate authorities thereafter: treasury ownership uses
two-step transfer; the standard ProxyAdmin has its own ownership transfer.
Operators must inspect both. A multisig can hold either authority without new
contract features. Upgrade authority can replace all treasury logic and must be
treated as control of its funds. Compatible upgrades must retain the storage
layout; rollback is possible only when the previous implementation understands
the current storage and no irreversible migration occurred.

The treasury emits proposal, cancellation, accepted-recipient, threshold and
withdrawal events. The proxy emits the standard ERC-1967 upgrade events. Protocol
fee credits cannot emit contract events because no call occurs. A `receive()`
event would capture only explicit transfers and cannot serve as a fee oracle.

## Explorer scope

Hiding the incorrect burned-fee fields uses upstream configuration; no Blockscout
fork is required. Displaying actual fee income and historical balances requires
an indexer/frontend extension, or a separate linked dashboard. Frontend 2.3.5 also supports a configured
[address widget](https://github.com/blockscout/frontend/blob/v2.3.5/docs/ENVS.md#address-3rd-party-widget)
showing an API-provided value and a link to the detailed view, without changing
Blockscout source. This does not replace its native balance-history API. Compute credits
from ordinary receipt gas and the block base fee, excluding native system gas;
separate withdrawals and donations, retain block hashes and handle reorgs. Never
fabricate transfers or relabel the generic backend value without checking its
calculation. Full fee-income indexing is follow-up work, not supplied by source
verification, this contract's events, or the hidden-field configuration.

## Compatibility and deployment

This changes fresh-network genesis code/storage and its authenticated state root.
It does not change execution rules, the core protocol pin or `prove.v2`.
The deployment target is a new network: the proxy is present from its first
block, and every role uses the same newly generated genesis/profile. No old-vault
migration, legacy ABI or mixed-genesis compatibility is provided. Starting this
network requires fresh datadirs; this change has not deployed or reset a network.

Once present from genesis, compatible treasury upgrades are ordinary L2
transactions, executable by followers and the witness-backed prover without a
hard fork. Publication of source/ABI and the active implementation remains an
explorer operation. Deployment and live upgrades require their own artifact
verification and authorization; local tests do not establish a live upgrade.
