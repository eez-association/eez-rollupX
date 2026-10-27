// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {L1BlockhashContract} from "./L2/L1BlockhashContract.sol";

/// @notice Integration fixture for ordinary applications reading the history
///         contract and PREVRANDAO in the same L2 execution environment.
contract OfflineHistoryProbe {
    L1BlockhashContract private constant HISTORY = L1BlockhashContract(0xeeeeEeeeEeeeEeEeEEEeEeeeeEEEEeeEEEee0077);

    struct Observation {
        uint256 l2Block;
        uint64 number;
        bytes32 hash;
        uint256 randao;
    }

    Observation[] public records;

    function record() external {
        (uint64 number, bytes32 hash) = HISTORY.current();
        records.push(Observation(block.number, number, hash, block.prevrandao));
    }

    function observe() external view returns (uint64 number, bytes32 hash, uint256 randao) {
        (number, hash) = HISTORY.current();
        randao = block.prevrandao;
    }

    function hashAt(uint64 number) external view returns (bytes32) {
        return HISTORY.getL1BlockHash(number);
    }
}
