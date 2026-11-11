# Safe 1.4.1 deployment fixtures

`bytecode.json` contains unmodified creation bytecode from the official
`@safe-global/safe-contracts@1.4.1` npm package, with its package digest, npm
integrity and source commit. No install scripts were executed. The source is
[Safe commit bf943f8](https://github.com/safe-global/safe-smart-account/tree/bf943f80fec5ac647159d26161446ac5d716a294).
The package's LGPL-3.0 license is included as `LICENSE`.

The Safe and SafeProxyFactory runtime hashes match the canonical 1.4.1
[Safe deployment metadata](https://github.com/safe-global/safe-deployments/tree/main/src/assets/v1.4.1).
The process test verifies these hashes after deployment. MultiSend contains an
address immutable, so its runtime varies with the deployment address; its
creation code comes from the same pinned package.

These fixtures avoid recompiling Safe with the fork's different Solidity compiler
and optimizer settings. The regression exercises atomic initialization and
MultiSend's nested factory calls using the published code.
