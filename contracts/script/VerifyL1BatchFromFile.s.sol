// SPDX-License-Identifier: MIT
pragma solidity ^0.8.34;

import {VerifyL1BatchCalldata} from "eez-core-protocol/script/e2e/scenarios/shared/Verify.s.sol";
import {Script} from "forge-std/Script.sol";

/// @notice File transport for calldata too large for an operating-system argument.
/// @dev Runs the pinned verifier unchanged, including all table and live-root checks.
contract VerifyL1BatchFromFile is Script {
    function runFromFile(
        string calldata inputPath,
        address rollups,
        bytes calldata expectedTable,
        bytes calldata expectedSteps,
        bytes calldata expectedStaticTable,
        bool allowPartial
    )
        external
    {
        new VerifyL1BatchCalldata()
            .run(
                vm.parseBytes(vm.readFile(inputPath)),
                rollups,
                expectedTable,
                expectedSteps,
                expectedStaticTable,
                allowPartial
            );
    }
}
