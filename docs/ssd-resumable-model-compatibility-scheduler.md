# SSD: scheduler resumible del Model Compatibility Probe

Estado: aprobado para implementación incremental
Fecha: 2026-09-01

## Meta

Ejecutar la suite de compatibilidad contra endpoints externos como una secuencia durable de unidades. Si el proveedor responde con una falla transitoria, el proceso debe guardar el avance, quedar pausado y reanudar desde la primera unidad pendiente sin repetir gates ya completados, en especial transporte y JSON estricto.

## Goal -> Gap -> RecommendedAction -> Evidence

| Goal | Gap actual | RecommendedAction | Evidence de cierre |
|---|---|---|---|
| Reanudar sin trabajo duplicado | `run_live` ejecuta la suite monolíticamente | Separar la suite en unidades ordenadas y persistir el cursor | Test de dos ejecuciones: transporte y JSON se invocan una sola vez |
| Respetar `Retry-After` | El cliente expone la duración pero el probe solo abre el circuito | Convertir la señal en pausa durable, con máximo por espera y acumulado | Tests de delta-seconds, espera acotada y reanudación temprana sin llamadas |
| Evitar loops costosos | Los presupuestos se reinician con cada proceso | Persistir llamadas, intentos de recovery y espera acumulada | Tests de agotamiento a través de múltiples reanudaciones |
| Evidencia auditable | El reporte final no explica el estado entre procesos | Checkpoint versionado, atómico y libre de secretos | Round-trip, rechazo de identidad incompatible e inspección de JSON |

## Alcance

Incluye:

- checkpoint por proveedor, modelo, URL base, perfil y versión de suite;
- estados `ready`, `waiting`, `completed` y `blocked`;
- cursor a la primera unidad pendiente;
- pausa durable por `429`, `5xx`, timeout o transporte transitorio;
- `Retry-After` acotado por espera, acumulado y cantidad de recoveries;
- pacing secuencial configurable entre unidades externas;
- persistencia atómica mediante archivo temporal y rename;
- salida segura: sin API key, prompts, respuestas ni cuerpos HTTP.

No incluye:

- paralelismo entre modelos;
- daemon o servicio de scheduling;
- coordinación distribuida o lock multi-host;
- certificación automática de tool calling nativo;
- sandbox o certificación de ejecución real de código generado; esa capacidad
  permanece deshabilitada hasta disponer de un `SandboxRunner`.

## Unidades y orden

1. `retry_after_synthetic` (local).
2. `transport_prompt_only`.
3. `json_object_strict`.
4. Una unidad por cada acción esperada, en el orden canónico de la suite.
5. `repair_bundle`, unidad causal que produce `autonomous_repair`, `multi_file` y `bounded_convergence`.
6. `native_tool_calling` (por ahora `not_tested`, local).

Una unidad se marca completada solo después de persistir todos sus resultados. Una interrupción dentro de `repair_bundle` puede repetir ese bundle completo; no puede repetir transporte, JSON ni acciones anteriores.

Las unidades individuales de acciones validan únicamente que el modelo pueda
producir el JSON esperado. Incluso los gates de `RunTests`, `RunClippy` y
`CheckFormat` no ejecutan `cargo`.

El `repair_bundle` live usa un Harness dedicado e in-memory. Registra
`Validate`, `RepairDiagnostic`, `ApplyCorrection`, `ApplyFileOperations` y un
`ProbeCompileTool` sintético; no materializa el artefacto ni inicia procesos.
`RunTests`, `RunClippy` y `CheckFormat` no están registrados y son rechazados si
el modelo los propone durante el bundle. Su veredicto certifica causalidad de
reparación sobre la fixture, no ejecución real de código generado.

## Máquina de estados

```text
ready --execute next unit--> ready
ready --transient within budget--> waiting(not_before)
ready --transient budget exhausted-> blocked
waiting --too early--------> waiting (0 external calls)
waiting --deadline reached-> ready --retry same unit-->
ready --all units done-----> completed
ready --budget/cap invalid-> blocked
```

