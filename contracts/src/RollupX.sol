// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {IRollupContract} from "eez-core-protocol/src/interfaces/IRollup.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {IEEZRegistry} from "eez-core-protocol/src/interfaces/IEEZRegistry.sol";
import {L1HistoryCode} from "./generated/L1HistoryCode.sol";

/// @dev The pinned registry exposes this public getter in addition to IEEZRegistry.
interface IRollupXRegistry is IEEZRegistry {
    function rollups(uint64 rollupId) external view returns (address rollupContract, bytes32 root, uint256 etherBalance);
}

/// @notice Rollup manager binding offline evidence to actual L1 facts.
/// @dev Implements the pinned IRollupContract extension point directly. Proof policy,
///      ownership and root escape follow Core's reference Rollup; the offline profile
///      is mandatory and has a single atomic initializer.
contract RollupX is IRollupContract, OwnableUpgradeable {
    bytes32 public constant CONTEXT_DOMAIN = keccak256("EEZ_ROLLUPX_L1_HISTORY_V1");
    /// @notice Fixed execution/validation contract; there is no selectable old profile.
    uint64 public constant VALIDATION_VERSION = 6;
    bytes32 public constant PROFILE_DOMAIN = keccak256("EEZ_ROLLUPX_OFFLINE_PROFILE");
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
        address feeCollector;
        uint64[] ruleTimes;
    }

    /// @notice The central EEZ registry this rollup is registered with
    address public immutable EEZContract;

    /// @notice The rollupId this contract manages. Written once on registration.
    uint64 public rollupId;

    /// @notice Minimum number of proof systems that must attest per batch (M of N). Owner is
    ///         free to set this to any value, including above the current PS count (which
    ///         effectively locks the rollup until more PSes are added).
    /// @dev Enforced internally by `checkProofSystemsAndGetVkeys`; not on the `IRollupContract`
    ///      interface (registry doesn't read it as a separate value).
    uint256 public threshold;

    /// @notice Per-proof-system verification key. `bytes32(0)` = not allowed.
    mapping(address proofSystem => bytes32 vkey) public verificationKey;

    /// @notice Emitted when a proof system is added with its initial verification key.
    event ProofSystemAdded(address indexed proofSystem, bytes32 verificationKey);

    /// @notice Emitted when a proof system is removed from the allowed set.
    event ProofSystemRemoved(address indexed proofSystem);

    /// @notice Emitted when an allowed proof system receives a replacement verification key.
    event VerificationKeyUpdated(address indexed proofSystem, bytes32 newVerificationKey);

    /// @notice Emitted when the threshold is initialized or updated.
    event ThresholdChanged(uint256 newThreshold);

    /// @notice Emitted after the registry accepts an owner-requested root replacement.
    event RootEscape(bytes32 newRoot);

    /// @notice The registration callback was called by an address other than EEZContract.
    error NotEEZRegistry();

    /// @notice The EEZ contract address is zero.
    error InvalidEEZContract();

    /// @notice A verification key is zero, or initial array lengths differ.
    error InvalidConfig();

    /// @notice A registration callback was received after a rollup ID was already assigned.
    error AlreadyRegistered();

    /// @notice The account registering this manager is not its current owner.
    error UnauthorizedRegistrantAccount(address registrant);

    /// @notice An attempt was made to add a proof system that already has a nonzero key.
    error ProofSystemAlreadyAllowed(address proofSystem);

    /// @notice A submitted proof system has no key, is zero, or breaks strictly increasing address order.
    error ProofSystemNotAllowed(address proofSystem);

    /// @notice A management op (`removeProofSystem` / `updateVerificationKey`) targeted a
    ///         proof system that isn't currently added (zero vkey).
    error ProofSystemNotAdded(address proofSystem);

    /// @notice Reverts during `checkProofSystemsAndGetVkeys` when fewer non-zero vkeys would
    ///         be returned than this manager's threshold requires
    error ThresholdNotMet(uint256 submitted, uint256 required);

    /// @notice `getCustomData` was asked to bind a blockNumber whose `blockhash`
    ///         is unavailable (≥ current block or older than the last 256 blocks → 0).
    error BlockHashUnavailable(uint64 blockNumber);

    uint256 public l2ChainId;
    bytes32 public protocolConfigHash;
    bytes32 public l1ConfigHash;
    bytes32 public initialBlockHash;
    uint64[] private ruleTimes;

    error InvalidOfflineProfile();
    error UnregisteredOfflineRollup();
    error InvalidAnchorSelector();
    error InvalidAnchorCoordinates();

    /// @notice Complete canonical profile preimage, sufficient for independent reconstruction.
    event OfflineProfileInitialized(bytes canonicalProfile);

    /// @notice Bind the implementation to its registry and lock implementation storage.
    constructor(address registry) {
        if (registry == address(0)) revert InvalidEEZContract();
        EEZContract = registry;
        _disableInitializers();
    }

    /// @notice Initialize a fresh proxy atomically with its fixed network profile and proof policy.
    function initialize(
        address initialOwner,
        uint256 initialThreshold,
        address[] memory proofSystems,
        bytes32[] memory vkeys,
        Profile calldata profile
    )
        external
        initializer
    {
        __Ownable_init(initialOwner);
        if (proofSystems.length != vkeys.length) revert InvalidConfig();
        _setThreshold(initialThreshold);
        for (uint256 i; i < proofSystems.length; ++i) {
            _addProofSystem(proofSystems[i], vkeys[i]);
        }
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
                || profile.feeCollector == address(0)
                || profile.feeCollector == 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE
                || profile.feeCollector == 0x4B2760F84B83d6b3829E30A7573B0172B6d6402C
        ) revert InvalidOfflineProfile();
        for (uint256 i; i < profile.ruleTimes.length; ++i) {
            if (i != 0 && profile.ruleTimes[i] <= profile.ruleTimes[i - 1]) revert InvalidOfflineProfile();
            ruleTimes.push(profile.ruleTimes[i]);
        }
        l2ChainId = profile.l2ChainId;
        l1ConfigHash = profile.l1ConfigHash;
        initialBlockHash = profile.genesisHash;
        bytes memory encoded = abi.encode(
            PROFILE_DOMAIN, VALIDATION_VERSION, HISTORY_ADDRESS, MAX_IMPORT_HEADERS, IMPORT_GAS_LIMIT, profile
        );
        protocolConfigHash = keccak256(encoded);
        emit OfflineProfileInitialized(encoded);
    }

    /// @notice Registration must use the same genesis committed by the profile.
    /// @dev EEZ installs its record before this callback; reverting rolls back
    ///      registration, including the counter and this manager's rollup ID.
    function rollupContractRegistered(uint64 id, address registrant) public override {
        if (msg.sender != EEZContract) revert NotEEZRegistry();
        if (rollupId != 0) revert AlreadyRegistered();
        if (registrant != owner()) revert UnauthorizedRegistrantAccount(registrant);
        rollupId = id;
        (address manager, bytes32 root,) = IRollupXRegistry(EEZContract).rollups(id);
        if (manager != address(this) || root != initialBlockHash || initialBlockHash == bytes32(0)) {
            revert InvalidOfflineProfile();
        }
    }

    /// @notice Rule-period identity for an authenticated L1 timestamp.
    /// @dev Each configured transition changes the identity, including blob-only changes.
    function l1RulesId(uint64 timestamp) public view returns (bytes32) {
        uint256 hi = ruleTimes.length;
        if (hi == 0) revert InvalidOfflineProfile();
        uint256 lo;
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
        bytes32 profileHash = protocolConfigHash;
        uint64 id = rollupId;
        if (profileHash == bytes32(0) || id == 0) revert UnregisteredOfflineRollup();
        if (block.timestamp > type(uint64).max || block.number == 0 || block.number > type(uint64).max) {
            revert InvalidAnchorCoordinates();
        }
        (address manager, bytes32 currentRoot,) = IRollupXRegistry(EEZContract).rollups(id);
        if (manager != address(this) || currentRoot == bytes32(0)) revert UnregisteredOfflineRollup();
        uint64 timestamp = uint64(block.timestamp);
        bytes32 rules = l1RulesId(timestamp);
        if (selector == 0) revert InvalidAnchorSelector();
        bool actualParent = selector == type(uint64).max;
        uint64 number = actualParent ? uint64(block.number - 1) : selector;
        // BLOCKHASH returns zero for current/future blocks and anchors older than
        // 256 blocks, as well as unavailable hashes. All reject with the same error.
        bytes32 hash = blockhash(number);
        if (hash == bytes32(0)) revert BlockHashUnavailable(number);
        if (actualParent) {
            return abi.encode(
                CONTEXT_DOMAIN,
                block.chainid,
                EEZContract,
                address(this),
                id,
                l2ChainId,
                profileHash,
                rules,
                currentRoot,
                uint8(1),
                number,
                hash,
                timestamp
            );
        }
        return abi.encode(
            CONTEXT_DOMAIN,
            block.chainid,
            EEZContract,
            address(this),
            id,
            l2ChainId,
            profileHash,
            rules,
            currentRoot,
            uint8(2),
            number,
            hash
        );
    }

    /// @notice Bulk vkey lookup for the chosen PS subset of a posting batch.
    /// @dev Strict: every `proofSystem` in the input MUST be allowed for this rollup
    ///      (non-zero vkey). Reverts `ProofSystemNotAllowed` on the first unknown one. There
    ///      is no zero-padding semantic — the orchestrator must compose batches whose
    ///      proofSystem subset for THIS rollup is a subset of this manager's allowed set,
    ///      and whose size is at least the manager's threshold. Implication: the (rid × ps)
    ///      verificationKeysPerRollup the registry sees is uniformly non-zero.
    /// @param proofSystems Allowed proof-system addresses in strictly increasing order, meeting the threshold.
    /// @return vkeys Nonzero verification keys in the same order as proofSystems.
    function checkProofSystemsAndGetVkeys(address[] calldata proofSystems)
        external
        view
        returns (bytes32[] memory vkeys)
    {
        if (proofSystems.length < threshold) revert ThresholdNotMet(proofSystems.length, threshold);
        vkeys = new bytes32[](proofSystems.length);
        // Strictly-increasing check: rejects address(0) AND duplicates in one pass. The registry
        // already enforces the same invariant on the batch's global PS list and on each
        // rollup's `proofSystemIndexes[]` (so the resolved subset reaches us already sorted),
        // but checking here keeps this manager safe for callers that don't pre-sort.
        address prev = address(0);
        for (uint256 i = 0; i < proofSystems.length; i++) {
            address ps = proofSystems[i];
            if (uint160(ps) <= uint160(prev)) revert ProofSystemNotAllowed(ps);
            bytes32 vk = verificationKey[ps];
            if (vk == bytes32(0)) revert ProofSystemNotAllowed(ps);
            vkeys[i] = vk;
            prev = ps;
        }
    }

    // EEZ snapshots proof policy before executing hooks and separately gates
    // setRoot during an active batch. Owner changes cannot alter that snapshot.
    /// @notice Adds a proof system to this rollup's allowed set. The owner is responsible
    ///         for verifying that `proofSystem` is a contract conforming to `IProofSystem`.
    /// @param proofSystem Address to add; it must not already have a verification key.
    /// @param vkey Nonzero verification key for the proof system.
    function addProofSystem(address proofSystem, bytes32 vkey) external onlyOwner {
        _addProofSystem(proofSystem, vkey);
    }

    /// @notice Removes a proof system. Owner is responsible for ensuring the remaining set
    ///         can still meet `threshold`; otherwise the rollup will be locked until more
    ///         PSes are added or `setThreshold` is lowered.
    /// @param proofSystem Currently allowed proof system to remove.
    function removeProofSystem(address proofSystem) external onlyOwner {
        if (verificationKey[proofSystem] == bytes32(0)) revert ProofSystemNotAdded(proofSystem);
        delete verificationKey[proofSystem];
        emit ProofSystemRemoved(proofSystem);
    }

    /// @notice Rotates the verification key for an already-allowed proof system
    /// @param proofSystem Currently allowed proof system whose key is replaced.
    /// @param newVkey Nonzero replacement verification key.
    function updateVerificationKey(address proofSystem, bytes32 newVkey) external onlyOwner {
        if (newVkey == bytes32(0)) revert InvalidConfig();
        if (verificationKey[proofSystem] == bytes32(0)) revert ProofSystemNotAdded(proofSystem);
        verificationKey[proofSystem] = newVkey;
        emit VerificationKeyUpdated(proofSystem, newVkey);
    }

    /// @notice Updates the threshold. Any value is accepted, including values above the
    ///         current PS count (locks the rollup) or zero (any batch passes the threshold
    ///         check). Owner is responsible for picking a sane value.
    /// @param newThreshold Minimum number of distinct allowed proof systems required per batch.
    function setThreshold(uint256 newThreshold) external onlyOwner {
        _setThreshold(newThreshold);
    }

    /// @notice Owner escape hatch — directly sets the rollup's root via the central
    ///         registry. Single state-mutating call from this contract back into EEZ.
    /// @dev Passes `rollupId` explicitly so the registry doesn't need a reverse lookup. The
    ///      registry validates `msg.sender == rollups[rollupId].rollupContract` and reverts
    ///      `RollupBatchActiveThisBlock` if `lastVerifiedBlock(rollupId) == block.number` (i.e.,
    ///      a postAndVerifyBatch has touched this rollup in the current block) — the escape hatch
    ///      is locked out for the rest of the block once a verified state transition lands.
    /// @param newRoot Replacement L2 block-hash commitment for this rollup.
    function setRoot(bytes32 newRoot) external onlyOwner {
        IEEZRegistry(EEZContract).setRoot(rollupId, newRoot);
        emit RootEscape(newRoot);
    }

    /// @dev Stores a nonzero key for a previously unconfigured proof system and emits ProofSystemAdded.
    /// @param proofSystem Address to add; address validity and interface conformance are not checked here.
    /// @param vkey Nonzero verification key for the proof system.
    function _addProofSystem(address proofSystem, bytes32 vkey) internal {
        if (vkey == bytes32(0)) revert InvalidConfig();
        if (verificationKey[proofSystem] != bytes32(0)) revert ProofSystemAlreadyAllowed(proofSystem);
        verificationKey[proofSystem] = vkey;
        emit ProofSystemAdded(proofSystem, vkey);
    }

    /// @dev Stores the threshold without range checks and emits ThresholdChanged.
    /// @param newThreshold Minimum proof-system count; zero and values above the allowed set size are accepted.
    function _setThreshold(uint256 newThreshold) internal {
        threshold = newThreshold;
        emit ThresholdChanged(newThreshold);
    }
}
