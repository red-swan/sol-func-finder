// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {IVault} from "./IVault.sol";
import {Sink} from "./Sink.sol";
import {Vault} from "./Vault.sol";

contract Router {
    function route(IVault v, uint256 x) external {
        v.deposit(x);
    }

    function make(Sink s) external returns (Vault) {
        return new Vault(s);
    }
}
