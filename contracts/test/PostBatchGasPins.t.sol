// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test, console} from "forge-std/Test.sol";

import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {
    ExecutionEntry,
    ExpectedL1ToL2Call,
    ExpectedRootPerRollup,
    L2ToL1Call,
    ProofSystemBatchPerVerificationEntries,
    RollupIdWithProofSystems,
    RollupUpdate,
    StaticExecutionEntry
} from "eez-core-protocol/src/interfaces/IEEZ.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";

import {ECDSAProofSystem} from "../src/ECDSAProofSystem.sol";

/// Empty body, so the per-entry marginal prices EEZ.sol's own bookkeeping
/// instead of the callee's work.
contract EmptyTarget {
    function ping() external {}
}

/// A fixed-size nonzero result exercises return copying and rolling-hash work
/// independently of input length. Actual callee execution remains charged by
/// the composer's separate target-gas term.
contract BoundedReturnTarget {
    uint256 private immutable resultBytes;

    constructor(uint256 resultBytes_) {
        resultBytes = resultBytes_;
    }

    fallback() external {
        bytes memory result = new bytes(resultBytes);
        for (uint256 i = 0; i < result.length; i++) {
            result[i] = 0xa5;
        }
        assembly {
            return(add(result, 0x20), mload(result))
        }
    }
}

/// Measures from a caller frame so intrinsic and calldata gas stay out; the
/// Rust side prices those with exact EIP-7623 arithmetic.
///
/// Raw `call`, not a typed one: re-encoding a 51-entry struct would bill the
/// ABI encoder to the per-entry marginal.
contract PostBatchGasProbe {
    function measure(address eez, bytes calldata postCall) external returns (uint256 gasUsed) {
        uint256 start = gasleft();
        (bool ok, bytes memory revertData) = eez.call(postCall);
        gasUsed = start - gasleft();
        if (!ok) {
            assembly {
                revert(add(revertData, 0x20), mload(revertData))
            }
        }
    }
}

