// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

contract NestedCreationChild {
    uint256 public value;

    constructor(uint256 initial) {
        value = initial;
    }

    function add(uint256 amount) external returns (uint256) {
        value += amount;
        return value;
    }
}

/// A callback must return the address it actually created, so the test does
/// not depend on an independently predicted CREATE or CREATE2 address.
contract NestedCreationFactory {
    address public lastChild;

    function obtain(bool fresh, bool deterministic, bytes32 salt, uint256 initial) external returns (address child) {
        if (fresh) {
            child = deterministic
                ? address(new NestedCreationChild{salt: salt}(initial))
                : address(new NestedCreationChild(initial));
            lastChild = child;
            require(child.code.length != 0, "factory cannot see child code");
            require(NestedCreationChild(child).value() == initial, "factory cannot read child state");
        } else {
            child = lastChild;
            require(child != address(0), "no existing child");
            require(child.code.length != 0, "existing child has no code");
            require(NestedCreationChild(child).value() == initial, "existing child has stale state");
        }
    }
}

/// Deployed on L1; factoryProxy is L1's proxy for the L2 factory.
contract NestedCreationRelay {
    function obtain(
        address factoryProxy,
        bool fresh,
        bool deterministic,
        bytes32 salt,
        uint256 initial
    )
        external
        returns (address)
    {
        return NestedCreationFactory(factoryProxy).obtain(fresh, deterministic, salt, initial);
    }
}

/// Deployed on L2. The final code check and add run in the original L2 frame
/// after the L1 call and its nested L2 callback have both returned.
contract NestedCreationCaller {
    address public lastChild;
    address public secondChild;
    uint256 public lastValue;

    error MissingChildCode(address child);

    function createAndUse(
        address relayProxy,
        address factoryProxy,
        bool fresh,
        bool deterministic,
        bytes32 salt
    )
        public
        returns (address child)
    {
        child = NestedCreationRelay(relayProxy).obtain(factoryProxy, fresh, deterministic, salt, 41);
        if (child.code.length == 0) revert MissingChildCode(child);
        uint256 result = NestedCreationChild(child).add(1);
        require(result == 42, "unexpected child value");
        lastChild = child;
        lastValue = result;
    }

    /// The second callback sees a write made after creation returned. A local
    /// CREATE then uses the factory nonce advanced by the first callback.
    function createUseAndReenter(
        address relayProxy,
        address factoryProxy,
        address localFactory,
        bool deterministic,
        bytes32 salt
    )
        external
    {
        address child = createAndUse(relayProxy, factoryProxy, true, deterministic, salt);
        require(
            NestedCreationRelay(relayProxy).obtain(factoryProxy, false, deterministic, salt, 42) == child,
            "reentry returned another child"
        );
        address next = NestedCreationFactory(localFactory).obtain(true, false, bytes32(0), 77);
        require(next != child && next.code.length != 0, "local creation failed");
        require(NestedCreationChild(next).value() == 77, "local child state");
        secondChild = next;
    }
}
