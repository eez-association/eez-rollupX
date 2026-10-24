// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

/// @notice Permanent L1 number-to-hash history installed by the L2 execution protocol.
/// @dev Genesis installs runtime and slots 0..6 directly; there is no public initializer.
///      The execution engine authenticates the recorded native transaction and its ancestry. This
///      contract enforces caller, schedule and append-only storage, not L1 consensus.
contract L1BlockhashContract {
    address public constant SYSTEM_ADDRESS = 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE;

    // Full-width scalar slots deliberately make the genesis/executor layout explicit.
    mapping(uint64 number => bytes32 hash) internal hashes; // slot 0
    uint256 internal firstNumber; // slot 1
    uint256 internal endpoint; // slot 2
    uint256 internal nextCutoff; // slot 3
    uint256 internal interval; // slot 4
    uint256 internal updateDelay; // slot 5
    uint256 internal maxImportHeaders; // slot 6

    error UnauthorizedHistoryImport();
    error InvalidHistoryState();
    error InvalidHistoryPosition();
    error HistoryImportTooLarge();
    error InvalidHistoryHash();
    error L1BlockNotImported(uint64 number);

    /// @notice Exact hash for an imported block. Absence is never represented by zero.
    function getL1BlockHash(uint64 number) external view returns (bytes32) {
        if (number < firstNumber || number > endpoint || hashes[number] == bytes32(0)) {
            revert L1BlockNotImported(number);
        }
        return hashes[number];
    }

    /// @notice The currently selected origin, also the last imported block.
    function current() external view returns (uint64 number, bytes32 hash) {
        if (endpoint > type(uint64).max || firstNumber > endpoint || hashes[uint64(endpoint)] == bytes32(0)) {
            revert InvalidHistoryState();
        }
        return (uint64(endpoint), hashes[uint64(endpoint)]);
    }

    /// @notice Append consecutive hashes and advance one scheduled cutoff, even on an empty import.
    /// @dev Transaction zero at cutoff + updateDelay, sent by the native system
    ///      address with zero value. It consumes transaction gas without advancing
    ///      the system account nonce.
    ///      The engine checks canonical calldata, ancestry, delay and PREVRANDAO;
    ///      failure invalidates the block. Empty imports explicitly repeat the origin.
    ///      This emits no logs and makes no external calls.
    function importHistory(uint64 cutoff, bytes32[] calldata newHashes) external {
        if (msg.sender != SYSTEM_ADDRESS) revert UnauthorizedHistoryImport();
        if (
            interval == 0 || interval > type(uint64).max || updateDelay == 0 || updateDelay > interval
                || maxImportHeaders == 0 || firstNumber > endpoint || endpoint > type(uint64).max
                || hashes[uint64(endpoint)] == bytes32(0)
        ) revert InvalidHistoryState();
        if (cutoff != nextCutoff || uint256(cutoff) + updateDelay != block.timestamp) {
            revert InvalidHistoryPosition();
        }
        if (newHashes.length > maxImportHeaders) revert HistoryImportTooLarge();
        uint256 newEndpoint = endpoint + newHashes.length;
        uint256 followingCutoff = uint256(cutoff) + interval;
        if (newEndpoint > type(uint64).max || followingCutoff > type(uint64).max) revert InvalidHistoryState();
        for (uint256 i; i < newHashes.length; ++i) {
            uint64 number = uint64(endpoint + i + 1);
            // A conflicting existing record is invalid even if its hash agrees:
            // append-only numbering never revisits any installed record.
            if (newHashes[i] == bytes32(0) || hashes[number] != bytes32(0)) revert InvalidHistoryHash();
            hashes[number] = newHashes[i];
        }
        endpoint = newEndpoint;
        nextCutoff = followingCutoff;
    }
}
