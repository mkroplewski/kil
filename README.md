# kil

`kil` is a small, deterministic compiler from an agent-editable KiCad IL (`*.kil.json`) to KiCad 10 projects. The IL is the source of truth; generated `.kicad_pro`, `.kicad_sch`, and `.kicad_pcb` files are replaceable artifacts and must not be edited by agents.

## Quick start

```powershell
cargo run -p kil -- schema
cargo run -p kil -- schema --module
cargo run -p kil -- inspect examples/modular-resistors/project.kil.json --block divider
cargo run -p kil -- check examples/rc-led.kil.json
cargo run -p kil -- build examples/two-resistors.kil.json
```

The default output is `build/kicad/<project>/` next to the input IL file. KiCad library identifiers are resolved from project/global library tables and the standard KiCad 10 installation. Override discovery with `--kicad-cli PATH`.

## CLI contract

- `kil check FILE` resolves libraries, generates into a temporary directory and runs KiCad netlist/ERC/DRC validation without publishing files.
- `kil build FILE [--out DIR]` performs the same checks and publishes a structurally valid project with rollback protection.
- `kil route FILE --krt PATH` routes a temporary board with KiCadRoutingTools, validates the result and atomically publishes only a normalized `*.kil.routes.json` cache.
- `kil inspect FILE [--component REF | --net NET | --block ID | --region X1 Y1 X2 Y2]` prints a compact read-only JSON slice, so an agent need not load the whole project.
- `kil schema` prints the root-project JSON Schema; `kil schema --module` prints the imported-module schema.
- `--diagnostics text|json` controls diagnostic output.
- Exit `0` means success or warnings only, `1` means invalid IL/generation, and `2` means valid generated files with ERC/DRC errors.

## v1 format

The schema is intentionally normalized: components and nets are declared once, while schematic and PCB sections carry only view/layout geometry. PCB coordinates are local millimetres with the origin at the lower-left, +X right and +Y up. Component references and `REF.PIN` endpoints replace native UUIDs; the compiler derives stable UUID v5 values.

V1 supports one schematic sheet, library symbols/footprints, two copper layers, polygon outlines, placements, traces, vias, zones, holes, silkscreen text and recursively imported layout modules. Round-trip editing, electrical sheet hierarchy, buses, derived/multi-unit symbols and inner layers remain outside v1.

The `examples/` directory includes a compact RC/LED circuit, a small ATtiny board and `modular-resistors`, which demonstrates a reusable local-coordinate block.

## Modules and large projects

A root `*.kil.json` may import strict-JSON module files. Each import has a stable instance `id`, a project-relative `path`, explicit schematic/PCB transforms and an optional `net_map`. Components keep globally meaningful references such as `U1`; duplicate references are rejected. A mapped local net joins its parent net. An unmapped local net is namespaced as `block-id/local-net`, which prevents accidental short circuits between blocks.

Module paths cannot be absolute, escape the project directory or form an import cycle. The compiler expands the graph before ordinary semantic validation and backend generation, so modules add no special state to generated KiCad files. `locked: true` on placements, routes and vias preserves completed module geometry when a later routing pass handles the remaining board.

This is deliberately file-first rather than an MCP editing protocol: agents can use ordinary patches, schema validation, version control and any text tooling. `kil inspect` only reduces read context; it does not become a mutation API.

## Automatic routing

Automatic routing uses [KiCadRoutingTools](https://github.com/drandyhaas/KiCadRoutingTools). Clone it separately and install its Python dependencies and Rust router, then run:

```powershell
python C:/tools/KiCadRoutingTools/build_router.py
cargo run -p kil -- route examples/tiny-controller.kil.json --krt C:/tools/KiCadRoutingTools
cargo run -p kil -- build examples/tiny-controller.kil.json
```

The main IL contains routing intent and any manually authored seed routes. The generated `tiny-controller.kil.routes.json` contains normalized segments and vias plus a SHA-256 fingerprint of every routing input. `check` and `build` reject a missing or stale cache, so changing placement, nets, rules or manual routes cannot silently reuse obsolete copper. Router output exists as a native KiCad file only inside staging; it is never accepted as source truth.

`pcb.routing.nets` accepts the same wildcard patterns as KiCadRoutingTools. `extra_args` passes additional router options without shell expansion. CLI `--net` values override the configured net patterns for one run. `--block ID` selects all nets owned by an imported block; explicit `--net` and `--block` are mutually exclusive.

The adapter reads KiCadRoutingTools' `JSON_SUMMARY_MIN` record and treats failed or open pad pairs as design violations. A successful process that emits no copper is rejected and cannot replace the previous cache. Before routing, KiCad refills zones in staging; after routing, KiCad independently checks and saves the board before copper is converted back to IL coordinates.

KiCadRoutingTools is intentionally an external dependency rather than a linked Rust crate: its public workflow is currently a Python CLI backed by a Rust extension. It is MIT licensed and supports incremental obstacle caches, scoped routing, rip-up/reroute, differential pairs and length matching. Its documented limitations include no push-and-shove, no blind/buried vias, no coarse global-routing pass and no region-specific design rules. For large boards, prefer routing by placement block or net group instead of one global `--nets "*"` invocation.
