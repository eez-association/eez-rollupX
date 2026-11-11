// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {MockProofSystem} from "eez-core-protocol/test/mocks/MockProofSystem.sol";
import {RollupDeployment} from "./RollupDeployment.sol";

/// @notice Process-test peer with its own verifier; its empty batches need no native DA.
/// @dev Only the foreign rollup uses the mock. The node under test retains its real signer.
contract ForeignRollupFixture {
    uint64 public immutable rollupId;
    address public immutable proofSystem;

    constructor(EEZ registry) {
        proofSystem = address(new MockProofSystem());
        address[] memory systems = new address[](1);
        systems[0] = proofSystem;
        bytes32[] memory vkeys = new bytes32[](1);
        vkeys[0] = bytes32(uint256(1));
        address manager = address(RollupDeployment.deploy(address(registry), address(this), 1, systems, vkeys));
        rollupId = registry.registerRollup(manager, bytes32(uint256(1)));
    }
}
