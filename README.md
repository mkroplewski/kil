# KIL

KIL compiles declarative circuit and layout intent into a KiCad 10 project. Edit `*.kil.json`; generated KiCad files are disposable output. The format is experimental and currently supports multiple schematic sheets and 2–32 copper layers.

## Format 2

The authoring document and resolved compiler model are separate types. The compiler expands module instances without modifying the source document, assigns printed references, resolves physical terminals and library assets, and generates KiCad objects using stable source identities.

- `circuit.parts` maps stable local IDs to physical parts. Each part supplies a symbol, footprint, reference prefix, optional printed reference, value, fields, and explicit terminal aliases.
- `circuit.nets` defines electrical connectivity using `part.terminal` endpoints. Terminal numbers are canonical; `terminals` supplies explicit aliases. Library display names are not implicit addresses.
- `circuit.unconnected` declares terminals intentionally left unconnected.
- `circuit.instances` instantiates modules with public port connections, typed parameter values, and optional schematic and PCB transforms.
- `schematic.symbols` maps view IDs to a physical `part`, symbol `unit`, position, and rotation. Several units can represent one physical part.
- `pcb` specifies board geometry and layout in millimetres, with +X right, +Y up and counter-clockwise positive rotations. Schematic coordinates follow KiCad's drawing frame.

Component identities such as `left/input_resistor` remain independent of printed references such as `R7`. Explicit references must be unique; otherwise the compiler assigns references in sorted identity order. Reannotation does not change the object's generated UUID. Renaming an identity creates a new identity.

See [two-resistors](examples/two-resistors.kil.json), [module definition](examples/modular-resistors/blocks/divider.kil.json), [module instance](examples/modular-resistors/project.kil.json), and [multi-unit op-amp](examples/dual-opamp.kil.json).

## Modules

A module declares `ports`, mapping public port names to its local nets. Every public port must be connected by an instance. Internal nets and component IDs are private to the instance; the same module can be instantiated repeatedly. Module paths must resolve inside the project directory and import cycles are errors.

Instance `schematic` and `pcb` transforms independently opt into the module's supplied views. Omit either to supply that view from the parent, using qualified part IDs. Electrical hierarchy does not require KiCad sheet hierarchy.

Parameters have a declared `type` of `string`, `number`, or `boolean` and an optional default. An exact string `${name}` substitutes the parameter value before typed parsing. There is no expression evaluation. Unknown parameters, missing values, and wrong types are errors.

## Commands

Requires KiCad 10 and its symbol and footprint libraries. Building from source requires Rust 1.90 or newer.

```sh
cargo install --path crates/kil-cli
kil inspect examples/two-resistors.kil.json
kil lock examples/two-resistors.kil.json
kil check examples/two-resistors.kil.json
kil build examples/two-resistors.kil.json --out build/divider
kil library show Amplifier_Operational:LM358
kil schema
kil schema --module
```

Use `--kicad-cli PATH` when KiCad is not on PATH. `--diagnostics json` emits structured diagnostics. `inspect` accepts `--component ID`, `--net NAME`, `--block ID`, or `--region X1 Y1 X2 Y2`.

Builds stage output, validate it with KiCad, then publish it with rollback on ordinary publication failures. Exit 0 means success, 1 means invalid input or failed compilation, and 2 means readable output with ERC/DRC design errors. Review generated boards before fabrication.

The root and module editor schemas are [kil-v2](schemas/kil-v2.schema.json) and [kil-module-v2](schemas/kil-module-v2.schema.json). Format 1 is not supported.

