# Revisión independiente de integración

| Campo | Identidad y alcance |
| --- | --- |
| Fecha / estado | 2026-10-24 UTC. Revisión estática terminada; ejecución de suites y Kurtosis a cargo del trabajo principal. |
| Fuente | Rama `codex/l1-history-offline-proving`, HEAD `3e23955dc477c9e675364776f118b1cecda75774` más cambios locales. Los hashes inferiores identifican los archivos principales inspeccionados. |
| Core | Checkout limpio en `84a5b5207880f192a6a5896cbfe0bac5249db47f`, línea `feature/e2e_edge_cases`. |
| Contrato | [Historia y proving offline](../../proposals/l1-origin-and-offline-proving.md), [native-v1 y tesorería](../../proposals/native-system-transactions.md), especificaciones del core fijado y [revisión del draft](../2026-10-24-specs-compliance/README.md). |
| Límites de la primera fase | Sin compilaciones, ejecución de tests, despliegues ni cambios de implementación por este revisor. Esta revisión no acredita resultados de procesos ni interoperabilidad real con Nethermind. |

No se confirmó otro defecto de integración en las rutas inspeccionadas, aparte de
R01/R02 ya registrados y asignados al trabajo principal. No se reabren las
decisiones aceptadas: redes nuevas sin legacy, core fijado, calendario propio,
V0 compacto, limpieza inicial y alcance condicional del prover.

## Rutas contrastadas

| Requisito | Inspección estática |
| --- | --- |
| Native-v1: identidad, orden, gas y nonce | `eez-primitives`, `native_source`, `ReconstructionActions`, planes nativos, construcción local, executor y comparación DA del signer. Los índices se conservan antes del filtrado; Finalize compara los bytes reconstruidos; el presupuesto usa gas acumulado previo; el handler no incrementa el nonce nativo. |
| Fees y FeeVault | Crédito después de finalizar cada transacción ordinaria con su gas final; exclusión nativa; aritmética comprobada; aliasing y SELFDESTRUCT; separación de simulación L1. Contrato con recipient mutable y transferencia de ownership en dos pasos. |
| Historia y anclas | Transacción de historia grabada, estado padre, ancestry y delay compartidos; mismo origen para header y PREVRANDAO. Contextos actual-parent/historical y restricciones de acciones contrastados con `getCustomData` y la autorización final. |
| Producción ordinaria | Bucle del reloj separado de preparación de settlement; adquisición de headers fuera del executor; snapshots y comprobaciones de parent/contexto antes del commit. No se deduce disponibilidad ilimitada de esta lectura. |
| Prover externo | La documentación activa especifica native-v1 distinto del draft-v0, la gramática V0 propia, una API `prove.v2.Prover`, identidad RLP de diez elementos, inputs locales independientes y validación offline sin fallback RPC. La generación de protobuf no se presenta como conformidad de ejecución. |

## Corrección de la observación sobre cobertura de tesorería

Una comunicación preliminar de este revisor indicó que no había localizado un
proceso de rotación y withdrawal con replay. **Esa observación era incorrecta:**
el test ya existe en
[native_system/treasury.rs](../../../crates/eez-node-e2e/tests/native_system/treasury.rs),
registrado por `mod treasury` en `native_system.rs`. No se añadió como resultado
de esta revisión.

`recipient_rotation_withdrawal_and_offline_replay` contiene assertions suficientes
para el recorrido solicitado:

- Comprueba el vault y su owner/recipient desde génesis, saldo inicial cero y
  acumulación de fees; rechaza un setter no autorizado y verifica que no cambió
  recipient (líneas 52–90).
- Rota mediante owner y ejecuta un withdrawal desde otro usuario. Verifica
  dirección emisora, un único evento, recipient/caller y aumento exacto del
  balance del destinatario; el destinatario anterior no cobra (92–153).
- Compara el saldo residual del vault a la altura del withdrawal con
  `block.baseFeePerGas * receipt.gasUsed`, y conserva la identidad de génesis
  (154–166). La fixture no envía otra transacción ordinaria posterior en ese bloque.
- Espera a que el bloque quede safe, inicia un follower frío y compara el bloque
  completo, los tres receipts, balances de vault y ambos recipients, recipient
  almacenado y owner (168–210). Esto comprueba contenido y estado, no sólo altura.

Por tanto, no queda un hueco de diseño de test para este recorrido. **Su resultado
actual debe acreditarse ejecutándolo.** La ausencia efectiva de conexiones
salientes del signer requiere `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`: `ProofSignerHandle`
selecciona entonces `scripts/ci/run-offline-signer.py`, cuyo filtro seccomp sobrevive
a exec y a los threads. El test no fuerza esa opción por sí mismo. La ejecución
coordinada y sus resultados se registrarán en el informe principal; este documento
no los anticipa. Tampoco sustituye esa ejecución con una afirmación sobre el
despliegue Kurtosis o un prover C#.

## Identidad de archivos inspeccionados

