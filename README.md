# KIL

KIL compiles declarative circuit and layout intent into a KiCad 10 project. Edit `*.kil.json`; generated KiCad files are disposable output. The format is experimental and currently supports one schematic sheet and two copper layers.

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
