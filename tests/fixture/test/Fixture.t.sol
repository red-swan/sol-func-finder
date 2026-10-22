// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Sink} from "../src/Sink.sol";

contract FixtureTest {
    function testRecord() external {
        new Sink().record(1);
    }
}
