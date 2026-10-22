// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Sink} from "./Sink.sol";

function freeHelper(Sink s, uint256 x) {
    s.record(x);
}

library RecLib {
    function viaLib(Sink s, uint256 x) internal {
        s.record(x);
    }
}
