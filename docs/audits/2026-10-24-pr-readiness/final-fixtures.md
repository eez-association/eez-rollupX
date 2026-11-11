# Recaptura final y conformidad offline

Las tres capturas de proceso y los **320 tests del signer** pasan con el runtime
final. Los **29 transcripts TCP/Stateless**, que esperan **24 pruebas**, conservan
sus controles y errores. No se cambió código, assertions ni expectativas manuales
para obtener estos resultados. La suite completa de procesos y la validación live
son trabajos separados; sus resultados pertenecen al informe principal.

## Identidad y procedimiento

Rama `codex/l1-history-offline-proving`, base
`3e23955dc477c9e675364776f118b1cecda75774` más el patch autorizado; core limpio
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. Los
[datos de evidencia](final-fixture-evidence.json) guardan capturas, hashes de
binarios, logs y corpus. El snapshot inicial de 514 archivos sigue conservado
localmente; su única desviación fue un comentario de `eez-control-rpc/Cargo.toml`
corregido para describir `prove.v2`, sin cambiar valores TOML.

Esta recaptura sigue la corrección del NatSpec del contrato de historia:
consume gas, pero no avanza el nonce del sistema. El responsable regeneró
runtime, génesis y fingerprint del manager. Se comprobó que los tres génesis base
y los tres génesis capturados contienen exactamente el runtime final de 1110 bytes,
SHA-256 `0e8a8f4671d83bd3416500fb606b73b1208083b9c1eece16085bf52eddde4bb1`.
El state root del génesis base E2E es
`0x76df720c92bb7bf7ff3a42031b3af51b9a2db7d91abf2da3a810ca00a89273bf`.

| Captura | Ejecución y resultado | Solicitud seleccionada |
| --- | --- | --- |
| `minimal_bidirectional_cross_chain_smoke` | Cargo fuente: 1 pass; firma aceptada y public input/estado comprobados | 1–200; anchor, outbound e inbound |
| `inbound_reverting_destination_preserves_caught_result_without_destination_writes` | Cargo fuente: 1 pass; revert atrapado y replay del follower | 1–210; anchor y observación fallida |
| `outbound_changing_static_reads_and_write_settle_and_replay` | Binarios fuente retenidos: 1 pass; escritura con lecturas intercaladas y follower | 1–30; anchor y grupo de cinco efectos |

Cada proceso seleccionó exactamente un test, con cero ignores. Todos usaron
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`; el launcher seccomp deniega conexiones
salientes del signer. Para las dos primeras capturas se limpiaron los tres
selectores `EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR` y
`EEZ_TEST_PROOF_SIGNER_BIN`, y se ejecutó:

```bash
cargo +1.95.0 test --locked --offline -p eez-node-e2e --test node_e2e -- \
  cross_chain::minimal_bidirectional_cross_chain_smoke --exact --test-threads=1 --nocapture
```

Static reutilizó `node_e2e-final`, `eez-proof-signer-final` y los dos wrappers
fuente ya compilados y hasheados. `NEXTEST_BIN_EXE_eez_composer` y
`NEXTEST_BIN_EXE_eez_follower` seleccionaron las copias nuevas bajo
`artifacts/pr-readiness/runtime-bin/final-source`; no se usaron los binarios
baseline ni una imagen shipping. La primera captura conserva hashes del test y
signer, pero sus wrappers no se hashearon antes del segundo enlace; esa identidad
por bytes no se afirma retrospectivamente. Lookup y static registran los cuatro.

## Inventario, generación y validación

La utilidad existente `inventory_native_proving_fixtures` se invocó expresamente
para cada directorio. Se seleccionó su única solicitud completa desde bloque 1,
comprobando número de entradas, dirección, éxito/revert y grupos de efectos.
Las capturas sustituyen a las anteriores; no añaden un formato legacy.

`regenerate_offline_vectors` construyó los 29 casos desde inputs y oráculos
existentes. `check_vectors.py --write-manifest` actualizó su procedencia;
`check_vectors.py` verificó las entradas y el esquema. También se verificaron
los **169 hashes fuente**. Los 29 nombres, las 24 pruebas y la secuencia de
controles se conservan; sólo cambia la longitud de los streams de bloques según
la captura. No se obtuvo ninguna expectativa de respuestas del servicio de replay.

```bash
cargo +1.95.0 test --locked --offline -p eez-proof-signer -- --test-threads=2
```

Resultado final: **320 passed, 0 failed, 2 ignored** en 12,33 s de ejecución.
Los dos ignores son las utilidades de inventario y generación ya invocadas
explícitamente. Los comandos Cargo usaron `CARGO_BUILD_JOBS=2`, LLVM 22 y
`TMPDIR` privado montado en `/tmp` con `bwrap`, por la cuota del `/tmp` compartido.

El primer intento de esta suite dio **317 pass y 3 fail**: la copia de fixtures
preservó timestamps anteriores a la compilación de inventario. Cargo conservó el
`include_str!` del génesis anterior para tres pruebas de derivación. Se demostró
que aquel ejecutable contenía el JSON anterior completo y no el nuevo; se conservó
su binario y log. Actualizar sólo el mtime del input forzó la recompilación.
El nuevo binario contiene el JSON nuevo y ya no el anterior; la misma suite pasa.
Es un fallo del procedimiento de recaptura, no un flake del código. Para repetir
la recaptura debe copiarse el contenido sin preservar timestamps antiguos.

El generador se repitió con el binario reconstruido y produjo exactamente los
mismos 29 archivos, verificados contra el manifest; no se ajustó ninguna expectativa.
Los logs, datadirs y ejecutables están bajo `artifacts/pr-readiness/final-fixtures`
y son **local-only**. Las capturas y el manifest sí forman parte del repositorio.
No se ejecutó Nethermind/C# ni se atribuyen aquí resultados de CI, de toda la suite
E2E o del enclave live.
