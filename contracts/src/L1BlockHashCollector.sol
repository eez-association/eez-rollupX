// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {L1BlockhashContract} from "./L2/L1BlockhashContract.sol";

/// @notice Reference application that processes imported L1 hashes one at a time.
/// @dev Deploy separately from genesis. The predeploy already retains the hashes;
///      this copy demonstrates application work committed together with its cursor.
contract L1BlockHashCollector {
    L1BlockhashContract public constant HISTORY = L1BlockhashContract(0xeeeeEeeeEeeeEeEeEEEeEeeeeEEEEeeEEEee0077);

    // One extra bit lets the cursor represent exhaustion after the last uint64 block.
    uint256 public nextL1BlockNumber;
    mapping(uint64 number => bytes32 hash) public blockHashes;

    event BlockHashCollected(uint64 indexed number, bytes32 hash);

    /// @param firstL1BlockNumber An already imported block at or after the network's bootstrap.
    /// @dev The strict lookup rejects an unsupported start or a missing history contract.
    constructor(uint64 firstL1BlockNumber) {
        HISTORY.getL1BlockHash(firstL1BlockNumber);
        nextL1BlockNumber = firstL1BlockNumber;
    }

    /// @notice Collect the next imported hash, or return (false, 0) without changing state.
    /// @dev Anyone can poll. Only a cursor beyond the current endpoint is normal waiting;
    ///      malformed history and covered-but-missing records still revert. Keep this check
    ///      inside the transaction: an off-chain check cannot account for another poll
    ///      advancing the cursor before this transaction executes.
    function poll() external returns (bool collected, bytes32 hash) {
        (uint64 latestImported,) = HISTORY.current();
        uint256 next = nextL1BlockNumber;
        if (next > latestImported) return (false, bytes32(0));

        // The endpoint comparison proves that narrowing the cursor preserves its value.
        uint64 number = uint64(next);
        hash = HISTORY.getL1BlockHash(number);
        blockHashes[number] = hash;
        nextL1BlockNumber = next + 1;
        emit BlockHashCollected(number, hash);
        return (true, hash);
    }
}
