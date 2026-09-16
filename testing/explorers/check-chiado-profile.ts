import assert from "node:assert/strict";
import { getChain } from "./packages/chains/src";
import { formatWei, prettyFormatWei } from "./packages/eth-format";
import { env } from "./packages/env";
import { networkSchema, chainIdSchema } from "./apps/web/src/env";
import { getChainIdByName } from "./apps/web/src/utils/web3";

const chiado = getChain(10200);
assert.equal(chiado, getChain("chiado"));
assert.equal(chiado.slotsPerEpoch, 16);
assert.equal(chiado.getEpochBySlot(24_788_218), Math.floor(24_788_218 / 16));
for (const timestamp of [1706724940, 1741254220, 1773653580, 1789380000]) {
  const { blobParams } = chiado.getActiveForkByDate(timestamp * 1000);
  assert.equal(blobParams.maxBlobsPerBlock, 2);
  assert.equal(blobParams.targetBlobsPerBlock, 1);
  assert.equal(blobParams.minBlobBaseFee, 1_000_000_000n);
  assert.equal(blobParams.blobBaseFeeUpdateFraction, 1_112_826n);
  assert.equal(blobParams.blobGasLimit, blobParams.gasPerBlob * 2n);
  assert.equal(blobParams.targetBlobGasPerBlock, blobParams.gasPerBlob);
}
assert.equal(chiado.getFork("pectra")?.activationSlot, 948224 * 16);
assert.equal(getChain(1).getFork("dencun")?.blobParams.maxBlobsPerBlock, 6);
assert.equal(getChain(1).getFork("pectra")?.blobParams.maxBlobsPerBlock, 9);
assert.equal(getChain(1).getFork("pectra")?.blobParams.minBlobBaseFee, 1n);
assert.equal(networkSchema.parse("chiado"), "chiado");
assert.equal(chainIdSchema.parse("10200"), "10200");
assert.equal(getChainIdByName("chiado"), 10200);
assert.equal(env.STATS_SYNCER_LOWEST_SLOT, 24788218);
assert.equal(formatWei(1_000_000_000_000_000_001n, { toUnit: "ether" }), "1.000000000000000001 xDAI");
assert.equal(prettyFormatWei(1_000_000_000_000_000_000n, { toUnit: "ether" }), "1 xDAI");
console.log("Chiado chain, fork capacity, fee floor, epoch, routing, statistics floor, and exact currency formatting checks passed");
