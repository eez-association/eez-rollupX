// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {ProofSystemBatchPerVerificationEntries} from "eez-core-protocol/src/interfaces/IEEZ.sol";
import {IMetaCrossChainReceiver} from "eez-core-protocol/src/interfaces/IMetaCrossChainReceiver.sol";

/// @notice Test-only poster for consuming, or leaving unused, immediate entries.
/// @dev Keeps the registry's outer calldata verbatim for follower reconstruction.
///      Configuration is intentionally unauthenticated; this is not a production wrapper.
contract EEZMetaPostBatcher is IMetaCrossChainReceiver {
    EEZ public immutable eez;
    address private metaTarget;
    bytes private metaData;

    error OnlyEEZ();

    constructor(EEZ eez_) {
        eez = eez_;
    }

    function setMetaCall(address target, bytes calldata data) external {
        metaTarget = target;
        metaData = data;
    }

    function postAndVerifyBatch(ProofSystemBatchPerVerificationEntries calldata batch) external {
        eez.postAndVerifyBatch(batch);
    }

    /// @notice Executes one configured call while EEZ's immediate tables are available.
    function executeMetaCrossChainTransactions() external override {
        if (msg.sender != address(eez)) revert OnlyEEZ();
        address target = metaTarget;
        bytes memory data = metaData;
        delete metaTarget;
        delete metaData;
        if (target == address(0)) return;
        (bool ok, bytes memory result) = target.call(data);
        if (!ok) {
            assembly ("memory-safe") {
                revert(add(result, 32), mload(result))
            }
        }
    }
}