SHA-256 calculado sobre los archivos locales al registrar esta revisión. El listado
no es un digest completo del patch ni de un binario. Cambios posteriores necesitan
su propia revisión y evidencia.

| Archivo | SHA-256 |
| --- | --- |
| `docs/proposals/l1-origin-and-offline-proving.md` | `c647ef2281337458917e94a731b9848672a914369accfcb1ffdd00d5ce45a959` |
| `docs/proposals/native-system-transactions.md` | `34d9d0e5d3f2ca6e8d7b6a4eaf945e6e9c2b3222f6698d66554f00a7750dda20` |
| `crates/eez-primitives/src/lib.rs` | `ddc57fc4bec5d1dcb71386e9bfd9117c15d199c2a917986b88d1945955b20bfe` |
| `crates/eez-protocol/src/native_source.rs` | `52b4acd8c6e072090ab8eebffcda4541da10e9226e280a63f08e3d5779d66f96` |
| `crates/eez-protocol/src/blob_semantics/reconstruction.rs` | `9483458e3f8e2b6bd878e9fc89950c13104977b4f607e903258987d9f39a46ab` |
| `crates/eez-protocol/src/system_tx.rs` | `b8d52807040321c63dc0d88aa3b1b1acb35353ecb435d893114f59852a456234` |
| `crates/eez-protocol/src/l1_history.rs` | `4942dff5de26026e8d3f501691319d3ce0af0a503acce30baf6d7373fbdf7ebf` |
| `crates/eez-protocol/src/l1_history/context.rs` | `55ef30df39f51b019f97c790eabb2f4d44b6e96126fe9f4e049d458907adced7` |
| `crates/eez-evm/src/evm.rs` | `f178c8882569f9dbd2911aa5dd39ed30ce582227f42cbcd4a071c888fe2614d1` |
| `crates/eez-evm/src/block.rs` | `debf98c59171b338cd48c6989a3c985bdc9dbdaa26b98654c73c2ba107779e1f` |
| `crates/eez-evm/src/history.rs` | `306f78fcb48dd986e0054b7fe1ac0019a50dda1b5fa06f42fce68130a8e31853` |
| `crates/eez-composer/src/local/build.rs` | `4ae95d8e7435b4fae4fbdef437f5a886bf60ee9ef28468a3f39d5a31cd9b217f` |
| `crates/eez-driver/src/sequencer.rs` | `8643e858da95b77da5316a13034d48e2ae9363c7d90b8060900741105cf999d6` |
| `crates/eez-l1/src/derivation/history.rs` | `5c3a7cf87f10ab211cc68233cfc2d37158cef30388b438832e78bc350a60a4a6` |
| `contracts/src/OfflineRollup.sol` | `8cfe7098b64d33ae409a0a84eb71f30481c8c3ee1db042e0c5b22806a1bf7d9e` |
| `contracts/src/L2/FeeVault.sol` | `812c6ca75e493269c9d5a14e44fdc84c2d72211eb1af9402ced846a92dc871d0` |
| `crates/eez-proof-signer/src/service/l1.rs` | `bec6d8405c798ebbc374511de37559199e2408adec0282623b2ece9cd1e1f69c` |
| `crates/eez-proof-signer/src/validate/streaming.rs` | `68e623213da41d5ab381653ed1b61a7e27bc6d8dca2d93f2bc0b3cfe71ddd459` |
| `crates/eez-proof-signer/src/settlement/da.rs` | `0935644d14d9d2ed9243ff0485754a14a037e46b5230c851c119623f54169230` |
| `crates/eez-proof-signer/src/settlement/effect_binding.rs` | `8c2f616e2570f59b6fbb409cc954ab0fd055ee52e1a16fc5e3f90ea37e1b4ac3` |
| `crates/eez-control-rpc/proto/prove.proto` | `e27d7a44a252608cdbf9b738110924cbbb86ff01fbbeeca1365088a4dcbc1887` |
| `crates/eez-control-rpc/src/identity.rs` | `4d2105f97c1e8b17791a54b9128fa27a3b7f6a15af5f1b937d8d2fbdcd056101` |
| `crates/eez-control-rpc/VALIDATION_IDENTITY.md` | `41b370f31d15ebdba75a8a482ab95d36a2d8cbc4447811842a53604f3420c6e0` |
| `crates/eez-control-rpc/conformance/IMPLEMENTING.md` | `288b609cc12f0aa31b4c4603a72a2762558d25ac31753c2ad418855082885ad4` |
| `crates/eez-node-e2e/tests/native_system/treasury.rs` | `bea82d7398e79aea8876e5c41803e89fc73d10061b86526d8864b1b69036b8fd` |
| `crates/eez-testkit/src/lib.rs` | `7a8b5addd89fe2b75ea5deaddb491ae798bcfd46efd3e559b866a21b6bff92bd` |
| `scripts/ci/run-offline-signer.py` | `cd434f77e8a28a0d184641beed5df55c5747fb4bd44fc77ce51eb9e63d8613f5` |

