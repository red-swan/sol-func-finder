// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

interface IPing {
    function ping(uint256 x) external;
}

interface IEncoder {
    function viaCall(address t, uint256 x) external;
}

contract Pinger is IPing {
    function ping(uint256) external {}
}

/// Same selector as `IPing.ping`, but not an `IPing`.
contract Unrelated {
    function ping(uint256) external {}
}

contract Encoder is IEncoder {
    // Interface call: guessed from the AST.
    function viaIface(IPing p, uint256 x) external {
        p.ping(x);
    }

    // Encoded: the function is named in the AST.
    function viaCall(address t, uint256 x) external {
        (bool ok,) = t.call(abi.encodeCall(IPing.ping, (x)));
        require(ok);
    }

    function viaSelector(address t, uint256 x) external {
        (bool ok,) = t.call(abi.encodeWithSelector(IPing.ping.selector, x));
        require(ok);
    }

    // Selector value only.
    function viaLiteral(address t, uint256 x) external {
        (bool ok,) = t.call(abi.encodeWithSelector(bytes4(0x773acdef), x));
        require(ok);
    }

    function viaSignature(address t, uint256 x) external {
        (bool ok,) = t.call(abi.encodeWithSignature("ping(uint256)", x));
        require(ok);
    }

    function viaKeccak(address t, uint256 x) external {
        (bool ok,) = t.call(abi.encodeWithSelector(bytes4(keccak256("ping(uint256)")), x));
        require(ok);
    }
}

/// An interface call on top of an encoded call: the chain is filed under the encoded one.
contract Mixed {
    function outer(IEncoder e, address t, uint256 x) external {
        e.viaCall(t, x);
    }
}
