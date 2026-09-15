# Chiado cross-chain packing stress test

The controlled live test submitted **1,344 requests**: 672 inbound and 672
outbound mutable `setValue` calls. All settled successfully. The largest
confirmed Sync contained **33 cross-chain source requests**, exceeding the
previously recorded live peak of 30. This is the measured peak for this workload
and runtime, not a universal transaction maximum.

Traffic ran on 2026-09-14 from 22:10:47 UTC until the last wave verified at
22:27:43 UTC. Independent validation finished afterwards. Existing
background traffic continued; test signatures are tracked separately. No
runtime setting, contract, or live database changed, and no funding transfer or
node restart was required.

## Workload and observed limits

| Wave | Inbound / outbound | Source gas limit | Largest Sync carrying wave requests | Median / maximum settlement latency |
| --- | --- | --- | --- | --- |
| Mixed 64 | 32 / 32 | 600,000 | 29 | 17.0 / 27.0 seconds |
| Mixed 256 | 128 / 128 | 600,000 | 31 | 68.4 / 138.0 seconds |
| Mixed 512 | 256 / 256 | 600,000 | 32 | 275.3 / 444.5 seconds |
| Outbound 256 | 0 / 256 | 600,000 | 31 | 100.4 / 125.1 seconds |
| Inbound 256 | 256 / 0 | 150,000 | 33 | 60.6 / 105.1 seconds |

Latency is canonical L1 settlement-block time minus source acceptance time,
not the time a provisional receipt first appeared or the verifier finished.
Sync counts include background requests where present. The first two 33-request
Syncs contain only this test's requests. Source requests differ from L2 body
transactions, which also contain generated system transactions.

The final wave uses 150,000 gas because earlier inbound receipts measured
86,260 gas. This changes the test sender's declared limit, retaining substantial
execution headroom; it does not relax composer or network checks. The initial
600,000 limit reserves more companion gas in the complete L1 bundle.

The held pool reached **573 requests**, including background traffic. Automatic
admission selected as many as 46 groups and accepted at most 33. The measured
window records 20 post-transaction gas cuts, six complete-bundle gas cuts, and
three preparation-time cuts. Excess requests stayed queued and eventually
settled.

At the first 33-request cut, the conservative post gas projection was
13,011,944 against a 13,381,013 planning budget. The next entry would exceed
that budget. Actual post execution used 6,594,440 gas; the 33 inbound companions
used another 2,846,580 gas. The complete native carrier used one blob.
Thus the gas estimate, rather than exhaustion of Chiado's two-blob
allowance, stopped admission. Improving that estimate is a separate optimization
requiring preservation of complete ordered execution and first-use costs.

The first peak is independently authenticated at Sync **87,385**, L1 block
**23,059,384**, post
[`0x39b7d863…036e20`](https://gnosis-chiado.blockscout.com/tx/0x39b7d8632d5a396df4cb424821af3ed8657fcb8293476ac703a1aeb11b036e20).
Three further confirmed Syncs also carried 33 requests.

## Correctness and retry observations

- All 1,344 source receipts succeeded and identify canonical transactions.
  Every inbound source consumed its execution; outbound receipts are below the
  confirmed L2 head. Source nonce advances are exactly 672 on each chain, and
  the final contract values match the ordered calls.
- Native KZG and semantic verification authenticated 159 posts and 159 blobs
  covering all 1,060 L2 blocks from 86,451 through 87,510 without gaps or duplicate
  covered blocks. Each test source occurs exactly once in canonical semantic
  data. The 6,296 unique L2 body transactions also include system/background work.
- An independent follower resumed its previously L1-derived database at 84,190,
  using no composer state overlay. It reconstructed the range and matched the
  live hash and state root at 87,510. Its root is
  `0x715aace5991be71c99f4d46a163691ae3ff12b77819ef7a97eebe0bf23222c48`.
- The measured window contains 17 canonical submission backoffs and five
  proof-cutoff errors. Requests were retained and retried; this is not an
  error-free latency result. Additional background proof-cutoff errors occurred
  during subsequent verification.
- The original verifier retained first-observed provisional receipts and stopped
  when 14 outbound receipts changed after an excluded attempt. Canonical rereads
  verified the same signatures successfully in later Syncs. The verifier now
  waits for the confirmed L2 head and retains provisional changes as evidence.
  Later waves observed 25 and 29 such changes. They are not duplicate canonical
  execution, and were not counted as final before confirmation.

At the 22:35:31 UTC health check, L2 was 87,936 and confirmed through 87,930,
with advancing heads and no pending nonce from the test account. Background
cross-chain traffic remained: the last observed drain had 79 queued requests.
The network was progressing, but occasional deadline retries continued. The
test does not establish zero backlog or guaranteed latency under arbitrary load.

## Runtime and reproduction evidence

The unchanged composer image is `eez-node:chiado-da-guard-20260914`, image ID
`sha256:3dbf236f8a1e4c8e7f4557e219d4c868dd11bcdd63149d3c006e38e2bf4e3580`.
The previously frozen composer executable SHA-256 is
`131ed913c14ecc89be578713bff3f2ea287b9f421b90b2ddeb42f7bb12d2cbba`;
the independently executed follower SHA-256 is
`3196bd2bdd1b7d1d61d13f9ac51175b8621b0498af436c20100e9baf6b8889ef`.

The active policy remains `always` for pure L2 activity, general consolidation
after 1,800 L2 blocks, fullness off, automatic cross-chain admission, no operator
blob/gas cap, and a 500-gwei post execution-price ceiling. The network and final
ordered preflight still enforce their own limits.

Local evidence is under `artifacts/chiado-crosschain-stress-20260914/`:
`result.json` indexes hashed evidence; phase manifests record source signatures
and receipt outcomes; `running-summary.json` freezes the measured window;
`post-verification.json`, `source-verification.json`, and
`independent-follower.json` record the three independent verification layers.
The existing native `verify_blob_evidence` executable authenticated the blobs.
`final-health.json` retains the final queue and retry observations.

`stress.cjs` signs bounded waves before broadcast and journals their identities.
`remaining-phases.py` advances only after the preceding wave verifies.
`finish-validation.py` runs native blob authentication, canonical source checks,
and isolated follower comparison. Syntax checks passed for the Node/Python
helpers. Runtime production code was not modified by this test.