/// @notice Pins the two gas constants the composer's drain projects with, so CI
///         fails as soon as EEZ.sol's real cost drifts past a pin.
///
/// @dev Measures `postAndVerifyBatch` on a composer-shaped batch: one leading
///      anchor entry plus N outbound entries, each with a chained state delta.
///
///      Several rung sizes catch nonlinearity from memory growth; the pins must
///      cover the base and the WORST per-entry marginal on the ladder.
contract PostBatchGasPinsTest is Test {
    /// Mirrors `POSTBATCH_BASE_GAS_PIN` in `crates/eez-composer/src/composer.rs`.
    /// Measured 139_129; pinned at measured x 1.10 rounded up.
    uint256 private constant POSTBATCH_BASE_GAS_PIN = 160_000;

    /// Mirrors `POSTBATCH_ENTRY_GAS_PIN` in `crates/eez-composer/src/composer.rs`.
    /// Measured 333_730 (worst rung); pinned at measured x 1.10 rounded up.
    uint256 private constant POSTBATCH_ENTRY_GAS_PIN = 370_000;

    // Restricted current-entry shapes only. Runtime also charges target gas,
    // exact transaction/ABI gas, prior-queue cleanup and its configured margin.
    uint256 private constant SHORT_DEFERRED_GAS_PIN = 230_000;
    uint256 private constant WORD_DEFERRED_GAS_PIN = 250_000;
    uint256 private constant BOUNDED_DEFERRED_GAS_PIN = 270_000;
    uint256 private constant EXISTING_PROXY_GAS_PIN = 70_000;
    uint256 private constant SHORT_QUEUE_CLEANUP_GAS_PIN = 55_000;
    uint256 private constant WORD_QUEUE_CLEANUP_GAS_PIN = 60_000;
    uint256 private constant SIMPLE_QUEUE_CLEANUP_GAS_PIN = 65_000;

    uint64 private constant ROLLUP_ID = 1;
    uint256 private constant PROVER_KEY = 0xA11CE;
    bytes32 private constant GENESIS_ROOT = keccak256("postbatch gas pin genesis");

    PostBatchGasProbe private probe;

    struct Rung {
        uint256 entries;
        uint256 callsPerEntry;
        bool preCreateProxies;
        bool repeatSource;
        uint256 blobs;
    }

    /// Accounts touched while building a rung; all cooled before the measured
    /// call, because EIP-2929 warm access is far cheaper and a real batch is cold.
    address[] private touched;

    function setUp() external {
        probe = new PostBatchGasProbe();
    }

    function testPostBatchGasPinsCoverTheLadder() external {
        uint256[4] memory ladder = [uint256(1), 2, 10, 50];
        uint256 base = _rung(0, true);
        console.log("base (leading immediate only):", base);

        uint256 worstMarginal;
        // A sender's first outbound settlement also CREATE2-deploys its source
        // proxy in the batch, which costs more than the entry's own bookkeeping.
        //
        // The compose-time probe deploys that proxy in a separate frame, so its
        // gas never reaches `target_gas` and the entry pin must absorb it.
        //
        // Both ladders run, so the pin has to hold for the cheap established
        // sender and for the costly first-ever one.
        for (uint256 proxies = 0; proxies < 2; proxies++) {
            bool preCreated = proxies == 0;
            console.log(preCreated ? "-- established source proxies" : "-- first-ever source proxies");
            for (uint256 i = 0; i < ladder.length; i++) {
                uint256 gasUsed = _rung(ladder[i], preCreated);
                uint256 marginal = (gasUsed - base) / ladder[i];
                console.log("outbound entries:", ladder[i]);
                console.log("  gas:", gasUsed);
                console.log("  marginal per entry:", marginal);
                if (marginal > worstMarginal) worstMarginal = marginal;
            }
        }

        console.log("worst marginal:", worstMarginal);
        // The slack covers unmeasured entry hashing/memory growth and solc drift.
        // Native DA is in blobs: only their hashes, not their contents, reach EEZ.
        //
        // The inner target call sits inside the marginal, and at runtime the
        // composer also charges its probed `target_gas`, so it is paid twice.
        assertLe(base, POSTBATCH_BASE_GAS_PIN, "observed base exceeds POSTBATCH_BASE_GAS_PIN");
        assertLe(worstMarginal, POSTBATCH_ENTRY_GAS_PIN, "observed marginal exceeds POSTBATCH_ENTRY_GAS_PIN");
    }

    function testLeadingAnchorBaseCoversColdNativeBlobReferencesAndLaterSettlements() external {
        // Above-six counts are fixtures, not network limits. Foundry supplies
        // hashes only; native sidecars and KZG remain the Rust tests' concern.
        uint256[6] memory blobCounts = [uint256(0), 1, 2, 6, 10, 64];
        uint256 baseCalldataBytes;
        for (uint256 i = 0; i < blobCounts.length; i++) {
            delete touched;
            (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
            Rung memory shape = Rung(0, 0, false, false, blobCounts[i]);
            ProofSystemBatchPerVerificationEntries memory batch = _batch(eez, proofSystem, address(0), shape);
            assertEq(batch.entries.length, 1, "one leading anchor");
            assertEq(batch.entries[0].rollupUpdates.length, 1, "one own-rollup update");
            assertEq(batch.entries[0].l2ToL1Calls.length, 0, "anchor has no calls");
            assertEq(batch.callData.length, 0, "native DA is not calldata");
            batch.proofs[0] = _proof(_publicInputsHash(batch));
            uint256 calldataBytes = abi.encodeCall(EEZ.postAndVerifyBatch, (batch)).length;
            if (i == 0) baseCalldataBytes = calldataBytes;
            assertEq(calldataBytes, baseCalldataBytes + 32 * blobCounts[i], "only blob indexes extend calldata");
            uint256 first = _measure(eez, batch, batch.entries[0].rollupUpdates[0].newRoot);
            assertLe(first, POSTBATCH_BASE_GAS_PIN, "first anchor exceeds the base pin");

            vm.roll(block.number + 1);
            batch = _batch(eez, proofSystem, address(0), shape);
            uint256 later = _measure(eez, batch, batch.entries[0].rollupUpdates[0].newRoot);
            assertLe(later, POSTBATCH_BASE_GAS_PIN, "later cold anchor exceeds the base pin");
            console.log("anchor blobs / first / later:", blobCounts[i], first, later);
        }
    }

    function testRepeatedSourcesStillFitTheEntryPinWithoutRepeatedDeployment() external {
        uint256 entries = 10;
        uint256 newDistinct = _rung(Rung(entries, 1, false, false, 1));
        uint256 newRepeated = _rung(Rung(entries, 1, false, true, 1));
        uint256 oldDistinct = _rung(Rung(entries, 1, true, false, 1));
        uint256 oldRepeated = _rung(Rung(entries, 1, true, true, 1));
        assertLe(newDistinct, POSTBATCH_BASE_GAS_PIN + entries * POSTBATCH_ENTRY_GAS_PIN);
        assertLe(newRepeated, POSTBATCH_BASE_GAS_PIN + entries * POSTBATCH_ENTRY_GAS_PIN);
        assertLe(oldDistinct, POSTBATCH_BASE_GAS_PIN + entries * POSTBATCH_ENTRY_GAS_PIN);
        assertLe(oldRepeated, POSTBATCH_BASE_GAS_PIN + entries * POSTBATCH_ENTRY_GAS_PIN);
        assertGt(newDistinct, newRepeated, "distinct new sources deploy more proxies");
        assertGt(oldDistinct, oldRepeated, "distinct existing sources pay cold account accesses");
        assertGt(newRepeated, oldRepeated, "first use still pays for a proxy");
        console.log("ten sources, new distinct / repeated:", newDistinct, newRepeated);
        console.log("ten sources, existing distinct / repeated:", oldDistinct, oldRepeated);
    }

    function testProxyCreationWorkDependsOnCallsWithinOneEntry() external {
        uint256 oneNew = _rung(Rung(1, 1, false, false, 1));
        uint256 oneOld = _rung(Rung(1, 1, true, false, 1));
        uint256 twoNew = _rung(Rung(1, 2, false, false, 1));
        uint256 twoOld = _rung(Rung(1, 2, true, false, 1));
        assertGt(twoNew - twoOld, oneNew - oneOld, "one entry can require multiple proxy deployments");
        console.log("one entry, one new / existing proxy:", oneNew, oneOld);
        console.log("one entry, two new / existing proxies:", twoNew, twoOld);
    }

    function testMultiProxyEntryRejectsInsufficientGasAndCompletesWithHeadroom() external {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        EmptyTarget target = new EmptyTarget();
        touched.push(address(target));
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(target), Rung(1, 2, false, false, 1));
        batch.proofs[0] = _proof(_publicInputsHash(batch));
        bytes memory postCall = abi.encodeCall(EEZ.postAndVerifyBatch, (batch));
        bytes32 expectedRoot = batch.entries[1].rollupUpdates[0].newRoot;
        for (uint256 i = 0; i < touched.length; i++) {
            vm.cool(touched[i]);
        }
        // These are forwarded execution limits, excluding transaction intrinsic
        // and calldata gas. A pin for one possible creation cannot fund two.
        uint256 tooSmall = POSTBATCH_BASE_GAS_PIN + POSTBATCH_ENTRY_GAS_PIN;
        (bool underfunded,) = address(eez).call{gas: tooSmall}(postCall);
        assertFalse(underfunded, "insufficient CREATE2 gas rejects this fixture");
        (, bytes32 unchangedRoot,) = eez.rollups(ROLLUP_ID);
        assertEq(unchangedRoot, GENESIS_ROOT, "failed post also rolls back the anchor");
        for (uint256 i = 0; i < touched.length; i++) {
            vm.cool(touched[i]);
        }
        (bool funded,) = address(eez).call{gas: 750_000}(postCall);
        assertTrue(funded, "sufficient forwarding headroom completes the same batch");
        (, bytes32 actualRoot,) = eez.rollups(ROLLUP_ID);
        assertEq(actualRoot, expectedRoot, "complete root is required in addition to outer success");
        console.log("two new proxies, rejected / successful forwarded gas:", tooSmall, uint256(750_000));
    }

    function testForcedRevertSpanCanDeployTheSameProxyAgain() external {
        uint256 ordinary = _rung(Rung(1, 2, false, true, 1));
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        EmptyTarget target = new EmptyTarget();
        touched.push(address(target));
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(target), Rung(1, 2, false, true, 1));
        // The successful first call runs in a nested frame that always reverts.
        // Its rolling-hash result survives, but its new proxy does not. The
        // second call needs a fresh CREATE2 for the same source address.
        batch.entries[1].l2ToL1Calls[0].revertNextNCalls = 1;
        uint256 reverted = _measure(eez, batch, batch.entries[1].rollupUpdates[0].newRoot);
        assertGt(reverted, ordinary, "rolled-back creation cannot count as an existing proxy");
        assertGt(eez.computeCrossChainProxyAddress(address(0x5000), ROLLUP_ID).code.length, 0);
        console.log("same proxy twice, ordinary / forced revert:", ordinary, reverted);
    }

    function testDeferredEntryPaysQueueStorageWithoutInlineExecution() external {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(0), Rung(1, 0, false, false, 1));
        batch.entries[1].proxyEntryHash = bytes32(uint256(1));
        batch.immediateEntryCount = 1;
        uint256 gasUsed = _measure(eez, batch, batch.entries[0].rollupUpdates[0].newRoot);
        assertEq(eez.queueLength(ROLLUP_ID), 1, "deferred entry is stored");
        assertEq(eez.entryQueueIndex(ROLLUP_ID), 0, "no companion has consumed it");
        assertLe(gasUsed, POSTBATCH_BASE_GAS_PIN + POSTBATCH_ENTRY_GAS_PIN);
        console.log("anchor plus one deferred entry:", gasUsed);
    }

    function testDeferredQueueStorageAndCleanupLadder() external {
        uint256[6] memory entries = [uint256(1), 2, 10, 35, 64, 127];
        uint256[7] memory resultSizes = [uint256(0), 31, 32, 33, 63, 64, 65];
        for (uint256 r = 0; r < resultSizes.length; r++) {
            for (uint256 n = 0; n < entries.length; n++) {
                (uint256 posted, uint256 cleared) = _deferredRung(entries[n], resultSizes[r], true);
                // 65 bytes needs a third payload slot and remains an explicit
                // out-of-bound control, not an extension of eligibility.
                if (resultSizes[r] <= 64) {
                    assertLe(posted, POSTBATCH_BASE_GAS_PIN + entries[n] * BOUNDED_DEFERRED_GAS_PIN);
                    assertLe(cleared, POSTBATCH_BASE_GAS_PIN + entries[n] * SIMPLE_QUEUE_CLEANUP_GAS_PIN);
                }
                if (resultSizes[r] < 32) {
                    assertLe(posted, POSTBATCH_BASE_GAS_PIN + entries[n] * SHORT_DEFERRED_GAS_PIN);
                    assertLe(cleared, POSTBATCH_BASE_GAS_PIN + entries[n] * SHORT_QUEUE_CLEANUP_GAS_PIN);
                } else if (resultSizes[r] == 32) {
                    assertLe(posted, POSTBATCH_BASE_GAS_PIN + entries[n] * WORD_DEFERRED_GAS_PIN);
                    assertLe(cleared, POSTBATCH_BASE_GAS_PIN + entries[n] * WORD_QUEUE_CLEANUP_GAS_PIN);
                }
                console.log("deferred entries / result bytes:", entries[n], resultSizes[r]);
                console.log("  post / later consumed-queue cleanup:", posted, cleared);
            }
        }
    }

    function testDeferredQueueCleanupAlsoRemovesUnconsumedEntries() external {
        (uint256 posted, uint256 cleared) = _deferredRung(35, 32, false);
        assertLe(posted, POSTBATCH_BASE_GAS_PIN + 35 * WORD_DEFERRED_GAS_PIN);
        assertLe(cleared, POSTBATCH_BASE_GAS_PIN + 35 * WORD_QUEUE_CLEANUP_GAS_PIN);
        assertLe(cleared, POSTBATCH_BASE_GAS_PIN + 35 * SIMPLE_QUEUE_CLEANUP_GAS_PIN);
        assertGt(cleared, POSTBATCH_BASE_GAS_PIN + POSTBATCH_ENTRY_GAS_PIN, "one old anchor pin cannot cover cleanup");
        console.log("35 unconsumed 32-byte entries, post / later cleanup:", posted, cleared);
        (posted, cleared) = _deferredRung(127, 64, false);
        assertLe(posted, POSTBATCH_BASE_GAS_PIN + 127 * BOUNDED_DEFERRED_GAS_PIN);
        assertLe(cleared, POSTBATCH_BASE_GAS_PIN + 127 * SIMPLE_QUEUE_CLEANUP_GAS_PIN);
        console.log("127 unconsumed 64-byte entries, post / later cleanup:", posted, cleared);
    }

    function testMixedDeferredQueueReservesAreAdditiveBeforeAndAfterConsumption() external {
        // Interleave packed bytes, one payload slot and two payload slots at
        // the measured queue bound. Every payload byte and every root is
        // nonzero; the reserve must cover cold storage without any refund.
        uint256[] memory resultSizes = new uint256[](127);
        for (uint256 i = 0; i < resultSizes.length; i++) {
            resultSizes[i] = i % 3 == 0 ? 31 : (i % 3 == 1 ? 32 : 64);
        }
        uint256 postReserve = POSTBATCH_BASE_GAS_PIN + 43 * SHORT_DEFERRED_GAS_PIN + 42 * WORD_DEFERRED_GAS_PIN + 42
            * BOUNDED_DEFERRED_GAS_PIN;
        uint256 cleanupReserve = POSTBATCH_BASE_GAS_PIN + 43 * SHORT_QUEUE_CLEANUP_GAS_PIN + 42
            * WORD_QUEUE_CLEANUP_GAS_PIN + 42 * SIMPLE_QUEUE_CLEANUP_GAS_PIN;
        for (uint256 consumed = 0; consumed < 2; consumed++) {
            (uint256 posted, uint256 cleared) = _deferredRung(resultSizes, consumed == 1);
            assertLe(posted, postReserve, "mixed queue creation exceeds the sum of result-size reserves");
            assertLe(cleared, cleanupReserve, "mixed queue cleanup exceeds the sum of result-size reserves");
            assertLe(posted, POSTBATCH_BASE_GAS_PIN + resultSizes.length * BOUNDED_DEFERRED_GAS_PIN);
            assertLe(cleared, POSTBATCH_BASE_GAS_PIN + resultSizes.length * SIMPLE_QUEUE_CLEANUP_GAS_PIN);
            console.log("mixed 127-entry queue, consumed / post / later cleanup:", consumed, posted, cleared);
        }
    }

    function testDeferredDynamicDataOutsideTheBoundNeedsFallback() external {
        (uint256 posted, uint256 cleared) = _deferredRung(1, 1024, false);
        assertGt(posted, POSTBATCH_BASE_GAS_PIN + BOUNDED_DEFERRED_GAS_PIN, "unbounded result needs extra storage gas");
        assertGt(cleared, POSTBATCH_BASE_GAS_PIN + SIMPLE_QUEUE_CLEANUP_GAS_PIN, "old dynamic data needs extra cleanup");
        console.log("out-of-bound 1024-byte result, post / later cleanup:", posted, cleared);
    }

    function testQueueStorageLayoutIncludesTheStaticQueue() external {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(0), Rung(0, 0, false, false, 10));
        ExpectedRootPerRollup[] memory roots = new ExpectedRootPerRollup[](1);
        roots[0] = ExpectedRootPerRollup({rollupId: ROLLUP_ID, root: batch.entries[0].rollupUpdates[0].newRoot});
        batch.staticEntries = new StaticExecutionEntry[](1);
        batch.staticEntries[0] = StaticExecutionEntry({
            expectedRoots: roots,
            proxyEntryHash: bytes32(uint256(1)),
            l2ToL1Calls: new L2ToL1Call[](0),
            rollingHash: bytes32(0),
            destinationRollupId: ROLLUP_ID,
            success: true,
            returnData: _nonzeroBytes(32)
        });
        _measure(eez, batch, batch.entries[0].rollupUpdates[0].newRoot);
        uint256 head = uint256(keccak256(abi.encode(ROLLUP_ID, uint256(3))));
        assertEq(uint256(vm.load(address(eez), bytes32(head + 1))), 0, "normal queue length at mapping head + 1");
        assertEq(uint256(vm.load(address(eez), bytes32(head + 2))), 1, "static queue length at mapping head + 2");
        assertEq(eez.queueLength(ROLLUP_ID), 0, "normal getter does not reveal the nonempty static queue");
    }

    function testExistingProxyDiscountRequiresRegistrationNotOnlyCode() external {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        EmptyTarget target = new EmptyTarget();
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(target), Rung(1, 1, false, false, 1));
        address proxy = eez.computeCrossChainProxyAddress(address(0x5000), ROLLUP_ID);
        vm.etch(proxy, hex"60006000f3");
        assertGt(proxy.code.length, 0, "code exists at the deterministic address");
        (bool registered,,) = eez.authorizedProxies(proxy);
        assertFalse(registered, "code alone is not the proxy registry predicate");
        batch.proofs[0] = _proof(_publicInputsHash(batch));
        (bool posted,) = address(eez).call{gas: 1_000_000}(abi.encodeCall(EEZ.postAndVerifyBatch, (batch)));
        (, bytes32 root,) = eez.rollups(ROLLUP_ID);
        assertNotEq(root, batch.entries[1].rollupUpdates[0].newRoot, "unregistered code cannot complete the entry");
        assertEq(root, posted ? batch.entries[0].rollupUpdates[0].newRoot : GENESIS_ROOT);
    }

    function testImmediateEntryResultMustBeEmptyEvenWhenTheTargetResultIsBounded() external {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        EmptyTarget target = new EmptyTarget();
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(target), Rung(1, 1, true, false, 1));
        batch.entries[1].returnData = _nonzeroBytes(32);
        batch.proofs[0] = _proof(_publicInputsHash(batch));
        vm.expectRevert(abi.encodeWithSelector(EEZ.L2TxEntryNotCanonical.selector, uint256(1)));
        eez.postAndVerifyBatch(batch);
        (, bytes32 root,) = eez.rollups(ROLLUP_ID);
        assertEq(root, GENESIS_ROOT, "noncanonical immediate result rejected before applying the anchor");
    }

    function testExistingProxyImmediateBookkeepingBoundaries() external {
        uint256[6] memory entries = [uint256(1), 2, 10, 35, 64, 127];
        uint256[4] memory inputs = [uint256(0), 4, 256, 257];
        uint256[6] memory results = [uint256(0), 32, 33, 63, 64, 65];
        uint256[4] memory ladderResults = [uint256(32), 63, 64, 65];
        for (uint256 n = 0; n < entries.length; n++) {
            for (uint256 r = 0; r < ladderResults.length; r++) {
                uint256 gasUsed = _boundedImmediateRung(entries[n], 256, ladderResults[r]);
                if (ladderResults[r] <= 64) {
                    assertLe(gasUsed, POSTBATCH_BASE_GAS_PIN + entries[n] * EXISTING_PROXY_GAS_PIN);
                }
                console.log("existing simple entries / input 256 / output bytes:", entries[n], ladderResults[r]);
                console.log("  gas:", gasUsed);
            }
        }
        for (uint256 i = 0; i < inputs.length; i++) {
            for (uint256 r = 0; r < results.length; r++) {
                uint256 gasUsed = _boundedImmediateRung(1, inputs[i], results[r]);
                if (inputs[i] <= 256 && results[r] <= 64) {
                    assertLe(gasUsed, POSTBATCH_BASE_GAS_PIN + EXISTING_PROXY_GAS_PIN);
                }
                console.log("existing input bytes / result bytes:", inputs[i], results[r]);
                console.log("  gas:", gasUsed);
            }
        }
    }

    function _boundedImmediateRung(
        uint256 entries,
        uint256 inputBytes,
        uint256 resultBytes
    )
        private
        returns (uint256 gasUsed)
    {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        BoundedReturnTarget target = new BoundedReturnTarget(resultBytes);
        touched.push(address(target));
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(target), Rung(entries, 1, true, false, 64));
        bytes memory result = _nonzeroBytes(resultBytes);
        for (uint256 i = 1; i < batch.entries.length; i++) {
            batch.entries[i].l2ToL1Calls[0].data = _nonzeroBytes(inputBytes);
            batch.entries[i].rollingHash =
                _rollingHashWithResult(eez, batch.entries[i].rollupUpdates, batch.entries[i].l2ToL1Calls, result);
        }
        return _measure(eez, batch, batch.entries[entries].rollupUpdates[0].newRoot);
    }

    function _deferredRung(
        uint256 entries,
        uint256 resultBytes,
        bool consume
    )
        private
        returns (uint256 posted, uint256 cleared)
    {
        uint256[] memory resultSizes = new uint256[](entries);
        for (uint256 i = 0; i < entries; i++) {
            resultSizes[i] = resultBytes;
        }
        return _deferredRung(resultSizes, consume);
    }

    function _deferredRung(
        uint256[] memory resultSizes,
        bool consume
    )
        private
        returns (uint256 posted, uint256 cleared)
    {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        address remote = address(0x9000);
        address proxy = eez.createCrossChainProxy(remote, ROLLUP_ID);
        touched.push(proxy);
        uint256 entries = resultSizes.length;
        ProofSystemBatchPerVerificationEntries memory batch =
            _batch(eez, proofSystem, address(0), Rung(entries, 0, false, false, 64));
        batch.immediateEntryCount = 1;
        // Reuse equal-sized fixture payloads as the uniform ladder does: these
        // caller-side allocations are outside the measured protocol call.
        bytes memory result = new bytes(0);
        for (uint256 i = 1; i < batch.entries.length; i++) {
            ExecutionEntry memory entry = batch.entries[i];
            entry.proxyEntryHash = eez.computeCrossChainCallHash(
                false, address(this), eez.MAINNET_ROLLUP_ID(), remote, ROLLUP_ID, 0, 0, abi.encode(i)
            );
            if (result.length != resultSizes[i - 1]) result = _nonzeroBytes(resultSizes[i - 1]);
            entry.returnData = result;
            bytes32 seed = keccak256(abi.encodePacked(bytes32(0), ROLLUP_ID, entry.rollupUpdates[0].currentRoot));
            entry.rollingHash = keccak256(abi.encodePacked(seed, entry.proxyEntryHash));
        }
        posted = _measure(eez, batch, batch.entries[0].rollupUpdates[0].newRoot);
        assertEq(eez.queueLength(ROLLUP_ID), entries, "all deferred entries stored");
        assertEq(eez.entryQueueIndex(ROLLUP_ID), 0, "posting did not execute deferred calls");
        _assertDeferredStorageLayout(eez, batch, 0);
        if (consume) {
            for (uint256 i = 1; i <= entries; i++) {
                (bool ok, bytes memory returned) = proxy.call(abi.encode(i));
                assertTrue(ok, "every companion must consume its matching deferred entry");
                assertEq(returned, batch.entries[i].returnData, "deferred dynamic result preserved");
            }
            (, bytes32 consumedRoot,) = eez.rollups(ROLLUP_ID);
            assertEq(consumedRoot, batch.entries[entries].rollupUpdates[0].newRoot, "all deferred roots applied");
            assertEq(eez.entryQueueIndex(ROLLUP_ID), entries, "all entries consumed");
            assertEq(eez.queueLength(ROLLUP_ID), entries, "consumption does not delete queue storage");
            _assertDeferredStorageLayout(eez, batch, entries);
        }
        vm.roll(block.number + 1);
        batch = _batch(eez, proofSystem, address(0), Rung(0, 0, false, false, 64));
        cleared = _measure(eez, batch, batch.entries[0].rollupUpdates[0].newRoot);
        assertEq(eez.queueLength(ROLLUP_ID), 0, "next post deletes the old queue");
        assertEq(eez.entryQueueIndex(ROLLUP_ID), 0, "next post resets the cursor");
        uint256 head = uint256(keccak256(abi.encode(ROLLUP_ID, uint256(3))));
        assertEq(uint256(vm.load(address(eez), bytes32(head))), uint64(block.number), "packed cursor reset");
        assertEq(vm.load(address(eez), bytes32(head + 1)), bytes32(0), "normal queue head cleared");
        assertEq(vm.load(address(eez), bytes32(head + 2)), bytes32(0), "static queue remains empty");
    }

    /// Pins the exact compiler storage layout read by native admission. Both
    /// zero/nonzero cursor and short/long dynamic bytes are checked against
    /// actual stored entries, so upstream layout drift cannot pass silently.
    function _assertDeferredStorageLayout(
        EEZ eez,
        ProofSystemBatchPerVerificationEntries memory batch,
        uint256 cursor
    )
        private
        view
    {
        uint256 head = uint256(keccak256(abi.encode(ROLLUP_ID, uint256(3))));
        uint256 queueHead = head + 1;
        uint256 count = batch.entries.length - 1;
        assertEq(uint256(vm.load(address(eez), bytes32(head))), uint64(block.number) | (cursor << 64));
        assertEq(uint256(vm.load(address(eez), bytes32(queueHead))), count, "normal queue mapping offset");
        assertEq(vm.load(address(eez), bytes32(head + 2)), bytes32(0), "static queue mapping offset");
        uint256 base = uint256(keccak256(abi.encode(queueHead)));
        for (uint256 i = 0; i < count; i++) {
            uint256 entrySlot = base + 7 * i;
            ExecutionEntry memory entry = batch.entries[i + 1];
            assertEq(uint256(vm.load(address(eez), bytes32(entrySlot))), 1, "entry stride and rollup-update length");
            assertEq(vm.load(address(eez), bytes32(entrySlot + 1)), entry.proxyEntryHash, "proxy hash offset");
            assertEq(vm.load(address(eez), bytes32(entrySlot + 2)), bytes32(0), "inline calls length offset");
            assertEq(vm.load(address(eez), bytes32(entrySlot + 3)), bytes32(0), "reentrant rows length offset");
            assertEq(vm.load(address(eez), bytes32(entrySlot + 4)), entry.rollingHash, "rolling hash offset");
            assertEq(uint256(vm.load(address(eez), bytes32(entrySlot + 5))), ROLLUP_ID | (uint256(1) << 64));
            uint256 rootData = uint256(keccak256(abi.encode(entrySlot)));
            assertEq(uint256(vm.load(address(eez), bytes32(rootData))), ROLLUP_ID, "one zero-delta own-rollup update");
            assertEq(vm.load(address(eez), bytes32(rootData + 1)), entry.rollupUpdates[0].currentRoot);
            assertEq(vm.load(address(eez), bytes32(rootData + 2)), entry.rollupUpdates[0].newRoot);
            bytes memory result = entry.returnData;
            uint256 resultHead = uint256(vm.load(address(eez), bytes32(entrySlot + 6)));
            if (result.length == 0) {
                assertEq(resultHead, 0, "empty-byte result encoding");
            } else if (result.length < 32) {
                bytes32 shortData;
                assembly {
                    shortData := mload(add(result, 0x20))
                }
                assertEq(resultHead, uint256(shortData) | (result.length * 2), "short-byte result length and payload");
            } else {
                assertEq(resultHead, result.length * 2 + 1, "long-byte result length encoding");
                uint256 resultData = uint256(keccak256(abi.encode(entrySlot + 6)));
                for (uint256 j = 0; j < result.length; j += 32) {
                    bytes32 word;
                    assembly {
                        word := mload(add(add(result, 0x20), j))
                    }
                    assertEq(vm.load(address(eez), bytes32(resultData + j / 32)), word, "long-byte payload layout");
                }
            }
        }
    }

    function _nonzeroBytes(uint256 length) private pure returns (bytes memory result) {
        result = new bytes(length);
        for (uint256 i = 0; i < result.length; i++) {
            result[i] = 0xa5;
        }
    }

    /// A fresh deployment per rung, so every rung pays the same cold start and
    /// the marginal is a clean difference.
    ///
    /// `preCreateProxies` off measures a sender's first settlement, with the
    /// proxy deployed inside the batch.
    function _rung(uint256 outboundEntries, bool preCreateProxies) private returns (uint256 gasUsed) {
        return _rung(Rung(outboundEntries, 1, preCreateProxies, false, 0));
    }

    function _rung(Rung memory shape) private returns (uint256 gasUsed) {
        delete touched;
        (EEZ eez, ECDSAProofSystem proofSystem) = _deployProtocol();
        EmptyTarget target = new EmptyTarget();
        touched.push(address(target));

        ProofSystemBatchPerVerificationEntries memory batch = _batch(eez, proofSystem, address(target), shape);
        return _measure(eez, batch, batch.entries[batch.entries.length - 1].rollupUpdates[0].newRoot);
    }

    function _measure(
        EEZ eez,
        ProofSystemBatchPerVerificationEntries memory batch,
        bytes32 expectedRoot
    )
        private
        returns (uint256 gasUsed)
    {
        batch.proofs[0] = _proof(_publicInputsHash(batch));
        bytes memory postCall = abi.encodeCall(EEZ.postAndVerifyBatch, (batch));
        for (uint256 i = 0; i < touched.length; i++) {
            vm.cool(touched[i]);
        }
        gasUsed = probe.measure(address(eez), postCall);
        (, bytes32 actualRoot,) = eez.rollups(ROLLUP_ID);
        assertEq(actualRoot, expectedRoot, "outer success must apply the intended root");
    }

    /// Mirrors `scripts/deploy.sh`: registry, proof system, threshold-1 manager
    /// whose vkey is the signer-address membership ticket, then `registerRollup`.
    function _deployProtocol() private returns (EEZ eez, ECDSAProofSystem proofSystem) {
        address prover = vm.addr(PROVER_KEY);
        eez = new EEZ(address(0xDEAD));
        proofSystem = new ECDSAProofSystem(prover);

        address[] memory proofSystems = new address[](1);
        proofSystems[0] = address(proofSystem);
        bytes32[] memory vkeys = new bytes32[](1);
        vkeys[0] = bytes32(uint256(uint160(prover)));
        Rollup manager = new Rollup(address(eez), address(this), 1, proofSystems, vkeys);

        assertEq(eez.registerRollup(address(manager), GENESIS_ROOT), ROLLUP_ID, "unexpected rollup id");
        touched.push(address(eez));
        touched.push(address(proofSystem));
        touched.push(address(manager));
    }

    /// A composer-shaped call: the leading anchor plus `outboundEntries` entries,
    /// all immediate, with state deltas chained from the live root.
    function _batch(
        EEZ eez,
        ECDSAProofSystem proofSystem,
        address target,
        Rung memory shape
    )
        private
        returns (ProofSystemBatchPerVerificationEntries memory batch)
    {
        (, bytes32 root,) = eez.rollups(ROLLUP_ID);
        ExecutionEntry[] memory entries = new ExecutionEntry[](1 + shape.entries);

        bytes32 next = keccak256(abi.encodePacked(root));
        entries[0] = _entry(eez, root, next, new L2ToL1Call[](0));
        root = next;

        for (uint256 i = 0; i < shape.entries; i++) {
            L2ToL1Call[] memory calls = new L2ToL1Call[](shape.callsPerEntry);
            for (uint256 j = 0; j < calls.length; j++) {
                address source = address(uint160(0x5000 + (shape.repeatSource ? 0 : i * shape.callsPerEntry + j)));
                calls[j] = L2ToL1Call({
                    revertNextNCalls: 0,
                    isStatic: false,
                    gas: 0,
                    sourceAddress: source,
                    sourceRollupId: ROLLUP_ID,
                    targetAddress: target,
                    value: 0,
                    data: abi.encodeCall(EmptyTarget.ping, ())
                });
                if (shape.preCreateProxies && (!shape.repeatSource || (i == 0 && j == 0))) {
                    touched.push(eez.createCrossChainProxy(source, ROLLUP_ID));
                }
            }
            next = keccak256(abi.encodePacked(root));
            entries[1 + i] = _entry(eez, root, next, calls);
            root = next;
        }

        address[] memory proofSystems = new address[](1);
        proofSystems[0] = address(proofSystem);
        RollupIdWithProofSystems[] memory rollupIds = new RollupIdWithProofSystems[](1);
        uint64[] memory indexes = new uint64[](1);
        rollupIds[0] = RollupIdWithProofSystems({rollupId: ROLLUP_ID, proofSystemIndexes: indexes});
        bytes[] memory proofs = new bytes[](1);

        bytes32[] memory blobHashes = new bytes32[](shape.blobs);
        uint256[] memory blobIndices = new uint256[](shape.blobs);
        for (uint256 i = 0; i < shape.blobs; i++) {
            blobHashes[i] = bytes32((uint256(1) << 248) | (i + 1));
            blobIndices[i] = i;
        }
        vm.blobhashes(blobHashes);
        batch = ProofSystemBatchPerVerificationEntries({
            expectedRootPerRollup: new ExpectedRootPerRollup[](0),
            entries: entries,
            staticEntries: new StaticExecutionEntry[](0),
            immediateEntryCount: entries.length,
            immediateStaticEntryCount: 0,
            proofSystems: proofSystems,
            rollupIdsWithProofSystems: rollupIds,
            blobIndices: blobIndices,
            callData: "",
            proofs: proofs,
            blockNumber: 0,
            bindMsgSenderInPublicInput: false
        });
    }

    /// An outbound settlement entry: `proxyEntryHash == 0` so EEZ.sol drains it
    /// inline, one state delta chaining `currentState` to the prior `newState`.
    function _entry(
        EEZ eez,
        bytes32 currentState,
        bytes32 newState,
        L2ToL1Call[] memory calls
    )
        private
        view
        returns (ExecutionEntry memory)
    {
        RollupUpdate[] memory updates = new RollupUpdate[](1);
        updates[0] = RollupUpdate({rollupId: ROLLUP_ID, currentRoot: currentState, newRoot: newState, etherDelta: 0});
        return ExecutionEntry({
            rollupUpdates: updates,
            proxyEntryHash: bytes32(0),
            l2ToL1Calls: calls,
            expectedL1ToL2Calls: new ExpectedL1ToL2Call[](0),
            rollingHash: _rollingHash(eez, updates, calls),
            destinationRollupId: ROLLUP_ID,
            success: true,
            returnData: ""
        });
    }

    /// EEZBase's tagged fold for this shape: the entry seed, then CALL_BEGIN and
    /// CALL_END per call. Every call succeeds with empty return data.
    function _rollingHash(
        EEZ eez,
        RollupUpdate[] memory updates,
        L2ToL1Call[] memory calls
    )
        private
        view
        returns (bytes32 hash)
    {
        return _rollingHashWithResult(eez, updates, calls, "");
    }

    function _rollingHashWithResult(
        EEZ eez,
        RollupUpdate[] memory updates,
        L2ToL1Call[] memory calls,
        bytes memory result
    )
        private
        view
        returns (bytes32 hash)
    {
        for (uint256 i = 0; i < updates.length; i++) {
            hash = keccak256(abi.encodePacked(hash, updates[i].rollupId, updates[i].currentRoot));
        }
        hash = keccak256(abi.encodePacked(hash, bytes32(0))); // proxyEntryHash
        for (uint256 i = 0; i < calls.length; i++) {
            bytes32 callHash = eez.computeCrossChainCallHash(
                false,
                calls[i].sourceAddress,
                calls[i].sourceRollupId,
                calls[i].targetAddress,
                eez.MAINNET_ROLLUP_ID(),
                calls[i].value,
                0,
                calls[i].data
            );
            hash = keccak256(abi.encodePacked(hash, uint8(1), callHash)); // CALL_BEGIN
            hash = keccak256(abi.encodePacked(hash, uint8(2), true, result)); // CALL_END(true, result)
        }
    }

    /// Mirrors `EEZ._verifyProofSystemBatch` for this single-rollup,
    /// single-proof-system, timeless batch.
    function _publicInputsHash(ProofSystemBatchPerVerificationEntries memory batch) private view returns (bytes32) {
        bytes32[] memory entryHashes = new bytes32[](batch.entries.length);
        for (uint256 i = 0; i < batch.entries.length; i++) {
            entryHashes[i] = keccak256(abi.encode(batch.entries[i]));
        }

        bytes32[] memory customDataHashes = new bytes32[](1);
        customDataHashes[0] = keccak256(abi.encode(ROLLUP_ID, bytes("")));
        bytes32[] memory blobHashes = new bytes32[](batch.blobIndices.length);
        for (uint256 i = 0; i < blobHashes.length; i++) {
            blobHashes[i] = blobhash(batch.blobIndices[i]);
        }
        bytes32[] memory staticEntryHashes = new bytes32[](batch.staticEntries.length);
        for (uint256 i = 0; i < staticEntryHashes.length; i++) {
            staticEntryHashes[i] = keccak256(abi.encode(batch.staticEntries[i]));
        }
        bytes32 sharedPublicInput = keccak256(
            abi.encodePacked(
                abi.encode(entryHashes),
                abi.encode(staticEntryHashes),
                abi.encode(blobHashes),
                keccak256(batch.callData),
                abi.encode(customDataHashes),
                address(0)
            )
        );
        bytes32 vkey = bytes32(uint256(uint160(vm.addr(PROVER_KEY))));
        bytes32 accumulator = keccak256(abi.encode(bytes32(0), ROLLUP_ID, vkey));
        return keccak256(abi.encodePacked(sharedPublicInput, accumulator));
    }

    function _proof(bytes32 publicInputsHash) private pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(PROVER_KEY, publicInputsHash);
        return abi.encodePacked(r, s, v);
    }
}
