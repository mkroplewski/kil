---
name: kil
description: Create, edit, inspect, validate, build, or autoroute KiCad Intent Language projects stored as *.kil.json. Use for KIL source projects, not for directly editing native KiCad files.
---

# KIL projects

Use `kil` as the only compiler and validator for KiCad Intent Language projects. Edit KIL source with ordinary file patches. Do not introduce an MCP editing layer.

## Source of truth

- Edit root `*.kil.json` files and imported module files.
- Never edit generated `.kicad_pro`, `.kicad_sch`, or `.kicad_pcb` files. A later build replaces them.
- Never hand-edit `*.kil.routes.json`. Only `kil route` may publish a route cache.
- Preserve strict JSON. Comments, trailing commas, `NaN`, and JSON5 syntax are invalid.

If the request concerns an existing native KiCad project without KIL source, explain that KIL v2 has no importer or round-trip workflow. Do not convert the native files by improvising a parser.

## Before editing

Run `kil --version` to confirm the CLI is available. For routing benchmarks and substantial boards, use an installed release or `cargo build --release` and `target/release/kil`, not an unoptimized debug compiler. If it is missing, report that and offer the installation command from the project README. Do not execute a remote installer without the user's authorization.

Find the root KIL file and inspect only the context needed for the task. Do not rescan the whole project after each edit:

```console
kil inspect board.kil.json
kil inspect board.kil.json --component U1
kil inspect board.kil.json --net GND
kil inspect board.kil.json --block controller
kil inspect board.kil.json --region 10 10 40 35
```

Use `kil schema` or `kil schema --module` once when a field is uncertain. The installed schema is authoritative over examples in this skill.

Before wiring, identify unfamiliar symbols and inspect their resolved definitions up front instead of discovering pins one component at a time:

```console
kil library show MCU_Microchip_ATtiny:ATtiny1616-S
```

The result includes inherited properties plus every pin's number, name, electrical type, graphic style, position, and hidden state. Derived KiCad symbols are resolved automatically.

Read [references/format.md](references/format.md) when creating a project, changing connectivity or geometry, or working with modules. Read [references/routing.md](references/routing.md) only when the user asks for routing or autorouting.

## Editing workflow

Make one coherent source edit that satisfies the request. Keep part identities stable and use endpoint notation such as `U1.3`. Use named terminals such as `controller.VCC` only when the part explicitly declares that alias.

Before the first compiler run, verify the common structural requirements together:

- every physical part has a symbol and footprint; place each schematic unit and supply each PCB placement before building;
- every endpoint appears on at most one net, and intentional unused pins are listed under `circuit.unconnected`;
- the board outline, rules, zones, holes, and routing policy refer only to declared objects and nets.

Fix all independent diagnostics from a compiler run in one edit. Do not rerun `schema`, broad `inspect`, or unchanged library queries unless a diagnostic makes them relevant.

Before the first build, run `kil lock FILE` to record the resolved library contents. Rerun it only to explicitly accept a reviewed library change. After placement edits, run `kil preview FILE` first. It validates source geometry without requiring or changing a route cache. Then use a scoped `kil route FILE --net NAME` or `--block ID`; KIL adds nets invalidated by placement changes and retains unaffected copper. Other source changes conservatively invalidate all cached copper. Then run `kil check` and `kil build`. For other edits, run:

```console
kil check path/to/project.kil.json
```

For a new autorouted project, run `kil preview` before routing. Never route an unrelated net just to create a cache for placement inspection. Expected opens in a preview are reported separately; placement errors still fail. Read the routing reference before autorouting.

## Prototype milestones

Use these as planning guidance. The agent chooses the order and scope appropriate to the design; KIL does not enforce a milestone sequence.

1. Compare the handoff with the capabilities reported by `kil inspect FILE`. Identify unsupported requirements before promising a finished board. Differential impedance, pair coupling, length tuning, thermal performance and current capacity are not proved by connectivity or DRC.
2. Establish board dimensions, connector mating edges, antenna exclusion, heatsink and mounting envelopes. Review this floorplan before ordinary routing.
3. Run `kil preview` and resolve pad, courtyard, edge and keepout conflicts. Group independent placement fixes into one edit.
4. Build reusable modules with local placement, pad-anchored critical routes, constraints and ports. A logical netlist split alone does not preserve physical design quality. Keep switching loops, decoupling and pin escape geometry local to their owner module.
5. Establish critical local copper and power/ground distribution, then route ordinary signals against the same accepted board. Never overlay independently routed stages or relax rules to obtain a route.
6. Read the route report's accepted/rejected status, open counts and DRC delta. After two unchanged failures, change placement, corridors or constraints. `--retry` deliberately overrides the guard; it is not a default recovery action.
7. Run full `kil check` and `kil build`, then review native layers and fabrication outputs. Report connectivity, DRC and handoff requirement coverage separately. Do not describe a board with opens as DRC-clean.

Reuse the latest report under `build/routing/PROJECT/latest.json` when resuming. Read the named failed group and endpoint list instead of rescanning the whole project or rerunning all nets.

Treat the exit status as part of the result:

- `0` means success or warnings only.
- `1` means invalid source, unresolved libraries, failed generation, or unreadable KiCad output. Fix source errors before continuing.
- `2` means generated files are readable but ERC or DRC found design errors. Report remaining violations and fix those within the user's requested scope.

Use `kil build` when the user requests generated KiCad files or when finished artifacts are part of the task. A successful build publishes to `build/kicad/<project>/` by default. Do not treat publication as permission to edit those files.

## Boundaries

KIL v2 targets KiCad 10, multiple schematic sheets, reusable circuit modules, single- and multi-unit library symbols, library footprints, and 2 to 32 copper layers. It has no importer, round-trip editing, buses, embedded libraries, blind/buried vias, or native differential-pair and length-tuning constraints. Read the current schema for stackup and keepout options instead of assuming all copper layers or all keepout restrictions must be identical.

Do not hide these limits by emitting unsupported fields. If the requested design needs an unsupported feature, identify the exact boundary and stop before producing misleading output.
