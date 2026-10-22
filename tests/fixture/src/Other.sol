// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Base} from "./Base.sol";
import {Sink} from "./Sink.sol";

contract Other is Base {
    constructor(Sink s) {
        sink = s;
    }

    function _hook(uint256 x) internal override {
        sink.record(x);
    }
}

contract Quiet is Base {}
