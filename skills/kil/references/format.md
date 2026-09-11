# Format 2

Read the root and module JSON schemas for exact fields. Source documents contain `circuit`, `schematic`, and `pcb` sections. Do not edit generated KiCad files.

Use stable part IDs in connections and placements. Printed references are optional metadata. A part has a `reference_prefix`, `symbol`, and `footprint`. Terminal numbers and explicitly declared `terminals` aliases are valid addresses. `circuit.unconnected` declares intentionally unused terminals.

Schematic `symbols` have independent view IDs and specify `part`, `unit`, `at`, and optional `rotation`. Place every unit of a multi-unit symbol. A physical part still has one PCB footprint.

Modules declare public `ports`; each instance supplies `connections`. Parts and private nets are qualified by instance path. Instance schematic and PCB transforms independently include supplied views; absent transforms omit those views. Parameters use exact `${name}` substitution with declared types, without executable expressions.

PCB placement uses `mode: fixed|preferred`, with fixed as the default. Placement `at` and route `path` accept exact points, `{part, offset}`, `{pad, offset}`, or `{edge, fraction, offset}` anchors. Named distance constraints specify `from`, `to`, and `max`; `preferred: true` emits a warning instead of failing for excessive distance. Pad anchors must belong to the route's net. Use the anchored-divider example when constructing a relative layout.