## Installation

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/mkroplewski/kil/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/mkroplewski/kil/main/install.ps1 | iex
```

Release installers bundle KiCadRoutingTools and create a Python environment. KiCad remains a system dependency. See [THIRD_PARTY.md](THIRD_PARTY.md).

Install the agent skill with `npx skills add mkroplewski/kil --skill kil -g -y`.

## Development

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Generation tests use small checked-in library fixtures and run without KiCad. Real KiCad checks require a local installation. CI also verifies that committed schemas match the Rust source models.

Licensed under MIT.

## Layout intent

Placement `at` and route `path` entries accept exact `[x, y]` points or anchors:

```json
{"pad": "controller.VCC", "offset": [0, 0]}
{"part": "controller", "offset": [3, 0]}
{"edge": 0, "fraction": 0.5, "offset": [0, 2]}
```

Pad offsets use the placed part's local board frame; component offsets rotate with that component. Edge indices address the root board outline and offsets use root board axes. Ambiguous repeated pad numbers require explicit coordinates. Relative placement dependencies must be acyclic. Module transforms preserve local relationships, and pad anchors resolve against actual footprint contents.

Placement `mode` defaults to `fixed`; `preferred` supplies a position without locking it. The compiler resolves these positions deterministically, without an automatic placement optimizer. Named `pcb.constraints` specify `from`, `to`, and positive `max` distance in millimetres. A failed requirement blocks generation; `preferred: true` makes a distance violation a warning. Routes retain explicit `locked` ownership.

See [anchored-divider](examples/anchored-divider.kil.json). Moving R1 moves R2 and both route endpoints. Replacing the footprint resolves those endpoints again instead of retaining copied pad coordinates.

## Build profiles, locks, and routing

Tool selection belongs to `build.routing`, separate from PCB geometry and electrical rules:

```json
{"build": {"routing": {"engine": "kicad-routing-tools", "nets": ["*"]}}}
```

Run `kil lock FILE` to accept the current resolved symbol and footprint contents. Commit the adjacent `*.kil.lock.json`. Checks, builds, and routing require a matching lock; library changes require review and another explicit `lock` command. No source document is rewritten by locking.

`kil route FILE` writes a derived `*.kil.routes.json` cache. `--net PATTERN` and `--block ID` select a subset. A targeted pass seeds from the valid cache and preserves unselected copper and its lock flags. Shared nets still span the full board. Stale caches require a full reroute. The router cannot change footprint placement or locked copper. KIL regenerates and checks the board from normalized cached copper before publishing that cache.

Routing fingerprints include resolved library contents, nets, PCB geometry, rules, and routing policy. Schematic placement, values, and printed reference changes do not invalidate copper.

`rules.minimum_track_width` is a requirement. `rules.preferred_track_width` is the routing default. Named `rules.net_classes` declare explicit net membership, clearance, minimum and preferred widths, and allowed copper layers. Class requirements must meet board-wide minimums; multiple class assignments are errors. The compiler checks geometry and emits native KiCad net classes and a `.kicad_dru` file with custom width/layer rules. That fourth file is part of the published project. See KiCad's [custom rule documentation](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html#custom-design-rules).

Connectivity comes only from `circuit.nets` and `circuit.unconnected`. KIL compares the exported schematic netlist with the resolved circuit. Unexpected connections, including shorts introduced by drawn wires, block publication. Missing or malformed ERC/DRC reports also block publication.

## Explicit integration tests

The ordinary suite uses checked-in library fixtures. Additional tests require KiCad 10 and KiCadRoutingTools. They appear as ignored in ordinary test output, and the dedicated Ubuntu CI job installs the tools and runs them on every pull request and push to `main`. CI uses the KiCad 10 release PPA and the router tag in `KRT_VERSION`. To run them locally:

```sh
KIL_KICAD_CLI=/path/to/kicad-cli KIL_KRT=/path/to/KiCadRoutingTools \
  cargo test -p kil-core --test kicad_integration -- --ignored
```

They verify repeated modules, symbol rotations, back-side pad anchors, an unintended schematic short, sequential routing of two net groups, preservation of copper and lock flags, cache-backed publication, and stale-cache rejection after moving a part. The router test uses a temporary copy of the example.

### Multilayer boards

Omit `pcb.stackup` for the usual two-layer, 1.6 mm board. A four-layer board only needs:

```json
"stackup": { "layers": 4 }
```

Layers are named `F.Cu`, `In1.Cu`, `In2.Cu`, and `B.Cu`. Routes, planes, and net-class layer restrictions use those same names. Vias remain through vias and cross every copper layer. Omitting `allowed_layers` from a net class allows all board layers; an explicit list also restricts which layers a through via may cross.

Default copper is 0.035 mm thick, with the remaining thickness distributed evenly as FR4. These are generation defaults, not a manufacturer-approved impedance stackup. For a fabrication specification, set `thickness`, `copper_thickness`, and `dielectrics` in `stackup`. Each dielectric specifies its `thickness`, optionally `material` and `epsilon_r`, in order between adjacent copper layers. Thicknesses must add up. Only the root project defines the board stackup.

Planes support optional `priority`, `solid`, `thermal_gap`, and `thermal_width`. The defaults retain thermal pad connections. See `examples/four-layer-divider.kil.json` for a checked inner-layer route and ground plane. Blind/buried vias and microvias are not supported.

### Schematic sheets

Keep using `schematic.symbols` for a single sheet. To organize a larger drawing, put the same `symbols`, `wires`, and `labels` inside named `schematic.sheets`:

```json
"schematic": {
  "sheets": {
    "power": { "symbols": { "regulator": { "part": "regulator", "unit": 1, "at": [50.8, 50.8] } } },
    "io": { "symbols": { "connector": { "part": "connector", "unit": 1, "at": [50.8, 50.8] } } }
  }
}
```

Each page has independent coordinates. Existing top-level drawing fields stay on the root sheet. Sheet names do not change part identities or circuit connectivity: the compiler connects sheets from `circuit.nets`, without another set of user-maintained electrical ports. Module ports still define circuit interfaces. A module can supply named sheets; repeated instances receive distinct page identities. A parent can instead omit the module's schematic transform and place its parts on its own pages.

Generated child files live in `<project>.sheets/`, owned by the compiler and replaced with the other generated artifacts on every build. Sheet filenames are stable generated identifiers, not source paths. See `examples/multi-sheet-divider.kil.json`.
