# Revisión de conformidad con specs-draft y fronteras de hard fork

**RollupX no cumple literalmente todo el draft.** La rama actual implementa un perfil
distinto, aprobado para redes nuevas. Varias diferencias anteriores ya están resueltas:
tipo nativo `0x45`, SYSTEM `ffff…fffe`, ausencia de nonce de cuenta en esas transacciones,
base fee al recolector y una única API `prove.v2`. Permanecen diferencias deliberadas
de consenso y garantías del draft que no ofrecemos. Esta revisión no propone volver
al core antiguo ni añadir compatibilidad legacy.

No se puede declarar toda la validación verde: esta revisión reproduce un test de
génesis desactualizado y un fallo de compilación aislada de `eez-driver` sin features.
Los casos ejecutados con la configuración de producción del driver sí pasan.

## Identidad y alcance

| Campo | Valor |
| --- | --- |
| Fecha / estado | 2026-10-24 UTC. Revisión estática y comprobaciones dirigidas terminadas; hallazgos abiertos descritos abajo. No es certificación de seguridad ni de despliegue. |
| Fork / worktree | `eduadiez/rollupX`, rama `codex/l1-history-offline-proving`, `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`. Se conservó todo el trabajo previo. |
| Fuente local | HEAD `3e23955dc477c9e675364776f118b1cecda75774` **más cambios locales**, identificados por [source-sha256.json](source-sha256.json). No se revisó sólo el commit publicado. |
| Protocolo | Gitlink y checkout limpios en [`84a5b5207880f192a6a5896cbfe0bac5249db47f`](https://github.com/eez-association/eez-core-protocol/commit/84a5b5207880f192a6a5896cbfe0bac5249db47f), línea `feature/e2e_edge_cases`. No se sustituyó por upstream main. |
| Draft | [`specs-draft@2c6d346bea0b3736f7453c35174291495694d1fc`](https://github.com/eez-association/specs-draft/commit/2c6d346bea0b3736f7453c35174291495694d1fc), main/HEAD confirmado con `git ls-remote`. Commit de 2026-10-06; sin cambios respecto a la revisión del día 23. |
| Entorno | Rust 1.95.0, dependencias bloqueadas, Foundry 1.7.1. Reth v2.7.0 `3d592ece6de8c4559987416a544fc215fd6d6921`; Stateless `0bda6b200c1eb4689c147c080c3c248b50be2ea8`, según Cargo.lock. |
| Contrato aceptado | [Historia y proving offline][PROP], [transacciones nativas y tesorería][NATIVE], [historia L1][HISTORY-SPEC], [RPC][RPC], [signer][SIGNER] y especificaciones del core fijado. |
| Evidencia | [checks.json](checks.json): comandos, resultados, diagnósticos breves y hashes. [spec-inventory.json](spec-inventory.json): documentos remotos y cobertura. Logs/binaries grandes sólo locales, bajo `artifacts/specs-review-2026-10-24/`. |

En el cierre posterior del PR, `source-sha256.json` pasó a identificar cada valor
con la clave `sha256`, conservando los 1.725 digests de este snapshot. Así las rutas
que contienen `api` o `key` no parecen nombres de credenciales al escáner. Las
correcciones y nuevas ejecuciones se registran en el [cierre del PR](../2026-10-24-pr-readiness/README.md);
no sustituyen retroactivamente los resultados de esta revisión.

Se revisaron los 28 documentos Markdown de `docs/`: capa EEZ, Rollup0, apéndices
y variante Gnosis. Se contrastaron con productores, consumidores y tests del fork.
Los documentos auxiliares retirados no se convierten en nuevos requisitos.
El apéndice E compara otros commits de agosto; G es informativo. Las notas
`companion-docs/IMPLEMENTATION_NOTES.md` están explícitamente superseded.

El propio [apéndice D][D] declara un blocker de producción: sus capítulos EEZ aún
contienen el fold antiguo y no fijan la capa física completa. Un MUST en este
borrador no anula el contrato aceptado del fork. Por decisión del propietario,
**el core fijado prevalece cuando ambos discrepan**. El estado de una propuesta
no se deduce de que exista código o un test.

Esta revisión actualiza la [matriz del 23 de octubre][OLD]. Conserva sus 58 IDs
para seguimiento, pero no sus estados ni sus cifras de conformidad. No se editaron
fuentes, tests, configuración operativa, submódulos ni servicios para esta auditoría.

## Qué cambios exigirían un hard fork después del lanzamiento

Aquí «HF de ejecución» significa que los mismos inputs pueden producir otra raíz,
hash o decisión de validez L2. «Activación de derivación» también es un cambio de
consenso del rollup: los nodos pueden discrepar sobre la cadena canónica aunque
el intérprete EVM no cambie. Un upgrade autorizado de un contrato y una nueva
regla del cliente no son equivalentes.

| Diferencia actual | Consecuencia de adoptar literalmente el draft en una red existente | Decisión vigente |
| --- | --- | --- |
| **Envelope y sourceHash**: ambos usan `0x45`, pero localmente es versión 1 y el draft exige 0; dominio, índices y call hash difieren. | **HF de ejecución y derivación**: bytes/hash de tx, receipt trie, identificación de ocurrencias y admisión. Nuestro decoder rechaza versión 0. No hay interoperabilidad por compartir el byte `0x45`. | Conservar native-v1, que representa también outbound, grupos e historia. |
| **Limpieza de saldos SYSTEM/EEZL2** al principio del bloque, frente a conservación de residuos y neutralidad por inbound. | **HF de ejecución**: cambian saldos y state roots, incluyendo el tratamiento de ETH no solicitado. | Conservar nuestra limpieza y contabilidad del core. |
| **Origen L1 grabado, delay y PREVRANDAO**, frente a último header estrictamente anterior al Sync. | **HF de ejecución y derivación**: cambian RANDAO del header/opcode, estado de historia y tx obligatoria. | Conservar delay, repetición válida y reloj independiente de L1. |
| **Gramática DA V0**: tokens compactos de conteos frente a un uvarint por bloque. | **Activación de derivación** obligatoria. Los bytes publicados se interpretan de forma distinta; no basta reemplazar un decoder sin frontera de formato. Puede no alterar el EVM si se reconstruyen los mismos bloques. | Conservar nuestro V0 compacto. Incompatibilidad reproducida con el vector del draft. |
| **Calendario de ejecución**: Osaka fijo frente a futuras activaciones siguiendo Ethereum mainnet. | Cada futura modificación de opcodes/gas/headers exige **HF de ejecución**. No hay hoy un fork posterior concreto que esta revisión ordene activar. | Calendario propio. L1 y L2 tienen calendarios distintos. |
| **Perfil core y llamadas admitidas**: hashes/selectores/tablas modernos, outbound/static/grupos, frente al perfil antiguo inbound-only. | Cambiar sólo un contrato mediante su upgrade puede ser una transición EVM ordinaria. Revertir también las reglas de hash/reconstrucción/admisión que hoy están en los clientes exigiría **activación de derivación y, donde cambie validez native, HF de ejecución**. | El core fijado prevalece; no degradarlo al texto antiguo. |

Otras constantes autenticadas —génesis, grid, direcciones especiales o el
`feeCollector` de ejecución— tampoco se cambian retroactivamente con una opción
operativa. Eso no es una divergencia que haya que corregir ahora. **Cambiar el
recipient de FeeVault mediante `setRecipient` sí es una transacción ordinaria:
no cambia el collector, el perfil ni las reglas de consenso, y no requiere HF.**

La base fee al collector ya está implementada: no permanece el gap anterior de
quemarla como Ethereum. EIP-7702 ordinario en L2 también está soportado; su soporte
en la cola de triggers cross-chain es otra capacidad.

Para las redes nuevas autorizadas, las reglas elegidas se fijan desde génesis.
No se necesita un hard fork para desplegarlas. Los impactos anteriores explican
el coste de cambiar de decisión **después**, no proponen una migración ahora.

## Hallazgos reproducidos de la rama

### R01 — test de génesis incompleto tras añadir FeeVault

`production_base_genesis_allocates_no_eth_and_keeps_required_predeploys`, en
[eez-evm/src/tests.rs][EVM-TESTS], falla al comparar la lista exacta: espera cinco
cuentas y la génesis tiene seis, incluyendo FeeVault `0xeeee…0078`.

El bucle anterior de balances cero pasa. El fallo no prueba ETH sin respaldo,
ni un contrato inesperado: FeeVault pertenece a la tesorería aceptada. Sin embargo,
las comprobaciones posteriores de ese test no llegan a ejecutarse y la suite del
paquete no está verde. Acción: actualizar la allowlist exacta al conjunto aprobado
y conservar las comprobaciones de saldos, proxy y código/estado de los predeploys.
No se corrigió durante esta revisión de sólo lectura de implementación.

### R02 — dependencia obligatoria marcada opcional en eez-driver

`cargo test --locked --offline -p eez-driver --lib sequencer::tests` no compila:
nueve errores E0433. [Cargo.toml][DRIVER-CARGO] marca `eez-primitives` opcional y
deja `default = []`, pero `block_committer.rs`, `sequencer.rs` y `slot.rs` lo usan
sin condicionar esas rutas a la feature. No es un fallo del comportamiento del reloj.

Composer activa `witness,derivation` y node activa `witness`. Repitiendo la selección
de tests con `--features witness,derivation`, **7 pasan**, incluyendo fuente L1
cerrada, preparación bloqueada, decisiones obsoletas, slots omitidos y avance
más allá del límite especulativo. Eso no arregla el paquete por defecto.
Acción: declarar la dependencia según su uso real y validar también compilación
aislada. La unificación de features de un workspace puede ocultar este defecto.

### R03 — «V0» local y remoto no son el mismo formato

Se enlazó un [probe mínimo](v0_probe.rs) contra el decoder Rust actual. El vector 1
de D.4.4 tiene 31 bytes y devuelve `Error::Counts`; los mismos seis bloques vacíos
en el codec local ocupan 26 bytes y se decodifican como seis bloques.
[v0-probe.json](v0-probe.json) conserva inputs, comando y hashes.

Es una diferencia ya aceptada, no una regresión. La prueba verifica **gramática
del payload**, no una candidata completa: esos bloques no acreditan por sí solos
las importaciones obligatorias de historia, headers, ejecución ni settlement.

## Matriz de requisitos y evidencia

Estados: **coincide** dentro del alcance indicado; **diferencia aceptada**; **core
prevalece**; **pendiente** para ofrecer la garantía remota; **no demostrado**;
**no aplicable**. No son porcentajes de conformidad. «Test» en las filas identifica
la cobertura pertinente; la sección de ejecución distingue qué se corrió ahora,
qué es evidencia anterior de la misma fuente y qué sólo se inspeccionó.

### Capa EEZ compartida

Referencias remotas: [arquitectura][E01], [EVM/proxies][E02], [ejecución][E03],
[proving][E04], [wire][E05]. Contrato local: [core][CORE], [entries][ENTRIES],
[static][STATIC], [caveats][CAVEATS], [multiprover][MULTI], [blobs][BLOB-SPEC].

| ID / estado | Requisito remoto y resultado actual | Integración / evidencia / efecto |
| --- | --- | --- |
| C01 · No cerrado en draft | D requiere pin completo, `getCustomData` y vectores físicos. Localmente el core y el perfil están fijados; el draft aún mezcla generaciones. | [ABI][ABI], [manager][MANAGER], oráculos de public inputs/rolling/KZG. No inventar un manifiesto remoto ni rebajar el pin. |
| C02 · Core prevalece | Draft: `uint256/int256`, `LookupCall` y tablas antiguas. Pin: `uint64/int192`, `ExecutionEntry.success`, arrays por frame, static entries y roots esperadas; selector `postAndVerifyBatch` distinto. | Productores/consumidores usan [ABI][ABI] y codecs actuales. Un port literal afecta contratos, DA y reconstrucción; no basta cambiar Solidity. |
| C03 · Core prevalece | Call hash actual incluye `isStatic`, anchuras y gas; rolling hash y posiciones están ligados a roots/contexto. No coincide con el hash antiguo de seis campos. | [native_source][SOURCE], [reconstrucción][RECON], vectores independientes. Ahora ese call hash entra también en sourceHash nativo: cambiarlo puede cambiar tx hashes L2. |
| C04 · Core prevalece | CREATE2 usa salt de rollup uint64 + dirección, initcode/contexto del pin; el draft usa uint256 y otra construcción. | Factories/proxies del core y ABI. Cambian direcciones/identidad contractual, no el opcode CREATE2 de la EVM. Si el cliente depende de ellas, actualizar también sus reglas. |
| C05 · Core prevalece | Fold antiguo `getTimestampAndBlockHash` frente a `getCustomData`, contextos/profile y hash actual; no existe el antiguo `crossProofSystemInteractions` de ese ABI. | [manager][MANAGER], signer settlement y vectores de digest. Upgrade de política L1 no implica automáticamente HF EVM; requiere acuerdo de todas las partes. |
| C06 · Coincide en invariante; core prevalece en ABI | Compromiso canónico es **hash de bloque**, aunque se llame stateRoot; eventos y layouts difieren del draft viejo. | [scanner][SCAN], [signer][SIGNER], replay/process previo. Ni altura coincidente ni receipt por sí solos completan settlement. |
| C07 · Core prevalece | Mutable/static, orden, reentrancia y callbacks siguen entries y posiciones del pin, no el antiguo modelo de lookups. El perfil permite grupos/outbound. | [RECON], signer y core tests. Conservar grupos completos, rollback y posiciones originales; los caveats del core siguen aplicando. |
| C08 · Coincide con límites de confianza | Managers/proof systems autorizan efectos; un follower detecta ejecución L2 inválida, pero no revierte efectos L1 ya aceptados. Custodia y gobernanza son fronteras de confianza. | [MULTI], [CAVEATS], [SIGNER]. No se demuestra seguridad frente a manager/admin/umbral comprometido. No es una nueva garantía por eliminar RPC. |
| C09 · Core prevalece; borde ahora verificado | El draft exige revert universal para DELEGATECALL/CALLCODE al proxy. El pin impide dispatch cross-chain no autorizado, pero ciertos forwards/self-probes pueden completar como llamadas EVM ordinarias. | [CrossChainProxyPolicy][PROXY-TESTS]: **10 tests pasan ahora**. El borde incierto de la revisión anterior queda precisado; no se promete revert literal para toda forma de llamada. |

### Ejecución, bloques y transacciones nativas

Referencias remotas: [EVM/system tx][R03], [bloques][R04], [ejecución][R05],
[economía][R11], [D][D], [F][F].

| ID / estado | Requisito remoto y resultado actual | Integración / evidencia / efecto |
| --- | --- | --- |
| X01 · Coincide en grid; despliegue por fijar | Ethereum 2 s/seis posiciones y Chiado de desarrollo 1 s/cinco; posiciones no desaparecen por slots L1 omitidos. Génesis producción sin ETH arbitrario. | [timing][TIMING], génesis/render, driver. Test de balances pasa antes del fallo R01; no equivale a verificar un deployment. |
| X02 · Coincide en perfil inspeccionado | Osaka, 30M, nonce/difficulty cero, ommers/withdrawals vacíos, beacon root cero, requests vacíos, no blobs L2; EIP-2935 sin tareas beacon de L1. | [reglas][L2], [profile][PROFILE], payload builder y Stateless/Reth fijados. Límite Osaka RLP 8 MiB presente en construcción y validación. Tests EVM y protocolo; no se reejecutó toda la matriz EIP. |
| X03 · Diferencia aceptada | No se sigue automáticamente el calendario mainnet de futuros forks: perfil Osaka, rechazo de posteriores no soportados. | [PROFILE], [L2], identidad de validación. Activar otra ejecución requiere HF explícito; el calendario L1 autenticado no activa por sí mismo un fork L2. |
| X04 · Parcial; diferencia aceptada | Ya usa `0x45`, sender `ffff…fffe`, siete campos RLP, sin firma/nonce en wire; **versión 1**, no 0. Destinos locales incluyen historia. Chain ID se representa en u64. | [primitives][PRIMITIVES], [native tests][NATIVE-TESTS]. Versión 0/trailing bytes rechazados; u64 admite nuestras redes, no todo uint256 del esquema remoto. Cambiar bytes/admisión exige HF. |
| X05 · Diferencia aceptada | Source identity sigue ocurrencias ordenadas, no hash del carrier ni resultado. Pero dominio versión 1, call hash del pin y `topLevelCallIndex` adicional hacen otro hash incluso para inbound simple. Outbound e historia tienen dominios propios. | [SOURCE], [RECON], [system tx][SYSTEM-TX], vectores nativos independientes. Mantiene índices originales tras skips y callbacks inline. Coincide la idea, no el preimage exacto del draft. |
| X06 · Coincide estructuralmente; versión distinta | RPC expone type/sourceHash/version y placeholders cero de nonce/firma; receipt typed estándar. Bytes native siguen el perfil 1. | [NATIVE-TESTS]: RPC, receipt, Engine, almacenamiento y round-trip. No aceptar v0 mediante alias o normalización. Soporte de otro cliente requiere esos mismos bytes. |
| X07 · Coincide con extensión | Native no incrementa nonce de SYSTEM, no paga fee y sí consume gas real del bloque. Cap cross-chain `2^24`; presupuesto exacto acotado por gas restante. Historia tiene cap propio 2M. | [SYSTEM-TX], EVM/receipts y tests de gas/no nonce/rechazo. No hacer gratis el trabajo de historia ni confundir caps de admisión con reglas de consenso. |
| X08 · Diferencia aceptada | Se vacían balances SYSTEM/EEZL2 al inicio del bloque y hay inventario agrupado dentro de él. No se preserva todo ETH no solicitado ni se exige la neutralidad remota por cada inbound. | [block lifecycle][BLOCK], [EVM-TESTS], core. Preludio incluido en checkpoints/replay; modificarlo cambia balances/state root y requiere HF. No confundir esta limpieza con quemar base fees. |
| X09 · Coincide; tesorería añadida | Base fee final al collector después de cada tx ordinaria, prioridad al beneficiary, génesis 1 gwei y parámetros 2/50. Renderer instala FeeVault; owner puede cambiar recipient. | [EVM][EVM], [FeeVault][VAULT], tests de revert/refund/floor/visibilidad/aliasing y **11 tests de vault** pasan. No se ejecuta código del collector al acreditar. Recipient mutable no requiere HF; cambiar collector sí cambia ejecución. |
| X10 · Diferencia aceptada | Draft `P(T)` = último L1 canónico con timestamp **< T**, RANDAO en el siguiente intervalo. Local: header seleccionado con timestamp **≤ cutoff**, grabado en update diferido; no obligación de escoger el más reciente. | [HISTORY-SPEC], [history protocol][HISTORY], EVM history. Repetir origen es válido aunque después llegue evidencia nueva. Header PREVRANDAO y opcode usan el mismo valor. Cambiar selección altera consenso. |
| X11 · Extensión aceptada | Native history al principio del bloque de actualización; no extraData ni llamada oculta. Hashes consecutivos desde checkpoint, hasta 64 por import, repeat explícito, ejecución antes de apps. | [history contract][HISTORY-CONTRACT], [HISTORY], [history EVM][HISTORY-EVM]. Se rechazan omisiones, duplicados, calldata/gas/runtime/RANDAO erróneos y se comprueba rollback. Almacena bloques L1 producidos, no slots vacíos. |
| X12 · Core prevalece; dirección propia | EEZL2 y proxy predeploy siguen direcciones/bytecode del pin y génesis local. No se adopta otra dirección reservada del borrador. | Génesis, renderer, [PROFILE], core. Upgrade autorizado de implementación es distinto de cambiar destino nativo validado por clientes; lo segundo necesita activación de consenso. |
| X13 · Extensión aceptada | Draft V1: una llamada inbound mutable por trigger, sin outbound/static/nesting, fallo terminal sin tx L2. Local: grupos, callbacks/static, outbound y resultados de fallo del core. | [RECON], signer semantics; procesos previos sobre esta fuente cubren caught revert y outbound static cambiante. No aplicar restricciones antiguas que rompan esas rutas. |
| X14 · Coincide en objetivo; política local | Reloj ordinario no espera datos L1 ni pruebas. Recupera posiciones intermedias, repite origen y separa candidatos condicionados/settlement. | [sequencer][CLOCK]: **7 tests pasan con features de producción**, incluyendo ausencia L1 y trabajo bloqueado. Reemplazos de settlement acotados no limitan número de bloques ordinarios. R02 impide test default. |

La ausencia de L1 fresco no implica detener el reloj ordinario, pero tampoco
garantiza safe-head/settlement indefinidos sin L1. Reorgs canónicas pueden sustituir
bloques unsafe. El delay reduce presión de latencia y exposición a reorgs; **no es
prueba de finalidad**, ni un límite máximo de antigüedad del origen. Slots omitidos
no tienen hash que inventar. Un header recibido tarde puede incorporarse en una
actualización futura sin reescribir la decisión ya grabada sólo por llegar tarde.

### DA y derivación de candidatos

Referencias remotas: [DA][R07], [D][D], [G informativo][G].

| ID / estado | Requisito remoto y resultado actual | Integración / evidencia / efecto |
| --- | --- | --- |
| D01 · Coincide con core fijado | Blobs nativos, stream lógico/físico del pin, framing/padding y orden de blobs autenticados; no sustituirlos por calldata auxiliar. | Core BLOB_FORMAT, `eez-blob-codec`, signer DA y scanner. Vectores KZG/rolling anteriores de esta fuente; no se repitieron ahora. |
| D02 · Diferencia aceptada incompatible | Counts remotos: uvarint por bloque. Local: `00` vacío aislado, tokens `c0..fe` para 2..64 vacíos, `ff`+varint para mayores; no vacíos tienen codificación propia. | [counts codec][COUNTS], **probe R03**, 16 tests payload pasan. Evita ambigüedad/trailing/noncanonical. Máximo local 16383 tx/bloque no es el entero arbitrario remoto. Cambio requiere activación DA. |
| D03 · Coincide salvo D02 | Payload directo no comprimido, sin contenedor RLP/zstd; runs maximales de beneficiary/extraData, extraData ≤32, tx canónicas, lengths mínimos, sin trailing. | `eez-payload-codec`, [RECON], signer. No inferir equivalencia wire completa de estas coincidencias. |
| D04 · Extensión aceptada | El stream local representa grupos, static/fallos y outbound, no sólo brackets de una llamada inbound. History native se conserva en el prefijo ordinario. | [RECON], codecs del pin, signer/deriver. Exigir el antiguo subconjunto descarta comportamiento aceptado y requiere coordinar semántica de derivación y pruebas. |
| D05 · Coincide en compromiso; perfil ampliado | Rango completo, bloques vacíos, prefijo puro, variantes al mismo número y checkpoints de hash; aceptar sólo consumo canónico demostrado, incluso parcial. | [SCAN], composer prefix/settlement, signer state chain. Procesos previos incluyen replay independiente. Un receipt o altura final no sustituye la selección de variante. |
| D06 · Coincide con cuotas explícitas | Parser y trabajo acotados; ningún bloque se salta por ser vacío. Cuota de headers de witness no equivale a longitud máxima de historia de la red. | [history fetch][HISTORY-FETCH], signer limits, tests de límites. Bridge máximo operativo 131072 headers; import máximo **consenso** 64. Catch-up puede partirse; no truncar evidencia para que quepa. |
| D07 · Local verificado; remoto incompleto | El draft exige vectores completos del core elegido, todavía sin pin definitivo. Local tiene ABI/public inputs, native, payload y blobs del pin. | 27 encodings/13 relaciones nativas en registro anterior; **29 transcripts/fixtures comprobados ahora** por checker. No prueban interoperabilidad con el wire remoto ni con un Nethermind aún no ejecutado. |

Un catch-up mayor que la ventana `BLOCKHASH` puede usar partes y ancla histórica
reciente. El witness necesita la cadena de headers que une el estado/origen previo
con el ancla autenticada, **incluidos headers intermedios que ninguna aplicación
leyó**. Partir imports en 64 no elimina esa obligación ni la disponibilidad de los
headers del puente. «Cuatro días» no es un umbral de consenso ni una capacidad
medida aquí; no se ejecutó un benchmark de cuatro días.

### Settlement, triggers y política L1

Referencias remotas: [composer][R06], [DA][R07], [settlement][R08], [L1→L2][R09], [D][D].

| ID / estado | Requisito remoto y resultado actual | Integración / evidencia / efecto |
| --- | --- | --- |
| S01 · Diferencia aceptada | Draft liga dominio a wrapper activo/contexto transitorio. Manager local liga registry, rollup, cadenas, root aceptado, perfil/reglas y modo de ancla, sin wrapper. | [MANAGER], [history context][CONTEXT]. No son el mismo digest ni idéntica política de autorización. Adoptarlo exige contratos L1 y adaptación de consumidores. |
| S02 · Diferencia aceptada | Draft exige contexto live de inclusión incluso en catch-up. Local admite actual-parent y ancla histórica explícita reciente. | Tests manager/header/context pasan; histórica edad 1..256 con hash no cero, actual parent con timestamp de inclusión. Misma raíz/perfil/reglas; una proof histórica no afirma futuro bloque exacto de inclusión. |
| S03 · Pendiente, aplazado | No existe wrapper del draft con prefix exacto, blobs completos, `H[0]`, una activación exitosa por bloque y activación atómica de wrapper. | [MANAGER], scanner/submitter inspeccionados. Añadirlo no es automáticamente HF EVM, pero **scanner lee calldata exterior de postAndVerifyBatch**: wrapper sin adaptar derivación rompe follower. |
| S04 · Pendiente / core prevalece | No hay guard universal por rollup para prohibir blob triggers ni latch transitorio de ≤1 consumo exitoso por carrier. Ingress propio rechaza blobs, pero eso no restringe todo L1 permissionless. | Core vigente, ingress, [SCAN]. Si se desea esa garantía, debe imponerse donde terceros no puedan eludirla; filtro del composer no basta. Contrato/política y derivación coordinados. |
| S05 · Diferencia con mitigaciones | No existe wrapper que autentique todo dispatch/prefix según D. Pin sí tiene guards de leading entries/all-immediate-failed/out-of-gas, roots esperadas y contexto. | [SIGNER], core errors/tests. No repetir como exploit actual la afirmación antigua «se puede saltar anchor» sin considerar esos guards; tampoco prometer que todo conteo inmediato queda firmado. |
| S06 · Diferencia aceptada | Publicación privada a target L1 exacto y prefixes agrupados; local permite companions en `revertingTxHashes` y consumo parcial, más amplio que todos-los-outer-triggers-éxito del draft. | composer bundle/submission y [SCAN]. Identidad de llamada no equivale a identidad del carrier; no hay fallback automático a mempool público. Efectos L1 originales no son replay offline del signer. |
| S07 · Coincide en invariante; política local | Proof o aceptación de relay no implica inclusión; sólo avance canónico validado resuelve ownership. Retry/miss recompone; hasta cuatro reemplazos en padre sin cambios, conservando historial. | [settlement policy][POLICY], [RPC], queues y driver. Timers no autorizan completar/evict; límite no detiene reloj ordinario. Tests específicos de toda la cola no se reejecutaron ahora. |
| S08 · Pendiente de servicio, aplazado | Ethereum permite triggers ordinarios EIP-7702; la cola cross-chain local aún no ofrece esa ruta completa. **L2 ordinario 7702 sí está habilitado por Osaka.** | Ingress/simulación de triggers y perfil EVM. Añadir apoyo de servicio no requiere por sí solo HF L2 si no se cambia semántica de native/core; hay que probar authorization-list/nonce y prefijos. |

### Prover y frontera de confianza

Referencias remotas: [settlement][R08], [L1→L2][R09], [following][R10], [D][D].

| ID / estado | Requisito remoto y resultado actual | Integración / evidencia / efecto |
| --- | --- | --- |
| P01 · Limitado a M=1; generalización aplazada | Draft N=floor(2M/3)+1 y gobernanza atómica. Signer admite una proof system/rollup; manager genérico permite threshold configurable, incluso cero o superior al conjunto. | [MANAGER], [post_batch][POST-BATCH]. No afirmar quorum 2/3+1 universal ni seguridad independiente de configuración. Generalizar agregación es L1/servicio, no automáticamente HF EVM. |
| P02 · Coincide | Attestation ECDSA secp256k1 de digest raw, 65 bytes, low-s, v27/28, clave separada; sin EIP-191. | Signer, verifier del pin y vectores. Es prueba firmada basada en confianza, no prueba ZK ni garantía de inclusión/finalidad. |
| P03 · Diferencia aceptada de garantía | Signer ejecuta L2 y valida semántica/evidencias, pero **no recibe ni simula las transacciones L1 originales ni todos sus bundles prefijo** como exige el draft. | [SIGNER] §§1/5/11, [post_batch][POST-BATCH], Stateless. Comprobaciones on-chain de rolling/static/contexto no equivalen a ese replay independiente completo. Añadirlo requiere evidencia/validación; no nace por conectar RPC. |
| P04 · Elección permitida; despliegue no auditado | Transporte de prover no es consenso. Un único servicio `prove.v2.Prover`, Capabilities y ProveStream; un `Profile`/`OfflineProfile`, identidad versión 1. | [RPC], [identity][IDENTITY]. Sin prove.v1 ni ProfileV3 activos. Cambiar rutas/nombres no es HF; confidencialidad/autenticación del despliegue no se verificó y no se deduce de gRPC. |
| P05 · Preparado; interoperabilidad no demostrada | Contrato language-neutral y fixtures permiten implementar C#/Nethermind; no se ejecutó un segundo motor aquí. | [conformance][CONFORMANCE], checker **29 vectores**. Comparar realmente tx/receipts/state/block roots, source hashes, rejects y digest entre implementaciones sigue pendiente. No requiere inventar RPC v3. |
| P06 · Coincide con garantía condicional local | Prover sin RPC verifica ejecución desde witness, headers enlazados al ancla, historia/imports/RANDAO, identidad y recomputa public inputs. Manager autentica ancla/root/perfil al verificar en L1. | [CONTEXT], [HISTORY], [MANAGER], signer. Header suministrado no es automáticamente canónico; la firma es válida bajo el contexto que L1 acepta. No prueba por sí sola futuro settlement, frescura maximal ni finalidad. |

`getCustomData` no ejecuta todos los headers ni todos los bloques L2. Proporciona
el contexto autenticado que el digest compromete; el prover comprueba la cadena
y la ejecución que llevan a ese contexto. En modo actual-parent, L1 aporta su
padre y timestamp; en modo histórico, comprueba un hash accesible en su ventana.
El puente puede llevar headers mucho más antiguos en el witness. Esto permite
al **prover** funcionar sin RPC L1/L2; el sistema que construye el witness y publica
settlement sigue necesitando obtener datos e interactuar con L1.

El core conserva caveats de callbacks/reintentos, observabilidad de gas y efectos
de reverts capturados. Se inspeccionaron como límites del contrato vigente, no se
reprodujo un exploit nuevo ni se promete composabilidad arbitraria.

### Operación, recuperación y requisitos abiertos

Referencias remotas: [arquitectura][R02], [following][R10], [economía][R11],
[open issues][R12], [A][A], [B][B], [C][C], [Gnosis][GN].

| ID / estado | Requisito remoto y resultado actual | Integración / evidencia / efecto |
| --- | --- | --- |
| O01 · Pendiente, aplazado | No se implementa anuncio unsafe firmado con el envelope/dominio de D.6. Follower puede recibir unsafe por RPC configurado. | [unsafe head][UNSAFE]. Añadir autenticación P2P no es HF EVM ni da autoridad canónica al productor en Rollup0 abierto. No confundir con autorización de productores en variante Gnosis. |
| O02 · Coincide con extensión local | Engine Osaka y construcción native en proceso, campos L1-beacon omitidos/vacíos conforme a perfil; atributos propios para historia. | Engine/primitives, payload builder, tests de round-trip. Un builder externo debe acordar esos inputs; API interop distinta de validez del bloque. |
| O03 · No demostrado; aplazado | No hay evidencia end-to-end de bootstrap eth/snap desde checkpoint finalizado exacto con blobs originales ya expirados y sin productor original. | Follower/deriver y Reth inspeccionados. Heredar infraestructura P2P no demuestra el procedimiento; requiere prueba de recuperación de cuerpos/estado/checkpoint. Operación/almacenamiento. |
| O04 · Límite operativo | Reorg watcher tiene profundidad por defecto 62 y error `ReorgTooDeep`; no garantiza recuperación automática de toda reorg no finalizada desde checkpoint. | [watcher][WATCHER]. Ampliar almacenamiento/recuperación no cambia consenso. No se promete que una reorg canónica profunda sea ignorada para mantener uptime. Conflicto finalizado requiere recuperación explícita. |
| O05 · Implementado con capacidad no medida | Admission usa reglas L1 de gas/blobs y presupuestos; costes y cotizaciones no sustituyen validación final. No se midió throughput/catch-up/builder en vivo. | Composer costs/admission, perfiles L1, [POLICY]. Cap operativo no debe transformarse en nuevo límite de validez por conveniencia. |
| O06 · Coincide como configuración, no deployment | Anchoring periódico SHOULD de 15 min puede configurarse; ejemplos tienen 900000 ms, junto a otras políticas. Métricas y lag distinguen aceptación/progreso. | [POLICY], configuración y monitoring. No se leyó configuración privada en vivo. Cambiar policy no es HF ni condición de validez de bloques. |
| O07 · Coincide en best effort; límite explícito | Pending requests y retry counters en memoria; restart reconcilia canónico, puede perder speculative y exigir resubmit. | RPC/queue contract. Persistir DB de bloques no da recuperación durable de cola. Añadir esa propiedad es operación/almacenamiento. |
| N01 · Sin contrato cerrado | Withdrawal ETH asíncrona del draft sigue sin autorización/plazo/payout/anti-replay completos. Outbound síncrono y FeeVault local no la implementan. | Core/propuestas actuales. No clasificar como HF cierto una implementación aún no diseñada; depende de si introduce reglas nuevas de cliente o sólo contratos. |
| N02 · No aplicable como MUST V1 | Force inclusion, TEE, slashing y optimizaciones futuras están aplazadas. | No se prometen en el fork. Si se diseñan después, clasificar el mecanismo concreto; no añadirlas por esta auditoría. |
| N03 · Falta manifiesto de producción remoto | Draft no fija todos los chain IDs, génesis, reference, membresía/builders/collector. Nuestro renderer y fixtures no son un despliegue aprobado de Rollup0. | [PROFILE], manager/identity y runbooks. Publicar manifiesto y vectores del perfil elegido antes del lanzamiento; no crear defaults para rellenar decisiones ajenas. |
| G01 · No aplicable | Variante Gnosis migra cadena existente preservando estado; esta rama prepara redes nuevas. | No importar compatibilidad/migración legacy. Si se eligiera ese producto, sí habría transición de consenso/estado y contratos L1. |
| G02 · No aplicable | Gnosis exige productores autorizados y prueba de autoría por variantes; no existe esa allowlist en perfil RollupX abierto. | No confundir firma prover con firma productor. Implementarlo sería cambio de autorización/canonicalidad, con ubicación por definir. |
| G03 · No aplicable; diseño parcial | Gnosis usa xDAI, collector propio y bridge DAI; RollupX nuevo usa ETH y tesorería local. Reglas de adapter aún abiertas en draft. | Migración monetaria/de génesis y L1 sería otro proyecto; no acreditar contabilidad DAI con tests ETH. |
| G04 · No aplicable por dependencia | Usar Chiado/reth_gnosis como **L1** no convierte a L2 en la Gnosis Chain migrada. | Config/reglas L1 versus perfil L2 se mantienen separados en witness, composer y signer. |

## Cobertura de documentos remotos

El inventario conserva SHA-256 y asignación de **cada uno de los 28 documentos**.
El agrupamiento de abajo facilita navegación; no excluye las partes normativas
en apéndices D/F ni convierte los textos informativos en contrato aceptado.

| Documento o grupo | Filas / disposición |
| --- | --- |
| Índices generales, EEZ y Rollup0 | C01, N03: Draft, jerarquía y alcance. |
| EEZ 01–05 | C01–C09, P01–P03: arquitectura, EVM/proxies, ejecución, proving y wire; core prevalece. |
| Rollup0 01–02 | X01, X13–X14, P04–P05, O01–O03: roles, neutralidad y transporte. |
| Rollup0 03–04 | X01–X14, O01–O02: EVM, native, grid, headers, RANDAO, unsafe. |
| Rollup0 05–06 | C07, D04–D05, S01–S08, P03, O06–O07: ejecución, candidatos y ownership. |
| Rollup0 07–10 | D01–D07, S01–S08, P01–P06, O01–O04: DA, settlement, L1→L2 y derivación. |
| Rollup0 11–12 | X07–X09, O05–O07, N01–N03: economía y límites/open issues. |
| A–C | Parámetros/vectores y decisiones abiertas: C01, X01–X09, D07, P01, O05–O06, N01–N03. B no establece un benchmark de esta rama. |
| D / F | Wire y sistema nativo normativos: X04–X11, D01–D07, S01–S05, P01–P02, O01. |
| E / G | Informativos. E audita otros commits; G explica DA y remite a D. No importar diagnósticos antiguos como fallos recién reproducidos. |
| Gnosis | G01–G04: otra variante, no consecuencia implícita de usar Chiado. |

## Comprobaciones ejecutadas y límites

Todos los comandos de código se ejecutaron en el worktree indicado. Para Cargo
se usaron `TMPDIR=$PWD/artifacts/tmp`, `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, PATH
con LLVM y `bwrap --bind / / --dev /dev --bind "$TMPDIR" /tmp --chdir "$PWD"`
por la cuota agotada del `/tmp` compartido. No se borraron recursos ajenos.

| Comando / comprobación | Resultado de esta revisión |
| --- | --- |
| `git ls-remote --symref https://github.com/eez-association/specs-draft.git HEAD refs/heads/main` | main/HEAD `2c6d346…`; snapshot local del mismo commit. |
| `cargo +1.95.0 test --locked --offline -p eez-primitives -p eez-evm -p eez-payload-codec --lib` | **36 pass, 1 fail** en eez-evm; salida 101. Los otros paquetes no llegaron a ejecutarse en esa invocación. R01 conservado. |
| `cargo +1.95.0 test --locked --offline -p eez-primitives -p eez-payload-codec -p eez-protocol --lib` | **237 pass**: 15 primitives, 16 payload, 206 protocol; 0 ignored/failed. |
| `forge test --root contracts` | **142 pass**, 16 suites, 0 failed/skipped; incluye proxy policy, manager/anchors, historia y FeeVault. |
| `cargo +1.95.0 test --locked --offline -p eez-driver --lib sequencer::tests` | **Error de compilación**, nueve E0433; ningún test ejecutado. R02. |
| `cargo +1.95.0 test --locked --offline -p eez-driver --features witness,derivation --lib sequencer::tests` | **7 pass**, 66 filtered, 0 ignored/failed. Otra configuración; no se oculta ni se arregla R02 con este resultado. |
| `python3 crates/eez-control-rpc/conformance/check_vectors.py` | **29 vectores** y schemas/fixtures consistentes. No ejecuta un motor Nethermind. |
| Probe Rust del vector DA remoto | Rechazo `Counts` de 31 bytes; equivalente local de 26 bytes decodificado. Assertions reales contra el codec. |
| SHA de fuente anterior | **305/305** entradas de la auditoría de simplificación aún coinciden; ninguna discrepancia. |
| Integridad de revisión / documentación | Comparación contra snapshot inicial sin cambios de implementación; `git diff --check` y links locales del nuevo informe comprobados. |

Son **422 casos que pasan y un caso que falla**, más una invocación que no
compila; no «423 tests correctos». Los checkers de vectores y el probe se informan
por separado. Los escenarios del reloj usan test doubles/sincronización controlada,
no una red desplegada. Los tests Forge son contratos locales, no broadcast.

La [auditoría inmediatamente anterior][PREVIOUS] retiene clippy/workspace, fmt,
320 casos del signer, 29 transcripts TCP/Stateless, oráculos de KZG/rolling/public
inputs y tres procesos source-based con signer sin red, aceptación real del manager
y replay del follower. Sus 305 hashes coinciden hoy: es evidencia previa pertinente,
**no comandos repetidos aquí ni prueba de toda la suite verde**, como muestran R01/R02.
Las capturas y hashes de ejecutables están en ese registro; sus logs grandes son locales.

No se repitieron workspace test/clippy, suite core completa, Anvil protocol matrix,
genesis regeneration/check, E2E/Kurtosis completo ni despliegues: el cambio actual
es un informe de revisión con comprobaciones dirigidas, no una implementación.
No se probó integración real Nethermind, bootstrap sin blobs antiguos, reorg de
profundidad arbitraria, disponibilidad futura de datos ni capacidad/performance
de cuatro días. No se suprimieron ni debilitaron tests para obtener verde.

## Acciones concretas para cerrar lo pendiente

| Acción | Motivo / condición de cierre |
| --- | --- |
| Corregir R01 y R02 en un cambio de implementación | Son fallos reales de la rama. Repetir los mismos comandos fallidos y los casos afectados, conservando estas primeras evidencias. |
| Publicar el perfil de red elegido, con esta matriz de diferencias | El nombre `0x45` o `V0` no basta para interoperar. Manifest debe fijar core, génesis/collector, native-v1, DA local, grid y reglas history/RANDAO. |
| Mantener una lista explícita de garantías aplazadas | Wrapper/guards, replay de triggers L1, multi-prover general, anuncios unsafe y bootstrap avanzado no se obtienen implícitamente del prover offline. No reabrir decisiones ya tomadas. |
| Comparar una implementación independiente antes de afirmar interoperabilidad | Nethermind debe pasar los fixtures y producir/rechazar exactamente los mismos bloques y public inputs, incluyendo fees, history repeat, fallos y ambas anclas. |
| Validación final del PR tras cualquier corrección | Las identidades/executables cambian al editar; ejecutar checks pertinentes y registrar nueva evidencia. Este informe no sustituye los gates CI ni una auditoría de seguridad. |

Ninguna de estas conclusiones autoriza despliegues, cambios del pin, upgrades,
migraciones o recuperación de datos. No hubo operaciones sobre redes/servicios.

[OLD]: ../2026-10-23-specs-divergences/README.md
[PREVIOUS]: ../2026-10-24-profile-simplification/README.md
[PROP]: ../../proposals/l1-origin-and-offline-proving.md
[NATIVE]: ../../proposals/native-system-transactions.md
[HISTORY-SPEC]: ../../../contracts/L1_HISTORY_SPEC.md
[RPC]: ../../../crates/eez-control-rpc/SPEC.md
[SIGNER]: ../../../crates/eez-proof-signer/SPEC.md
[IDENTITY]: ../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md
[CONFORMANCE]: ../../../crates/eez-control-rpc/conformance/README.md
[CORE]: ../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md
[ENTRIES]: ../../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md
[STATIC]: ../../../eez-core-protocol/docs/STATIC_ENTRY.md
[CAVEATS]: ../../../eez-core-protocol/docs/CAVEATS.md
[MULTI]: ../../../eez-core-protocol/docs/MULTI_PROVER_SPEC.md
[BLOB-SPEC]: ../../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md
[ABI]: ../../../crates/eez-protocol/src/abi.rs
[SOURCE]: ../../../crates/eez-protocol/src/native_source.rs
[RECON]: ../../../crates/eez-protocol/src/blob_semantics/reconstruction.rs
[SYSTEM-TX]: ../../../crates/eez-protocol/src/system_tx.rs
[PRIMITIVES]: ../../../crates/eez-primitives/src/lib.rs
[NATIVE-TESTS]: ../../../crates/eez-primitives/src/tests.rs
[EVM-TESTS]: ../../../crates/eez-evm/src/tests.rs
[BLOCK]: ../../../crates/eez-evm/src/block.rs
[EVM]: ../../../crates/eez-evm/src/evm.rs
[L2]: ../../../crates/eez-protocol/src/l2.rs
[PROFILE]: ../../../crates/eez-evm/src/profile.rs
[TIMING]: ../../../crates/eez-protocol/src/l2/derivation.rs
[CLOCK]: ../../../crates/eez-driver/src/sequencer.rs
[DRIVER-CARGO]: ../../../crates/eez-driver/Cargo.toml
[HISTORY]: ../../../crates/eez-protocol/src/l1_history.rs
[HISTORY-EVM]: ../../../crates/eez-evm/src/history.rs
[HISTORY-FETCH]: ../../../crates/eez-l1/src/derivation/history.rs
[HISTORY-CONTRACT]: ../../../contracts/src/L2/L1BlockhashContract.sol
[CONTEXT]: ../../../crates/eez-protocol/src/l1_history/context.rs
[COUNTS]: ../../../crates/eez-payload-codec/src/v0/counts.rs
[MANAGER]: ../../../contracts/src/OfflineRollup.sol
[VAULT]: ../../../contracts/src/L2/FeeVault.sol
[PROXY-TESTS]: ../../../contracts/test/CrossChainProxyPolicy.t.sol
[SCAN]: ../../../crates/eez-l1/src/scan.rs
[WATCHER]: ../../../crates/eez-l1/src/l1_watcher.rs
[UNSAFE]: ../../../crates/eez-follower/src/unsafe_head.rs
[POST-BATCH]: ../../../crates/eez-proof-signer/src/settlement/post_batch.rs
[POLICY]: ../../configurable-settlement-policy.md
[E01]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/eez-protocol-spec/01-architecture.md
[E02]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/eez-protocol-spec/02-evm-and-proxies.md
[E03]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/eez-protocol-spec/03-execution-model.md
[E04]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/eez-protocol-spec/04-proving-and-settlement.md
[E05]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/eez-protocol-spec/05-wire-formats.md
[R02]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/02-architecture.md
[R03]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/03-evm-proxy-systemtx.md
[R04]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/04-block-production.md
[R05]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/05-execution-model.md
[R06]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/06-composer.md
[R07]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/07-da-batches-bundles.md
[R08]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/08-proving-settlement.md
[R09]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/09-l1-to-l2.md
[R10]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/10-derivation-following.md
[R11]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/11-gas-economics.md
[R12]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/12-open-issues.md
[A]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/A-reference.md
[B]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/B-gas-cost-analysis.md
[C]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/C-open-questions.md
[D]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/D-wire-formats.md
[F]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/F-system-transaction-design.md
[G]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/G-blob-payload-design.md
[GN]: https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/gnosis-chain-spec/index.md
