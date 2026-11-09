// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Memory} from "@openzeppelin/contracts/utils/Memory.sol";
import {RLP} from "@openzeppelin/contracts/utils/RLP.sol";

interface IL1HashHistory {
    function getL1BlockHash(uint64 number) external view returns (bytes32);
}

/// @notice Ordinary L2 application reads authenticated by the permanent L1 history predeploy.
/// @dev Deploy with the network's fixed L1 header rules. This is an EVM contract,
///      not a cross-chain call or a source of fresh L1 data. The caller supplies
///      EIP-1186 account/storage proofs for the exact requested block and slot.
contract L1StorageProof {
    using Memory for bytes;
    using Memory for Memory.Slice;
    using RLP for bytes;
    using RLP for Memory.Slice;

    address public constant HISTORY = 0xeeeeEeeeEeeeEeEeEEEeEeeeeEEEEeeEEEee0077;
    bytes32 private constant EMPTY_ROOT = keccak256(hex"80");
    uint256 public constant MAX_PROOF_NODES = 65;
    uint256 public constant MAX_NODE_BYTES = 1024;
    uint256 public constant MAX_HEADER_BYTES = 2048;

    // type(uint64).max means disabled. Header number/timestamp coordinates match
    // eez-protocol::l1_history::HeaderRules; no caller selects a different layout.
    uint64 public immutable londonBlock;
    uint64 public immutable shanghaiTime;
    uint64 public immutable cancunTime;
    uint64 public immutable pragueTime;
    uint64 public immutable unsupportedTime;

    error InvalidHeader();
    error InvalidProof();
    error ProofLimit();
    error NonCanonicalEncoding();

    constructor(uint64 london, uint64 shanghai, uint64 cancun, uint64 prague, uint64 unsupported) {
        if (
            shanghai > cancun || cancun > prague || prague > unsupported
                || (london == type(uint64).max && shanghai != type(uint64).max)
        ) revert InvalidHeader();
        londonBlock = london;
        shanghaiTime = shanghai;
        cancunTime = cancun;
        pragueTime = prague;
        unsupportedTime = unsupported;
    }

    /// @notice Verify a storage value, including canonical non-inclusion (zero).
    /// @dev Proof arrays contain the root and subsequent hashed nodes. Inline
    ///      children are already in their parents and must not be repeated.
    ///      Missing/truncated/extra nodes reject. An authenticated empty trie
    ///      needs no nodes; an absent account also requires an empty storage proof.
    function readStorage(
        uint64 blockNumber,
        address account,
        bytes32 slot,
        bytes calldata rawHeader,
        bytes[] calldata accountProof,
        bytes[] calldata storageProof
    )
        external
        view
        returns (uint256)
    {
        if (rawHeader.length == 0 || rawHeader.length > MAX_HEADER_BYTES) revert ProofLimit();
        bytes32 recorded = IL1HashHistory(HISTORY).getL1BlockHash(blockNumber);
        if (recorded == bytes32(0) || keccak256(rawHeader) != recorded) revert InvalidHeader();
        bytes32 stateRoot = _stateRoot(blockNumber, rawHeader);
        _bound(accountProof);
        _bound(storageProof);
        (bool exists, bytes memory accountRlp) = _lookup(stateRoot, keccak256(abi.encodePacked(account)), accountProof);
        if (!exists) {
            if (storageProof.length != 0) revert InvalidProof();
            return 0;
        }
        Memory.Slice[] memory fields = accountRlp.decodeList();
        if (fields.length != 4) revert InvalidProof();
        _scalar(fields[0], 8); // Account nonce is uint64 on the supported L1s.
        _scalar(fields[1], 32);
        bytes32 storageRoot = _word(fields[2]);
        _word(fields[3]); // Code hash is a fixed-width hash, never an integer.
        (exists, accountRlp) = _lookup(storageRoot, keccak256(abi.encodePacked(slot)), storageProof);
        if (!exists) return 0;
        uint256 value = _scalar(accountRlp.asSlice(), 32);
        // Canonical Ethereum storage omits zero leaves entirely.
        if (value == 0) revert NonCanonicalEncoding();
        return value;
    }

    function _stateRoot(uint64 number, bytes memory raw) private view returns (bytes32) {
        Memory.Slice[] memory h = raw.decodeList();
        if (h.length < 15 || h.length > 21 || _scalar(h[8], 8) != number) revert InvalidHeader();
        uint256 timestamp = _scalar(h[11], 8);
        if (_active(unsupportedTime, timestamp)) revert InvalidHeader();
        if (_active(shanghaiTime, timestamp) && !_active(londonBlock, number)) revert InvalidHeader();
        uint256 count = 15 + (_active(londonBlock, number) ? 1 : 0) + (_active(shanghaiTime, timestamp) ? 1 : 0)
            + (_active(cancunTime, timestamp) ? 3 : 0) + (_active(pragueTime, timestamp) ? 1 : 0);
        if (h.length != count) revert InvalidHeader();
        // Prefix fields keep the same meaning across all supported forks.
        _word(h[0]);
        _word(h[1]);
        _fixed(h[2], 20);
        bytes32 root = _word(h[3]);
        _word(h[4]);
        _word(h[5]);
        _fixed(h[6], 256);
        _scalar(h[7], 32);
        _scalar(h[9], 8);
        _scalar(h[10], 8);
        if (_bytes(h[12]).length > 32) revert InvalidHeader();
        _word(h[13]);
        _fixed(h[14], 8);
        uint256 i = 15;
        if (_active(londonBlock, number)) _scalar(h[i++], 8);
        if (_active(shanghaiTime, timestamp)) _word(h[i++]);
        if (_active(cancunTime, timestamp)) {
            _scalar(h[i++], 8);
            _scalar(h[i++], 8);
            _word(h[i++]);
        }
        if (_active(pragueTime, timestamp)) _word(h[i]);
        return root;
    }

    function _active(uint64 coordinate, uint256 value) private pure returns (bool) {
        return coordinate != type(uint64).max && value >= coordinate;
    }

    function _bound(bytes[] calldata proof) private pure {
        if (proof.length > MAX_PROOF_NODES) revert ProofLimit();
        for (uint256 i; i < proof.length; ++i) {
            if (proof[i].length == 0 || proof[i].length > MAX_NODE_BYTES) revert ProofLimit();
        }
    }

    /// @dev The pinned OpenZeppelin TrieProof implements inclusion only. This
    ///      traversal also requires explicit authenticated non-inclusion: empty
    ///      branch child, divergent compact path, or the canonical empty root.
    ///      It reuses the pinned RLP/slice decoder with stricter scalar checks.
    ///      Each step consumes key nibbles; a 32-byte secure key permits at most
    ///      65 nodes. No generic trie execution or unbounded recursion is exposed.
    function _lookup(bytes32 root, bytes32 key, bytes[] calldata proof) private pure returns (bool, bytes memory) {
        if (root == EMPTY_ROOT) {
            if (proof.length != 0) revert InvalidProof();
            return (false, bytes(""));
        }
        if (proof.length == 0 || keccak256(proof[0]) != root) revert InvalidProof();
        bytes memory node = proof[0];
        uint256 used = 1;
        uint256 offset;
        for (uint256 step; step < MAX_PROOF_NODES; ++step) {
            Memory.Slice[] memory items = node.decodeList();
            Memory.Slice child;
            if (items.length == 17) {
                // Secure account/storage keys have a fixed 64-nibble length.
                if (offset == 64) return _terminal(_bytes(items[16]), used, proof.length);
                if (_bytes(items[16]).length != 0) revert InvalidProof();
                child = items[_nibble(key, offset++)];
            } else if (items.length == 2) {
                bytes memory path = _bytes(items[0]);
                if (path.length == 0) revert InvalidProof();
                uint8 flag = uint8(path[0]) >> 4;
                if (flag > 3 || (flag % 2 == 0 && uint8(path[0]) % 16 != 0)) revert InvalidProof();
                uint256 start = 2 - flag % 2;
                uint256 length = path.length * 2 - start;
                bool leaf = flag >= 2;
                if (length > 64 - offset || (leaf && length != 64 - offset) || (!leaf && length == 0)) {
                    revert InvalidProof();
                }
                // Validate the value/reference shape even when the path diverges.
                bytes memory value;
                if (leaf) {
                    value = _bytes(items[1]);
                    if (value.length == 0) revert InvalidProof();
                } else {
                    _childShape(items[1], false);
                }
                for (uint256 j; j < length; ++j) {
                    uint256 at = j + start;
                    uint8 nibble = (uint8(path[at / 2]) >> (at % 2 == 0 ? 4 : 0)) & 15;
                    if (nibble != _nibble(key, offset + j)) return _terminal(bytes(""), used, proof.length);
                }
                offset += length;
                if (leaf) return _terminal(value, used, proof.length);
                child = items[1];
            } else {
                revert InvalidProof();
            }
            (bool embedded, bytes memory childId) = _childShape(child, true);
            if (childId.length == 0) return _terminal(bytes(""), used, proof.length);
            if (embedded) {
                node = childId;
            } else {
                if (used == proof.length || proof[used].length < 32 || keccak256(proof[used]) != bytes32(childId)) {
                    revert InvalidProof();
                }
                node = proof[used++];
            }
        }
        revert ProofLimit();
    }

    function _childShape(Memory.Slice child, bool allowEmpty) private pure returns (bool, bytes memory) {
        bytes memory encoded = child.toBytes();
        if (uint8(encoded[0]) >= 0xc0) {
            if (encoded.length >= 32) revert InvalidProof();
            return (true, encoded);
        }
        bytes memory value = _bytes(child);
        if (value.length != 32 && !(allowEmpty && value.length == 0)) revert InvalidProof();
        return (false, value);
    }

    function _terminal(bytes memory value, uint256 used, uint256 length) private pure returns (bool, bytes memory) {
        if (used != length) revert InvalidProof();
        return (value.length != 0, value);
    }

    function _nibble(bytes32 key, uint256 offset) private pure returns (uint8) {
        return (uint8(key[offset / 2]) >> (offset % 2 == 0 ? 4 : 0)) & 15;
    }

    function _bytes(Memory.Slice item) private pure returns (bytes memory value) {
        value = item.readBytes();
        if (keccak256(RLP.encode(value)) != keccak256(item.toBytes())) revert NonCanonicalEncoding();
    }

    function _fixed(Memory.Slice item, uint256 length) private pure returns (bytes memory value) {
        value = _bytes(item);
        if (value.length != length) revert NonCanonicalEncoding();
    }

    function _word(Memory.Slice item) private pure returns (bytes32) {
        return bytes32(_fixed(item, 32));
    }

    function _scalar(Memory.Slice item, uint256 maxBytes) private pure returns (uint256 value) {
        bytes memory raw = _bytes(item);
        if (raw.length > maxBytes || (raw.length != 0 && raw[0] == 0)) revert NonCanonicalEncoding();
        value = item.readUint256();
        if (keccak256(RLP.encode(value)) != keccak256(item.toBytes())) revert NonCanonicalEncoding();
    }
}