En esa primera fase se ejecutaron lecturas con `git`, `rg`, `sed` y Python para
los hashes; ninguna suite runtime por este revisor. Sólo se añadió este registro
documental. La investigación y adaptación posterior se registra a continuación.

## Corrección posterior: barrera idle de los tests de composición

Durante la suite completa del trabajo principal falló
`chained_interstate::identical_outbound_calls_in_separate_transactions_chain_state`.
La inspección del primer proceso y del código identifica una barrera de test
obsoleta: `open_collection_window` esperaba `eez.composer.sync_slot.invoked`
**antes** de enviar los triggers. El reloj actual acepta un cutoff idle mediante
`eez.sequencer.slot.decision` con `prepare=false, skip_post=true` y continúa
produciendo sin llamar a `compose_sync_slot`. La prueba no llegaba a ejecutar
sus assertions cross-chain.

El log original `artifacts/pr-readiness/native/logs/eez-node-node-2301345-2.log`
contiene 22 decisiones idle aceptadas, cero invocaciones de `compose_sync_slot`
y 280 bloques ordinarios producidos. SHA-256:
`e775ea49e5dfdf076b55981bba1bf3a324c6bd79dd99a5dca190f3e26af7fef0`.
Es evidencia local del primer fallo; este registro conserva su resumen, no el
log completo. No acredita que los escenarios cross-chain posteriores funcionen.

Con autorización del trabajo principal se adaptó únicamente el harness:

- `eez-testkit::signals` declara el nombre existente `SEQUENCER_SLOT_DECISION`.
- `open_collection_window` conserva su cursor fresco y timeout, y exige una
  decisión con `prepare=false` **y** `skip_post=true`. No acepta un simple hint
  del planner ni su fallback por timeout/error. Las assertions de agrupación,
  orden, resultados, receipts, evicción y compromiso L1/L2 permanecen intactas.
- `settlement_policy::conditional_position_paces_late_pool_transactions_and_replays`
  conserva sus assertions de inclusión, pacing y replay, pero vincula el bloque
  al cutoff aceptado con el mismo target/timestamp. Ya no exige una invocación
  Composer `mode=Ordinary` que la producción ordinaria no realiza.

Los cinco tests de `chained_interstate` comparten la barrera corregida, en seis
puntos de llamada:

1. `identical_outbound_calls_in_separate_transactions_chain_state`.
2. `three_order_dependent_inbound_calls_in_one_bundle`.
3. `mixed_direction_state_chain_in_one_slot`.
4. `poison_mid_bundle_leaves_survivors_correct` (dos oleadas).
5. `same_sender_outbound_chain`.

La espera de `settlement_policy::late_sync_settles_due_history_within_the_original_target_window`
exige `mode=Steady` para un intento real de preparación tardía; ese camino aún
invoca Composer y no se modificó sin evidencia de fallo. El canary
`testing/kurtosis/scripts/verify-partial-consumption.py` también utilizaba la
invocación antes de su primera oleada: se comunicó el mismo problema al trabajo
principal para coordinarlo con el agente de Kurtosis. Este revisor no editó ese
script ni su test Python.

Se ejecutó `rustup run 1.95.0 rustfmt --edition 2024 --config skip_children=true`
sobre los tres archivos Rust modificados y `git diff --check` sobre ellos; ambos
terminaron correctamente. **No se ejecutó Cargo ni se atribuye ningún pass nuevo.**
La repetición focalizada y la suite completa quedan a cargo del trabajo principal.

| Archivo del harness después de la adaptación | SHA-256 |
| --- | --- |
| `crates/eez-testkit/src/signals.rs` | `ab18b25d4d899064184747d9974aa5f83e7979af359ea745a31e8c03d99448d7` |
| `crates/eez-node-e2e/tests/chained_interstate.rs` | `0e66a098e56dddc44faf972fd154323379800207f2bb28d73350657097b09d69` |
| `crates/eez-node-e2e/tests/settlement_policy.rs` | `dfea3734823dada3e2ad8fdaaa1ca4c58ebaa0bc45f8b4bee61ea7828db8c50f` |

## Validación posterior con artefactos finales

La [recaptura final](final-fixtures.md), encargada después de la revisión estática,
ya terminó: tres procesos pasan con signer sin red; los fixtures resultantes
validan los **320 tests del signer**, incluidos los **29 transcripts TCP/Stateless
con 24 pruebas esperadas**. Las dos utilidades ignoradas se ejecutaron por separado.
No se modificó código ni se relajaron assertions para regenerar el corpus.

El registro conserva el primer fallo del procedimiento: un timestamp preservado
dejó un génesis antiguo incrustado en el binario de test. Se comprobó su contenido,
se reconstruyó con el input actual y se repitió la misma suite. Los hashes y límites
de procedencia están en [la evidencia](final-fixture-evidence.json); los logs y
binarios completos son local-only. La suite completa de procesos, el enclave live
y las imágenes shipping siguen siendo tracks separados del trabajo principal;
estas capturas no anticipan sus resultados ni acreditan integración con Nethermind.
