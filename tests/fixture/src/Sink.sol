// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

contract Sink {
    uint256 public total;

    function record(uint256 x) external {
        total += x;
    }
}
