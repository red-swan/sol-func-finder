// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Sink} from "./Sink.sol";

abstract contract Base {
    Sink internal sink;

    modifier tracked(uint256 x) {
        sink.record(x);
        _;
    }

    // Inherited external function: a new entry point on every concrete child.
    function entry(uint256 x) external {
        _hook(x);
    }

    function _hook(uint256 x) internal virtual {}

    function overloaded(uint256 x) external {
        sink.record(x);
    }

    function overloaded(address) external {}

    function rec(uint256 n) public {
        if (n == 0) {
            sink.record(n);
            return;
        }
        rec(n - 1);
    }

    // Inherited chain: the concrete contract only matters at the ends of the run.
    function chain(uint256 x) external {
        _mid(x);
    }

    function _mid(uint256 x) internal {
        _inner(x);
    }

    function _inner(uint256 x) internal {
        sink.record(x);
    }

    function _baseSuper(uint256 x) internal virtual {
        sink.record(x);
    }
}
