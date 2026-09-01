# KIL format decisions

Read this reference when creating or structurally editing KIL source. Query `kil schema` for the complete field-level contract.

## Project model

A root file contains:

- `format_version`, `project`, and `units`;
- a `components` map keyed by stable reference designators;
- a `nets` map whose values are component endpoints;
- schematic placement, wires, labels, and no-connect markers;
- PCB outline, placement, routes, vias, zones, holes, silk, and optional routing policy;
- project-wide default rules.

Declare each component and net once. Schematic and PCB sections should carry geometry rather than duplicate electrical objects.

Library identifiers use `Library:Entry`, for example `Device:R` and `Resistor_SMD:R_0603_1608Metric`. The environment's KiCad library tables determine whether an entry exists. Let `kil check` resolve symbols, pins, footprints, and pads.

Use `kil library show Library:Entry` before connecting an unfamiliar symbol. It resolves KiCad `extends` chains and reports the effective properties and pins seen by KIL. Do not infer a pin number from a related package or component variant.

## Coordinates

PCB coordinates are local millimetres relative to the lower-left corner of the board. `+X` points right and `+Y` points up. Positive angles turn counter-clockwise.

Routes are explicit polylines grouped under their net. Vias, zones, holes, and silk are separate objects. V1 has no placement or constraint solver, so write the resulting geometry rather than prose constraints.

Before adding PCB geometry, inspect nearby objects with `kil inspect --region`. Keep placements, route points, zones, and holes within the board outline unless the object intentionally crosses an edge and KiCad accepts it.

## Connectivity

An endpoint has the form `REF.PIN`, such as `U3.14`. A named pin is allowed only when it selects one symbol pin. Every referenced component and net must exist. A physical pad must match the resolved symbol pin where the backend requires a pin-to-pad connection.

Use `schematic.no_connect` for intentionally unused pins. Do not omit a known connection merely to silence ERC.

## Modules

Use modules to keep large projects readable and to isolate local geometry. A module may contain components, local nets, schematic data, PCB data, and nested imports. The root import supplies a stable block ID, a relative path, optional net mapping, and separate schematic and PCB transforms.

```json
{
  "id": "controller",
  "path": "blocks/controller.kil.json",
  "net_map": {
    "VCC": "+3V3",
    "GND": "GND"
  },
  "schematic": { "at": [100, 70] },
  "pcb": { "at": [25, 18], "rotation": 90 }
}
```

Mapped nets join the named parent nets. Unmapped local nets receive names such as `controller/SPI_CLK`. Component references remain global, so two imported modules cannot both define `U1`.

Module paths must remain inside the project directory. Absolute paths, `..` escapes, cycles, duplicate block IDs, duplicate references, and conflicting placements are errors.

Set `locked: true` on completed placements, routes, or vias when later routing should preserve them.
