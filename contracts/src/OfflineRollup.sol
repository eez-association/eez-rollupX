// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {IEEZRegistry} from "eez-core-protocol/src/interfaces/IEEZRegistry.sol";
import {L1HistoryCode} from "./generated/L1HistoryCode.sol";

/// @notice New-network RollupX manager, binding complete offline evidence to actual L1 facts.
/// @dev Core's generic manager remains usable by other rollup profiles. This manager
///      exposes one initialization path and rejects Core's context-free configuration.
contract OfflineRollup is Rollup {
    bytes32 public constant CONTEXT_DOMAIN = keccak256("EEZ_ROLLUPX_L1_HISTORY_V1");
    bytes32 public constant PROFILE_DOMAIN = keccak256("EEZ_ROLLUPX_OFFLINE_PROFILE_V1");
    bytes32 public constant RULES_DOMAIN = keccak256("EEZ_ROLLUPX_L1_RULES_V1");
    address public constant HISTORY_ADDRESS = 0xeeeeEeeeEeeeEeEeEEEeEeeeeEEEEeeEEEee0077;
    uint64 public constant MAX_IMPORT_HEADERS = 64;
    uint64 public constant IMPORT_GAS_LIMIT = 2_000_000;
    uint256 public constant MAX_RULE_EPOCHS = 64;

    /// @dev Genesis hash binds bootstrap records and runtime/storage. Schedule fields
    ///      also bind non-header genesis configuration. l1ConfigHash is the canonical
    ///      L1 rule document digest; ruleTimes enumerates every timestamp transition.
    struct Profile {
        uint256 l1ChainId;
        uint256 l2ChainId;
        bytes32 genesisHash;
        uint64 genesisTimestamp;
        uint64 blockInterval;
        uint32 syncInterval;
        uint32 updateOffset;
        bytes32 l1ConfigHash;
        bytes32 historyCodeHash;
        uint64[] ruleTimes;
    }

    uint256 public l2ChainId;
    bytes32 public protocolConfigHash;
    bytes32 public l1ConfigHash;
    bytes32 public initialBlockHash;
    uint64[] private ruleTimes;

    error OfflineProfileRequired();
    error InvalidOfflineProfile();
    error UnregisteredOfflineRollup();
    error InvalidAnchorSelector();
    error InvalidAnchorCoordinates();

    /// @notice Complete canonical profile preimage, sufficient for independent reconstruction.
    event OfflineProfileInitialized(bytes canonicalProfile);

    constructor(address registry) Rollup(registry) {}

    /// @notice Old manager initialization cannot create a partially configured offline network.
    function initialize(address, uint256, address[] memory, bytes32[] memory) external pure override {
        revert OfflineProfileRequired();
    }

    /// @notice Initialize a fresh proxy atomically with its fixed network profile and proof policy.
    function initializeOffline(
        address initialOwner,
        uint256 initialThreshold,
        address[] memory proofSystems,
        bytes32[] memory vkeys,
        Profile calldata profile
    )
        external
        initializer
    {
        _initializeRollup(initialOwner, initialThreshold, proofSystems, vkeys);
        uint256 interval = uint256(profile.blockInterval) * profile.syncInterval;
        uint256 delay = uint256(profile.blockInterval) * (uint256(profile.updateOffset) + 1);
        if (
            profile.l1ChainId == 0 || profile.l1ChainId != block.chainid || profile.l2ChainId == 0
                || profile.genesisHash == bytes32(0) || profile.l1ConfigHash == bytes32(0) || profile.blockInterval == 0
                || profile.syncInterval < 2 || profile.updateOffset >= profile.syncInterval
                || interval > type(uint64).max || profile.genesisTimestamp < interval
                || uint256(profile.genesisTimestamp) + delay > type(uint64).max
                || profile.historyCodeHash != L1HistoryCode.HASH || profile.ruleTimes.length == 0
                || profile.ruleTimes.length > MAX_RULE_EPOCHS || profile.ruleTimes[0] != 0
        ) revert InvalidOfflineProfile();
        for (uint256 i; i < profile.ruleTimes.length; ++i) {
            if (i != 0 && profile.ruleTimes[i] <= profile.ruleTimes[i - 1]) revert InvalidOfflineProfile();
            ruleTimes.push(profile.ruleTimes[i]);
        }
        l2ChainId = profile.l2ChainId;
        l1ConfigHash = profile.l1ConfigHash;
        initialBlockHash = profile.genesisHash;
        bytes memory encoded =
            abi.encode(PROFILE_DOMAIN, HISTORY_ADDRESS, MAX_IMPORT_HEADERS, IMPORT_GAS_LIMIT, profile);
        protocolConfigHash = keccak256(encoded);
        emit OfflineProfileInitialized(encoded);
    }

    /// @notice Registration must use the same genesis committed by the profile.
    /// @dev EEZ installs its record before this callback; reverting rolls back
    ///      registration, including the counter and this manager's rollup ID.
    function rollupContractRegistered(uint64 id, address registrant) public override {
        super.rollupContractRegistered(id, registrant);
        (address manager, bytes32 root,) = IEEZRegistry(EEZContract).rollups(id);
        if (manager != address(this) || root != initialBlockHash || initialBlockHash == bytes32(0)) {
            revert InvalidOfflineProfile();
        }
    }

    /// @notice Rule-period identity for an authenticated L1 timestamp.
    /// @dev Each configured transition changes the identity, including blob-only changes.
    function l1RulesId(uint64 timestamp) public view returns (bytes32) {
        if (ruleTimes.length == 0) revert InvalidOfflineProfile();
        uint256 lo;
        uint256 hi = ruleTimes.length;
        while (lo + 1 < hi) {
            uint256 mid = (lo + hi) / 2;
            if (ruleTimes[mid] <= timestamp) lo = mid;
            else hi = mid;
        }
        return keccak256(abi.encode(RULES_DOMAIN, l1ConfigHash, ruleTimes[lo]));
    }

    /// @notice Bind either the exact inclusion parent/time or an available recent historical block.
    /// @dev The registry reads this before queue mutation. Accepted-root and rule-period
    ///      changes invalidate a historical proof even while its blockhash is available.
    function getCustomData(uint64 selector) external view override returns (bytes memory) {
        if (protocolConfigHash == bytes32(0) || rollupId == 0) revert UnregisteredOfflineRollup();
        if (block.timestamp > type(uint64).max || block.number == 0 || block.number > type(uint64).max) {
            revert InvalidAnchorCoordinates();
        }
        (address manager, bytes32 currentRoot,) = IEEZRegistry(EEZContract).rollups(rollupId);
        if (manager != address(this) || currentRoot == bytes32(0)) revert UnregisteredOfflineRollup();
        uint64 timestamp = uint64(block.timestamp);
        bytes32 rules = l1RulesId(timestamp);
        if (selector == type(uint64).max) {
            uint64 number = uint64(block.number - 1);
            bytes32 hash = blockhash(number);
            if (hash == bytes32(0)) revert BlockHashUnavailable(number);
            return abi.encode(
                CONTEXT_DOMAIN,
                block.chainid,
                EEZContract,
                address(this),
                rollupId,
                l2ChainId,
                protocolConfigHash,
                rules,
                currentRoot,
                uint8(1),
                number,
                hash,
                timestamp
            );
        }
        if (selector == 0) revert InvalidAnchorSelector();
        if (selector >= block.number || block.number - selector > 256) revert BlockHashUnavailable(selector);
        bytes32 historicalHash = blockhash(selector);
        if (historicalHash == bytes32(0)) revert BlockHashUnavailable(selector);
        return abi.encode(
            CONTEXT_DOMAIN,
            block.chainid,
            EEZContract,
            address(this),
            rollupId,
            l2ChainId,
            protocolConfigHash,
            rules,
            currentRoot,
            uint8(2),
            selector,
            historicalHash
        );
    }
}
