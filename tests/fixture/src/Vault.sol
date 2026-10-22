// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Base} from "./Base.sol";
import {IVault} from "./IVault.sol";
import {Sink} from "./Sink.sol";
import {RecLib, freeHelper} from "./Lib.sol";

contract Vault is Base, IVault {
    constructor(Sink s) {
        sink = s;
        _ctorPing();
    }

    function _ctorPing() internal {
        sink.record(0);
    }

    // Overrides the hook that Base.entry calls.
    function _hook(uint256 x) internal override {
        sink.record(x);
    }

    function deposit(uint256 amount) external tracked(amount) {}

    function viaLib(uint256 x) external {
        RecLib.viaLib(sink, x);
    }

    function viaFree(uint256 x) external {
        freeHelper(sink, x);
    }

    function viaThis(uint256 x) external {
        this.deposit(x);
    }

    function _baseSuper(uint256 x) internal override {
        super._baseSuper(x);
    }

    function callSuper(uint256 x) external {
        _baseSuper(x);
    }

    function diamond(uint256 x) external {
        _viaA(x);
        _viaB(x);
    }

    function _viaA(uint256 x) internal {
        _shared(x);
    }

    function _viaB(uint256 x) internal {
        _shared(x);
    }

    function _shared(uint256 x) internal {
        sink.record(x);
    }

    function lowLevel(address a) external {
        (bool ok,) = a.call("");
        require(ok);
    }
}