Un fallo decisivo de modelo o adaptador completa la unidad con `fail`; no se transforma en retry infinito. Una falla externa transitoria no avanza el cursor.

## Contrato del checkpoint

El documento persistido contiene:

- `schema_version` y `suite_version`;
- identidad segura del target y perfil;
- estado y siguiente unidad;
- resultados completos de unidades confirmadas;
- llamadas externas acumuladas y tiempo activo acumulado;
- recoveries usados y espera acumulada;
- `not_before_unix_ms` y última causa de pausa, sin payload remoto;
- timestamp de actualización.

Al cargar, proveedor, modelo, URL, perfil y suite deben coincidir exactamente. Una incompatibilidad es error explícito; nunca se descarta silenciosamente el avance.

## Guardrails de recovery

- Una sola autoridad: el scheduler llama al cliente sin envolverlo en `RetryingModelClient`.
- Todo target live requiere HTTPS; una base URL HTTP se rechaza antes de usar el Bearer token.
- Concurrencia por target: 1.
- `Retry-After`: aceptar delta-seconds y HTTP-date conforme a [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html), convertirlo a una duración y acotarlo.
- Si falta una duración en una falla transitoria, usar un fallback pequeño y configurable.
- Si la espera solicitada excede el máximo individual, o supera el máximo acumulado, bloquear con evidencia en vez de dormir o truncar silenciosamente.
- Las pausas largas se persisten y el proceso termina; solo el pacing corto se duerme dentro de una ejecución.
- Una reanudación anterior a `not_before` no consume llamada, intento ni presupuesto activo.
- NVIDIA documenta `429` como rate limiting y recomienda reducir concurrencia; la ejecución secuencial y el pacing siguen ese guardrail: [NVIDIA RAG troubleshooting](https://docs.nvidia.com/rag/2.5.0/troubleshooting.html).

## Presupuestos iniciales

Los valores son configuración, no constantes implícitas:

- máximo de llamadas: el menor entre permiso vivo y configuración del probe;
- recoveries transitorios: 3;
- espera individual: 120 segundos;
- espera acumulada: 300 segundos;
- fallback transitorio: 5 segundos;
- pacing entre unidades externas: 2 segundos;
- tiempo activo: el límite actual del perfil; la espera persistida no lo consume.

## Estrategia de pruebas

### Unitarias

- round-trip del checkpoint y ausencia de campos sensibles;
- rechazo de target, perfil o suite incompatibles;
- sanitización del nombre de archivo;
- escritura atómica y carga de checkpoint existente;
- `Retry-After` en delta-seconds y HTTP-date;
- espera mayor al cap y presupuesto acumulado agotado;
- reanudación temprana con cero llamadas;
- llamadas e intentos persistentes entre procesos.

### Integración determinista

Un cliente scriptado ejecuta: transporte pass, JSON pass y `429` en la primera acción. La segunda ejecución, con reloj adelantado, reintenta esa acción y termina. Los contadores prueban que transporte y JSON permanecen en una invocación.

### Regresión

`cargo fmt --check`, `cargo check --all-targets --all-features`, suite completa y Clippy estricto. La prueba viva es posterior y acotada; una falla del proveedor se informa como inconclusa, no como incompatibilidad.

## Contrato de cierre

La unidad queda cerrada cuando:

1. SSD y ADR están versionados en `docs/`.
2. El checkpoint nunca contiene secretos ni contenido remoto.
3. Un `429` pausa en la misma unidad y persiste `not_before`.
4. Reanudar antes del plazo hace cero llamadas.
5. Reanudar después del plazo continúa desde la primera unidad pendiente.
6. Transporte y JSON aprobados no se repiten.
7. Los presupuestos sobreviven al reinicio del proceso.
8. Todas las validaciones deterministas están verdes y la evidencia exacta queda documentada.
9. `repair_bundle` no puede materializar archivos ni iniciar `cargo`, `rustc` u
   otro proceso.
10. Toda evidencia live anterior al hardening se identifica como histórica y
    no se usa para certificar el límite de ejecución actual.
