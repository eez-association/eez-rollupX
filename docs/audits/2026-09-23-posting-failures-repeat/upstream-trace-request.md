# Prepared upstream trace request

This text has not been sent. Times are UTC on 2026-09-23, Chiado chain 10200.
We need per-bundle simulation/insertion results for two posting attempts. The
public logs show aggregate counts but do not identify the failed bundle.

## Case 1: setup candidate rejects its only bundle

- Execution block: 23193925; beacon slot: 24956256; target: 15:33:00.
- Parent: `0xfefb1ee5787b8e3bf61aeeb90ba874729b4d666f72294364e238dd6b98f4b7ab`.
- Postbatch transaction: `0x6bbcc109059d04f84efc2baaea6fd6f078bd15e5610a7827d4f183ded482a9e2`.
- Composer dispatched at 15:32:58.741399; nearby builder input warning at
  15:32:58.815625 says the bundle hash is absent.
- Payload ID: 77315480; candidate ID: 370022; candidate hash:
  `0x0784e971c8dcee21bdfd1c356a74c352780d6ce614e268da80d01f234bf69195`.
- At 15:32:58.848878 this candidate has one considered/failed bundle and zero
  included bundles. On shutdown, the sole received bundle is counted simulated OK.
- Local exact-target preflight: 113,305 gas used; signed gas allowance 135,966.
  Non-bundle candidate gas used: 16,886,343; block limit: 17,000,000.

Please provide the bundle's internal order ID and the candidate insertion error,
including failing transaction hash, error/revert bytes, insertion position,
remaining gas before insertion and sender nonce/state at that point. That small
record should distinguish gas capacity from state conflict or invalid input.
If failure occurred after simulation, include which earlier candidate
transactions changed the relevant state. We do not need a database copy.

## Case 2: replacement after skipped slot produces no timely bundle bid

- Execution block: 23194052; beacon slot: 24956396; target: 15:44:40.
- Parent: `0x08d74bebf9d9b2353f4ecc4400a1888ee03f2c860b238ff0906791e98a86c90c`.
- Replacement postbatch transaction:
  `0x921da32e1b1726ac88d6c979bf15169eb0e0ba0227867cbc0b3ee6307f10a8e5`.
- Old postbatch for skipped 15:44:35 slot:
  `0x65fe62c1ebdbf5dce4c9a38332d26b3378e7661f6e9028557e94e046e8ec0333`.
- Old/new payload IDs: 77315894 / 77315897, same execution height and parent.
- Replacement dispatched at 15:44:38.407334; nearby builder input warning at
  15:44:38.469301 says the bundle hash is absent.
- Relay header request at target +124 ms finds no bid. First submitted candidate
  at +872.889 ms has no bundle (ID 371389).
- Higher-value unfinished candidates 371390 and 371391 are chosen at
  +849.721 ms and +924.241 ms but have no subsequent submission in public logs.
- Local exact-target replacement preflight: 4,389,089 gas used; signed allowance
  5,652,827. Old postbatch preflight used 2,580,700 gas.

Please provide the separate old/replacement order IDs and per-order queue,
simulation-start/end and insertion results for payload 77315897. Also provide
whether candidates 371390/371391 contain the replacement transaction and their
finalization/cancellation reason. We need to distinguish rejected replacement
input from a valid replacement delayed behind old-slot work. Aggregate two
received/one simulated counters cannot make that distinction.

If these historical per-order records are unavailable, please say so. A future
controlled reproduction needs only these targeted records plus the submitted
bundle body and blob sidecars for exact parent/timestamp replay; no database copy
or broader log-level change is requested here.
