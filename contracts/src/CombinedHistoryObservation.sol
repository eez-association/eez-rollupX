// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {L1BlockhashContract} from "./L2/L1BlockhashContract.sol";

/// @notice Test application distinguishing history, ordinary and cross-chain prefixes.
contract CombinedHistoryObservation {
    L1BlockhashContract private constant HISTORY = L1BlockhashContract(0xeeeeEeeeEeeeEeEeEEEeEeeeeEEEEeeEEEee0077);
    uint256 public marker;

    function bump() external {
        marker++;
    }

    function setMarker(uint256 value) external {
        marker = value;
    }

    function observe() public view returns (uint256 value, uint64 number, bytes32 hash) {
        value = marker;
        (number, hash) = HISTORY.getLatestImportedL1Block();
    }

    /// @dev A failed mutable destination call is authenticated lookup data,
    ///      with no delivery transaction. Return the same bytes as observe().
    function observeAndRevert() external view {
        (uint256 value, uint64 number, bytes32 hash) = observe();
        bytes memory result = abi.encode(value, number, hash);
        assembly {
            revert(add(result, 32), mload(result))
        }
    }
}

/// @notice One L1 source creates both observations before a real L2 delivery.
contract CombinedHistoryReader {
    CombinedHistoryObservation public immutable target;
    uint256 public beforeMarker;
    uint64 public beforeNumber;
    bytes32 public beforeHash;
    bytes32 public lookupHash;
    uint256 public afterMarker;
    uint256 public runs;

    constructor(address proxy) {
        target = CombinedHistoryObservation(proxy);
    }

    function run(uint256 nextMarker) external {
        (beforeMarker, beforeNumber, beforeHash) = target.observe();
        // Low-level CALL deliberately selects mutable lookup routing despite
        // the destination implementation only reading state before reverting.
        (bool success, bytes memory outcome) = address(target).call(abi.encodeCall(target.observeAndRevert, ()));
        require(!success, "mutable lookup did not revert");
        lookupHash = keccak256(outcome);
        require(lookupHash == keccak256(abi.encode(beforeMarker, beforeNumber, beforeHash)), "lookup prefix differs");
        target.setMarker(nextMarker);
        (afterMarker,,) = target.observe();
        require(afterMarker == nextMarker, "delivery did not advance state");
        runs++;
    }
}
