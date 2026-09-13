# Format 2

Read the root and module JSON schemas for exact fields. Source documents contain `circuit`, `schematic`, and `pcb` sections. Do not edit generated KiCad files.

Use stable part IDs in connections and placements. Printed references are optional metadata. A part has a `reference_prefix`, `symbol`, and `footprint`. Terminal numbers and explicitly declared `terminals` aliases are valid addresses. `circuit.unconnected` declares intentionally unused terminals.

Schematic `symbols` have independent view IDs and specify `part`, `unit`, `at`, and optional `rotation`. Place every unit of a multi-unit symbol. A physical part still has one PCB footprint.

Modules declare public `ports`; each instance supplies `connections`. Parts and private nets are qualified by instance path. Instance schematic and PCB transforms independently include supplied views; absent transforms omit those views. Parameters use exact `${name}` substitution with declared types, without executable expressions.

PCB placement uses `mode: fixed|preferred`, with fixed as the default. Placement `at` and route `path` accept exact points, `{part, offset}`, `{pad, offset}`, or `{edge, fraction, offset}` anchors. Named distance constraints specify `from`, `to`, and `max`; `preferred: true` emits a warning instead of failing for excessive distance. Pad anchors must belong to the route's net. Use the anchored-divider example when constructing a relative layout.

Keepouts independently exclude `tracks`, `vias`, `pads`, `copper_pours`, and `footprints`. Each flag defaults to `true`; set a flag to `false` to allow that object type. For an antenna copper exclusion that permits the module body, set `footprints: false`. Omit `layers` to cover every copper layer. At least one restriction must remain enabled.

For mixed copper weights, set `pcb.stackup.copper_thicknesses` in millimetres in layer order, from `F.Cu` through the inner layers to `B.Cu`. Supply one positive thickness per copper layer, for example `[0.07, 0.035, 0.035, 0.07]` on a four-layer board. Omit the array to use the uniform `copper_thickness` setting. Explicit dielectric thicknesses plus the effective copper thicknesses must sum to the board thickness.
