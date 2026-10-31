# Pinned protocol scenario inventory

Protocol: `84a5b5207880f192a6a5896cbfe0bac5249db47f`. Paths are relative to the protocol checkout.

Baseline: `c702f90d`; qualified implementation: `6808f13b` (PR head `f0f40401` adds audit documentation only).
All 64 candidate scenarios passed upstream assertions and independent causal/native-blob checks, with zero skips or exclusions.
Fresh canonical-L1 follower replay covered their settlement batches. These results precede the signing-domain simplification; see [current validation status](README.md).

[Original detailed inventory, assertions, failure excerpts and per-scenario evidence](https://github.com/eez-association/eez-rollupX/blob/f0f40401d491ddaf4e1190e408dba92963b9118f/docs/audits/2026-10-30-full-protocol-e2e/scenarios.json) preserves the complete historical record.

| Scenario / source path | Source SHA-256 | Baseline | Qualified candidate |
| --- | --- | --- | --- |
| multi-call-twice<br>`script/e2e/scenarios/multi_call/L1_to_L2/multi-call-twice/E2EMultiCallTwice.s.sol` | `b18f0d315db1a06da8b3fbc2d42bc3b0d835c01e43ee268c4f8d76bbbf8e9166` | PASS | PASS |
| multi-call-two-diff<br>`script/e2e/scenarios/multi_call/L1_to_L2/multi-call-two-diff/E2EMultiCallTwoDiff.s.sol` | `ad8880ac2cc2cf5d99c5c8a6ec6f729e69aaab577e09c1a85a2ff81bb06027cc` | PASS | PASS |
| multi-call-twiceL2<br>`script/e2e/scenarios/multi_call/L2_to_L1/multi-call-twiceL2/E2EMultiCallTwiceL2.s.sol` | `06981584c29bbbba675772cdfc28c16730590d2e59da8b8e774aece48b1d38c0` | PASS | PASS |
| multi-call-two-diffL2<br>`script/e2e/scenarios/multi_call/L2_to_L1/multi-call-two-diffL2/E2EMultiCallTwoDiffL2.s.sol` | `a43497529b4dfd9a022a98e8c191f94e3168664e3e577790ff7c046d199eebd3` | PASS | PASS |
| counter-multi-tx<br>`script/e2e/scenarios/multi_tx/L1_to_L2/counter-multi-tx/E2ECounterMultiTx.s.sol` | `e7e9c9db24dcbee3d5f06bd3c4835eb52dec8f215e517610e86042ad56c21276` | PASS | PASS |
| deepNested<br>`script/e2e/scenarios/nested/L1_to_L2/deepNested/E2EDeepNested.s.sol` | `b953b9add03f118df80551952af237fd8788db6aaa821e2ef05acca23452ebdd` | PASS | PASS |
| flash-loan<br>`script/e2e/scenarios/nested/L1_to_L2/flash-loan/E2EFlashLoan.s.sol` | `93c66bfc8072489fb10268fd0fa0ffc58a2e1dd0a7119c3aa03e5a33c2040221` | PASS | PASS |
| multi-call-nested<br>`script/e2e/scenarios/nested/L1_to_L2/multi-call-nested/E2EMultiCallNested.s.sol` | `a4d48e9cbf53c5d08b3d55523d99fd08f2fd31e24e7f1965a15258ead8e33767` | PASS | PASS |
| nestedCounter<br>`script/e2e/scenarios/nested/L1_to_L2/nestedCounter/E2ENestedCounter.s.sol` | `f9a75e9bbb80d14301c30132c4df5a41dd9bc5ee94afa9f6cda26cf764766016` | PASS | PASS |
| multi-call-nestedL2<br>`script/e2e/scenarios/nested/L2_to_L1/multi-call-nestedL2/E2EMultiCallNestedL2.s.sol` | `8821188d4566be2a334a43e45bb0851697d319fbe422af0acdeb1cb46c7544f9` | PASS | PASS |
| nestedCounterL2<br>`script/e2e/scenarios/nested/L2_to_L1/nestedCounterL2/E2ENestedCounterL2.s.sol` | `95516ab65f70322fee202d99a9f8bc45ab42cbe8e8d32a6979acb6d59161099c` | PASS | PASS |
| bridge<br>`script/e2e/scenarios/one_way/L1_to_L2/bridge/E2EBridge.s.sol` | `3268b6fe37853dc87f398f9f13d39b2a0064fe143a2345fb5997cf3c17d37079` | PASS | PASS |
| counter<br>`script/e2e/scenarios/one_way/L1_to_L2/counter/E2ECounter.s.sol` | `939126ea0485d3cfe2f539919d7d22941b93b3cb3f04aa1a5b627fe679d6856c` | PASS | PASS |
| bridgeL2<br>`script/e2e/scenarios/one_way/L2_to_L1/bridgeL2/E2EBridgeL2.s.sol` | `b9450f5e3ba5c1d4dc408d4e429363b2449ff023c00d59ba27a412a17050423f` | PASS | PASS |
| counterL2<br>`script/e2e/scenarios/one_way/L2_to_L1/counterL2/E2ECounterL2.s.sol` | `87cbf8d6cffe9fa40d00cf3fdb2106f5efc2f4471b4d0e9e0747bc6465984e0b` | PASS | PASS |
| reentrant<br>`script/e2e/scenarios/reentrant/L1_to_L2/reentrant/E2EReentrant.s.sol` | `fd808e0dfdf36919675faf69dec3dd93f2eda85716bc48d8e3aff732a91df46d` | PASS | PASS |
| catchNestedRevertAfterCallback<br>`script/e2e/scenarios/revert/L1_to_L2/catchNestedRevertAfterCallback/E2ECatchNestedRevertAfterCallback.s.sol` | `f1bb8743082266654e4b9f2f2879a994b3bde14aef46b979874e1ed603b2c638` | FAIL | PASS |
| catchRemoteAndNestedRevertsAfterSuccessfulCallbacks<br>`script/e2e/scenarios/revert/L1_to_L2/catchRemoteAndNestedRevertsAfterSuccessfulCallbacks/E2ECatchRemoteAndNestedRevertsAfterSuccessfulCallbacks.s.sol` | `501884c436781a9a0fbf5846f0515b407ee41093fd91c679bdc5f1b7e470bb81` | FAIL | PASS |
| catchRemoteRevertAfterCallback<br>`script/e2e/scenarios/revert/L1_to_L2/catchRemoteRevertAfterCallback/E2ECatchRemoteRevertAfterCallback.s.sol` | `13eaec428c537fbc9cda772a86c7e0d96384c5207d4f4558db87745037629bae` | FAIL | PASS |
| nestedCallRevert<br>`script/e2e/scenarios/revert/L1_to_L2/nestedCallRevert/E2ENestedCallRevert.s.sol` | `25948fa6b6b7f0b965cf196d3c24e625c1e5573044796b8976242c6821abdc73` | PASS | PASS |
| revertCounter<br>`script/e2e/scenarios/revert/L1_to_L2/revertCounter/E2ERevertCounter.s.sol` | `90c76827d4a8eaae7f2f0c8806bf381cc36051983b783064ddb4f04cb687241f` | PASS | PASS |
| revertFromOtherChain<br>`script/e2e/scenarios/revert/L1_to_L2/revertFromOtherChain/E2ERevertFromOtherChain.s.sol` | `522c8a8c07c4445f8a8dd22750c5082f8468b2fda0e46451e57414d037e9c786` | PASS | PASS |
| catchNestedRevertAfterCallbackL2<br>`script/e2e/scenarios/revert/L2_to_L1/catchNestedRevertAfterCallbackL2/E2ECatchNestedRevertAfterCallbackL2.s.sol` | `50e75acb061bb265468c141f518140d74e4ce5a48340c068de07828857004b25` | FAIL | PASS |
| catchRemoteAndNestedRevertsAfterSuccessfulCallbacksL2<br>`script/e2e/scenarios/revert/L2_to_L1/catchRemoteAndNestedRevertsAfterSuccessfulCallbacksL2/E2ECatchRemoteAndNestedRevertsAfterSuccessfulCallbacksL2.s.sol` | `06c2a46bb2f109010757c91746f1e356ff2337526f868c831254193ac49a325a` | FAIL | PASS |
| catchRemoteRevertAfterCallbackL2<br>`script/e2e/scenarios/revert/L2_to_L1/catchRemoteRevertAfterCallbackL2/E2ECatchRemoteRevertAfterCallbackL2.s.sol` | `4fe444d5a3407f7c529260da1f8fa535713c07b929c9b4c778ef6f03ef70ea84` | FAIL | PASS |
| nestedCallRevertL2<br>`script/e2e/scenarios/revert/L2_to_L1/nestedCallRevertL2/E2ENestedCallRevertL2.s.sol` | `081481cccb88200f5e1ada1670e96be0e643bd2123d1ddb9614455bd7ddffc60` | PASS | PASS |
| nestedRollbackGroupsRestoreStateBeforeContinuationL2<br>`script/e2e/scenarios/revert/L2_to_L1/nestedRollbackGroupsRestoreStateBeforeContinuationL2/E2ENestedRollbackGroupsRestoreStateBeforeContinuationL2.s.sol` | `a399951f4b197c8c9c8b1b29fb7159a194558353e36948187d2116b1a1406bc1` | FAIL | PASS |
| nestedRollbackRestoresInnerThenOuterStateL2<br>`script/e2e/scenarios/revert/L2_to_L1/nestedRollbackRestoresInnerThenOuterStateL2/E2ENestedRollbackRestoresInnerThenOuterStateL2.s.sol` | `2d6d42219e976af7a18e27cd33e60bd733283cd6368d2100fb89ec9f4605bd29` | FAIL | PASS |
| revertCounterL2<br>`script/e2e/scenarios/revert/L2_to_L1/revertCounterL2/E2ERevertCounterL2.s.sol` | `ca6a843fe6c95cc6e501f9b223f55f982aca00b84507987695a0f25850d66bf3` | PASS | PASS |
| revertFromOtherChainAndCallAgainL2<br>`script/e2e/scenarios/revert/L2_to_L1/revertFromOtherChainAndCallAgainL2/E2ERevertFromOtherChainAndCallAgainL2.s.sol` | `d7e41f00e9e9e5724c653df607a8696d2705045fe3f3afc57df471064f52281d` | PASS | PASS |
| revertFromOtherChainL2<br>`script/e2e/scenarios/revert/L2_to_L1/revertFromOtherChainL2/E2ERevertFromOtherChainL2.s.sol` | `c80e86f56036cc983420a15ba9bd28bf2210d13ab93c0c3f090f4f0560465797` | PASS | PASS |
| revertFromOtherChainNested<br>`script/e2e/scenarios/revert/L2_to_L1/revertFromOtherChainNested/E2ERevertFromOtherChainNested.s.sol` | `23a82ff09e50e323aaffeaaaa97f646ac050a14feeebe14223912f09c5af8cc3` | PASS | PASS |
| rollbackGroupWithStaticReadThenWriteL2<br>`script/e2e/scenarios/revert/L2_to_L1/rollbackGroupWithStaticReadThenWriteL2/E2ERollbackGroupWithStaticReadThenWriteL2.s.sol` | `b92c9e8f35da9e7be978d3afc14b226c226844c945283156010472b6e530e1ff` | FAIL | PASS |
| deepStaticReadCallbacksUnderStaticAndMutableCalls<br>`script/e2e/scenarios/static/L1_to_L2/deepStaticReadCallbacksUnderStaticAndMutableCalls/E2EDeepStaticReadCallbacksUnderStaticAndMutableCalls.s.sol` | `851dacd559c9e8ddbd0feb965e5c44a53edde17ac2534fab7feadbe2b312efca` | FAIL | PASS |
| mutableCallMakesReadThatCallsBack<br>`script/e2e/scenarios/static/L1_to_L2/mutableCallMakesReadThatCallsBack/E2EMutableCallMakesReadThatCallsBack.s.sol` | `ee0034bdb56328c36db18a4a9ad7f5069876c61e081031d8ae43d89fa26d2640` | FAIL | PASS |
| nestedStaticCounter<br>`script/e2e/scenarios/static/L1_to_L2/nestedStaticCounter/E2ENestedStaticCounter.s.sol` | `28c41209a2f269c53fc0f5d25113c682f83f309718c00d4e210d1a6c31ad1dd2` | PASS | PASS |
| nestedStaticLocalWriteL2<br>`script/e2e/scenarios/static/L1_to_L2/nestedStaticLocalWriteL2/E2ENestedStaticLocalWriteL2.s.sol` | `ca9bf3ef5f7421ceea254ff9d9fbeac6a8d0c2e43bbe713a23de8320baff8032` | FAIL | PASS |
| staticLocalWrite<br>`script/e2e/scenarios/static/L1_to_L2/staticLocalWrite/E2EStaticLocalWrite.s.sol` | `b5ad3ae5ba18c47e47ed3219c90297003cbc3b6569de7f2dfec2550022e46963` | PASS | PASS |
| staticReadCallsBackAndReadsAgain<br>`script/e2e/scenarios/static/L1_to_L2/staticReadCallsBackAndReadsAgain/E2EStaticReadCallsBackAndReadsAgain.s.sol` | `016abf9102fe63ac32af595b013e47899999dbfdefc83b774234fe160a36a59b` | FAIL | PASS |
| staticReadWrite<br>`script/e2e/scenarios/static/L1_to_L2/staticReadWrite/E2EStaticReadWrite.s.sol` | `a5c6aa1286d20514c041c0d3d7cfc03cf2d285702851b4cde2f1556b1b6dc491` | PASS | PASS |
| staticReadX2Write<br>`script/e2e/scenarios/static/L1_to_L2/staticReadX2Write/E2EStaticReadX2Write.s.sol` | `2a0f3253eb4a448b156c37ab82fb5b28bc56aeadaba18bb09207f2c5e16007c0` | PASS | PASS |
| staticThenNestedWrites<br>`script/e2e/scenarios/static/L1_to_L2/staticThenNestedWrites/E2EStaticThenNestedWrites.s.sol` | `6338ae6b2b5537fe542bd3afd5febaa907ad6648d5dc4ac0542deb5f0353a49d` | PASS | PASS |
| topLevelStaticCounter<br>`script/e2e/scenarios/static/L1_to_L2/topLevelStaticCounter/E2ETopLevelStaticCounter.s.sol` | `6abf9f02d38b0d73fbefc80766e45d1e5f52dc03a8e4b7350bb8e321a68afe6e` | PASS | PASS |
| topLevelStaticReentrantCounter<br>`script/e2e/scenarios/static/L1_to_L2/topLevelStaticReentrantCounter/E2ETopLevelStaticReentrantCounter.s.sol` | `005ae8ad1f776099fa424642b14e0058d8cfcdc9d7e3a5f9bf2a7cc79753209a` | PASS | PASS |
| topLevelStaticReentrantMissingProxy<br>`script/e2e/scenarios/static/L1_to_L2/topLevelStaticReentrantMissingProxy/E2ETopLevelStaticReentrantMissingProxy.s.sol` | `5345ac85f7cc61d31378eaac0b3ae625ba272214dff2c3ac2e6a666e6ad46c34` | FAIL | PASS |
| deepStaticReadCallbacksUnderStaticAndMutableCallsL2<br>`script/e2e/scenarios/static/L2_to_L1/deepStaticReadCallbacksUnderStaticAndMutableCallsL2/E2EDeepStaticReadCallbacksUnderStaticAndMutableCallsL2.s.sol` | `756ea0f69442a58ecd00401de1ace1fbd6ffa30b5c4a96652227e9a6ddf3cf0d` | FAIL | PASS |
| mutableCallMakesReadThatCallsBackL2<br>`script/e2e/scenarios/static/L2_to_L1/mutableCallMakesReadThatCallsBackL2/E2EMutableCallMakesReadThatCallsBackL2.s.sol` | `be9d96972df1cb8b7b70afa20edbd7ca44a87cbb6ce8f9c34877247cd8a23162` | FAIL | PASS |
| nestedStaticCounterL2<br>`script/e2e/scenarios/static/L2_to_L1/nestedStaticCounterL2/E2ENestedStaticCounterL2.s.sol` | `7e327465d05b597f724bdb69139058aa16606f02432127541f4986601aed2dad` | PASS | PASS |
| nestedStaticLocalWriteL1<br>`script/e2e/scenarios/static/L2_to_L1/nestedStaticLocalWriteL1/E2ENestedStaticLocalWriteL1.s.sol` | `e18a8c5b80f7c6a985e5d0a0769760d95abb97c0ad91cc3845827b0e19842ecf` | FAIL | PASS |
| staticCounterL2<br>`script/e2e/scenarios/static/L2_to_L1/staticCounterL2/E2EStaticCounterL2.s.sol` | `503f6dd9e929b676a9af5aa2cf5239bc86665dc8b7b57410f4eb800df49f22da` | PASS | PASS |
| staticLocalWriteL2<br>`script/e2e/scenarios/static/L2_to_L1/staticLocalWriteL2/E2EStaticLocalWriteL2.s.sol` | `beff1b4df738bce16ccdc8730c2f53913357c0410cf4f8b0f0ef7ba5ded24887` | FAIL | PASS |
| staticReadCallsBackAndReadsAgainL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadCallsBackAndReadsAgainL2/E2EStaticReadCallsBackAndReadsAgainL2.s.sol` | `e03ac9686c9f904677d5245809b1b78db2d92edcabc3deb5f1decba5c7fd7fab` | FAIL | PASS |
| staticReadCatchesCallbackRevertAndChecksUncaughtPropagationL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadCatchesCallbackRevertAndChecksUncaughtPropagationL2/E2EStaticReadCatchesCallbackRevertAndChecksUncaughtPropagationL2.s.sol` | `878ed24c4cfee0a9ad102597d501be1a0c01f448563eb7e4f1f51e5b33863ed8` | FAIL | PASS |
| staticReadHandlesFailedCallbackL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadHandlesFailedCallbackL2/E2EStaticReadHandlesFailedCallbackL2.s.sol` | `1e25260f60e3bcc7d28e8b8e27341eb76d56d24c879e5163e82f2ca1c62211be` | FAIL | PASS |
| staticReadPropagatesFailedCallbackL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadPropagatesFailedCallbackL2/E2EStaticReadPropagatesFailedCallbackL2.s.sol` | `e4c3944c05201bb00ef8b70facb8ca7339c049986372adf0ea0e61a2b6e7ca69` | FAIL | PASS |
| staticReadThenCatchFailedWriteL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadThenCatchFailedWriteL2/E2EStaticReadThenCatchFailedWriteL2.s.sol` | `898fff2d3cf5a26aa44c520e1d1216e9891c63e068f9f863fb77d97a47731061` | FAIL | PASS |
| staticReadThenNestedWriteCaughtRevertAndRollbackL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadThenNestedWriteCaughtRevertAndRollbackL2/E2EStaticReadThenNestedWriteCaughtRevertAndRollbackL2.s.sol` | `b7b414cabb282738cdf84cc9cbbf4509b62ed48d555802e246237a2ad6350829` | FAIL | PASS |
| staticReadThenRollbackWriteL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadThenRollbackWriteL2/E2EStaticReadThenRollbackWriteL2.s.sol` | `de3f811c1c67d3a7d48c905ae9d9f1d49a41174d666ac413900bb288b0e5236e` | FAIL | PASS |
| staticReadThenWriteWithCallbackL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadThenWriteWithCallbackL2/E2EStaticReadThenWriteWithCallbackL2.s.sol` | `e7a177a77945d7ceb928781a5903d74afff3c8f65e30bad50683ad5890f8d699` | FAIL | PASS |
| staticReadWriteL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadWriteL2/E2EStaticReadWriteL2.s.sol` | `48b96d74ab7a529d083e7b32991c8746e32e22100c9847d4f02d8be3615b7f69` | FAIL | PASS |
| staticReadX2WriteL2<br>`script/e2e/scenarios/static/L2_to_L1/staticReadX2WriteL2/E2EStaticReadX2WriteL2.s.sol` | `a9ef07acff540d2779077cf4b5d8200220e6d7bde19e9dc6946e3dd83a3eefcc` | FAIL | PASS |
| staticThenNestedWritesL2<br>`script/e2e/scenarios/static/L2_to_L1/staticThenNestedWritesL2/E2EStaticThenNestedWritesL2.s.sol` | `9c00c6b6b63484a06294b464bfd472a000f3ac55d95de644f13cf8bbc046f7fa` | FAIL | PASS |
| topLevelStaticReentrantCounterL2<br>`script/e2e/scenarios/static/L2_to_L1/topLevelStaticReentrantCounterL2/E2ETopLevelStaticReentrantCounterL2.s.sol` | `bd490d67292e929b13f500cd53af027b27218aeeffe4926bb634daf0ec05413a` | FAIL | PASS |
| topLevelStaticReentrantMissingProxyL2<br>`script/e2e/scenarios/static/L2_to_L1/topLevelStaticReentrantMissingProxyL2/E2ETopLevelStaticReentrantMissingProxyL2.s.sol` | `73bd4c9a1569a7199f56b2241d51cd44e8f173a7376637c9bb5436f6e83c32e8` | FAIL | PASS |
