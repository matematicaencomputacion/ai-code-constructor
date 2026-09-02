# ADR-001: scheduler persistente por unidades para probes externos

- Estado: Accepted
- Fecha: 2026-09-01
- Decisores: usuario y líder técnico

## Contexto

La suite de compatibilidad ya limita llamadas y abre un circuito ante fallas externas, pero una nueva ejecución empieza desde cero. En endpoints rate-limited esto repite transporte y JSON, aumenta costo y puede agravar el `429`. El contrato aceptado exige pausar, persistir el último gate completado y reanudar desde el primero pendiente.

El endpoint NVIDIA es OpenAI-compatible para chat completions y tool calling, pero la disponibilidad del endpoint no demuestra compatibilidad completa del modelo con el Harness: [NVIDIA NIM chat completions](https://docs.nvidia.com/nim/large-language-models/latest/system-example.html).

## Fuerzas

- integridad causal de la evidencia;
- mínimo costo externo y cero loops ilimitados;
- tolerancia a cierre del proceso;
- checkpoint legible y auditable;
- comportamiento determinista en tests;
- no persistir secretos ni payloads del modelo.

## Opciones consideradas

### A. Dormir y reintentar dentro de `run_live`

Simple, pero pierde progreso al cerrar el proceso, bloquea durante esperas largas y vuelve difícil auditar presupuestos entre ejecuciones.

### B. Envolver cada llamada con `RetryingModelClient`

Reutiliza recovery existente, pero crea dos autoridades: el cliente duerme/reintenta sin persistir cada transición y el scheduler no puede garantizar el cursor ni el presupuesto durable.

### C. Scheduler persistente por unidades

Cada unidad produce resultados atómicos. El scheduler es la única autoridad de pacing, pausa y retry; persiste antes de terminar y vuelve a ejecutar solo la unidad pendiente.

## Decisión

Elegimos C.

- Se introduce un scheduler separado del probe y un executor de unidades.
- El checkpoint es JSON versionado y se escribe atómicamente.
- Los gates `autonomous_repair`, `multi_file` y `bounded_convergence` forman un `repair_bundle` atómico porque comparten una misma secuencia causal del AgentLoop.
- Las unidades de acciones son gates de gramática: validan la decisión JSON pero
  no ejecutan la herramienta representada.
- El `repair_bundle` live usa un Harness exclusivo e in-memory. Registra
  `Validate`, `RepairDiagnostic`, `ApplyCorrection`, `ApplyFileOperations` y un
  `ProbeCompileTool` sintético que no materializa archivos ni inicia procesos.
- `RunTests`, `RunClippy` y `CheckFormat` no están registrados dentro del bundle
  y son rechazados si el modelo los propone.
- Un resultado positivo del bundle no certifica ejecución real de código
  generado. Esa capacidad queda deshabilitada hasta incorporar un
  `SandboxRunner`.
- Un resultado externo transitorio no completa la unidad.
- Un resultado decisivo sí completa la unidad, aunque sea `fail`.
- El scheduler usa cliente crudo; no usa `RetryingModelClient`.
- `Retry-After` se interpreta según RFC 9110 y se somete a caps explícitos.
- Una pausa durable no consume el tiempo activo del probe.

## Consecuencias

Positivas:

- no se repiten gates confirmados;
- presupuesto y recovery son auditables entre procesos;
- las esperas largas no mantienen un proceso dormido;
- las pruebas pueden inyectar reloj, delay y cliente scriptado.

Negativas:

- aparece un esquema persistido que debe versionarse;
- el `repair_bundle` puede repetirse si el proceso cae antes de confirmarlo;
- esta primera versión asume un solo escritor por checkpoint.

## Riesgos y mitigaciones

- Checkpoint corrupto: error explícito; no iniciar llamadas.
- Identidad cambiada: rechazar mismatch; no mezclar evidencia.
- Path traversal por nombre de modelo: nombre derivado sanitizado y directorio controlado.
- Exposición del Bearer token: exigir HTTPS y rechazar HTTP antes de iniciar el probe live.
- Espera abusiva del proveedor: máximo individual y acumulado; exceso termina `blocked`.
- Doble retry: arquitectura prohíbe el wrapper de retry debajo del scheduler.
- Confusión entre compatibilidad y ejecución segura: reportar por separado los
  gates de gramática, el RepairBundle sintético y la ejecución real todavía no certificada.
- Dos procesos simultáneos: fuera de alcance inicial; documentar single-writer y añadir lock antes de paralelizar.

## Acciones de implementación

1. Extraer unidades ejecutables del probe monolítico sin cambiar semántica de gates.
2. Implementar checkpoint, validación, persistencia atómica y estado de pausa.
3. Cablear CLI con path, pacing y caps configurables.
4. Cerrar pruebas unitarias e integración determinista.
5. Ejecutar validación completa antes de una prueba viva acotada.
