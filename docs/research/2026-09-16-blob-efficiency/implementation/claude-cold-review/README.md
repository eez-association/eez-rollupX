# Cold signer implementation review checkpoint

This is an external-model review with independent local disposition, not a
security certification or release approval. The user authorized the disclosed
41-file implementation snapshot, then authorized the two-file follow-up after
automatic approval review requested separate consent. Both invocations disabled
Claude tools, MCP servers, hooks and session persistence. No credentials, operator
runtime configuration or witness dumps were exported.

Inputs and outputs: [manifest](input-manifest.json), [first prompt](prompt.txt),
[first review](round-1.md), [follow-up prompt](round-2-prompt.txt),
[follow-up review](round-2.md), [CLI metadata](status.json).
The selected CLI model was `opus[1m]`; the result metadata reports the actual
model usage. These artifacts preserve the exact reviewed snapshot scope, not
subsequent unreviewed changes.

| Finding | Independent disposition | Evidence / remaining work |
|---|---|---|
| F1: shipping v3 effect/fork coverage incomplete | Accepted; partially closed | Current real RPC tests include independently encoded typed transfers/forks plus captured mixed effects and caught revert. An independent reverted ordinary CREATE fixture now passes as well. EIP-7702 successful clearing/ignored-authorization fixtures now pass; executing delegated code, recursive callback/static and nonempty requests remain. The reviewer has not read the latest test bodies. |
| F2: attestable type constructed before observation replay | Fixed | Inner job returns `RecomputedPublicInputsHash`; only the full job constructs `AttestablePublicInputsHash` after replay and cancellation/deadline checks. The new forged native lookup test changes both claim copies, reaches `observation_replay`, rejects via RPC and records zero signing calls. Claude confirmed the code repair before this last test was added. |
| F3: reconstruction errors lose diagnostics | Fixed | Stateless reconstruction now returns a block-number/reason rejection immediately. Existing RPC tracing logs the pipeline error at WARN. Complete current signer suite passes. |
| Claimed BPO selection failure | Rejected and withdrawn by reviewer | Pinned alloy-evm selects Osaka for BPO timestamps, not a separate unsupported execution spec. Real BPO acceptance test passes. Blob schedule boundary behavior deserves its own additional case; the withdrawn speculation is not evidence of a bug. |
| F4: larger windows occupy the one proving worker | Accepted as a measurement/availability concern | Largest native cold/deadline/restart measurements remain open. Existing empty-state fixture timings are not a deployment envelope. |
| Follow-up: malicious high-gas block can outlive the request deadline | Existing availability concern, outside this format change | Supplied parent hash/witness authentication does not establish canonical ancestry before execution. Thus a single block could already cause this occupancy under old limits. Consensus execution is non-interruptible within a block; cancellation is cooperative between units. No incorrect signature follows from this finding. A separate operational gas quota may help, but is not silently introduced as a consensus restriction or treated as a newly authorized requirement. It would require a deployment-derived bound, capability propagation and explicit compatibility/error policy. |

The follow-up's assertion that a gas ceiling is now a completion requirement is
not adopted merely because the reviewer listed it. It is a proposed operational
hardening measure for an existing exposure, not an identified new authorization
bypass. Nor is an expensive OOG benchmark needed to re-prove the code's existing
cooperative-cancellation model during this change. Actual legitimate long-window
resource/timing/recovery requirements remain mandatory and can still demonstrate
a need for cache/preparation or explicit operational bounds.

No review finding waives any row in the coverage ledger. Repeat review against
the completed implementation, especially if preparation/cache/capability paths
are added. The current 301 passing signer tests and focused native-request
regressions support this checkpoint only; see [gate map](../signer-gates.md).
