# kil

`kil`, the KiCad Intent Language, compiles a compact JSON description of a circuit into a KiCad 10 project.

The JSON file is the source of truth. Agents and humans edit `*.kil.json`; `kil` regenerates `.kicad_pro`, `.kicad_sch`, and `.kicad_pcb` files. This avoids asking a language model to patch large KiCad S-expressions without breaking references, UUIDs, or file structure.

> [!WARNING]
> `kil` is experimental. It currently targets new, single-sheet, two-layer designs. Do not use it as the only copy of a production design until the format and compiler have seen more real-world testing.

## Why another circuit format?

Native KiCad files are good application files, but poor editing targets for an agent. They contain repeated geometry, generated identifiers, library data, and ordering details that consume context without describing design intent.

`kil` keeps the editable representation small:

- components and nets are declared once;
- connections use endpoints such as `U1.3` or `U1.VCC`;
- PCB geometry uses local millimetres and explicit polylines;
- reusable blocks live in separate JSON files;
- UUID v5 values and KiCad boilerplate are generated deterministically;
- invalid input produces structured diagnostics before generated files replace a working build.

This is a file format and CLI, not an editing server. Any editor, script, agent, or version-control workflow can modify the JSON.

## Example

```json
{
  "format_version": 1,
  "project": { "name": "divider", "kicad": 10 },
  "units": "mm",
  "components": {
    "R1": {
      "symbol": "Device:R",
      "value": "10k",
      "footprint": "Resistor_SMD:R_0603_1608Metric"
    },
    "R2": {
      "symbol": "Device:R",
      "value": "20k",
      "footprint": "Resistor_SMD:R_0603_1608Metric"
    }
  },
  "nets": {
    "SIGNAL": ["R1.1", "R2.1"],
    "GND": ["R1.2", "R2.2"]
  },
  "schematic": {
    "placement": {
      "R1": { "at": [50.8, 50.8] },
      "R2": { "at": [63.5, 50.8] }
    }
  },
  "pcb": {
    "outline": [[0, 0], [12, 0], [12, 14], [0, 14]],
    "placement": {
      "R1": { "at": [5, 5] },
      "R2": { "at": [5, 9] }
    },
    "routes": {
      "SIGNAL": [
        { "layer": "F.Cu", "path": [[4.175, 5], [4.175, 9]] }
      ]
    }
  }
}
```

PCB coordinates start at the lower-left corner of the board. `+X` points right, `+Y` points up, distances use millimetres, and positive rotations are counter-clockwise.

See [examples/two-resistors.kil.json](examples/two-resistors.kil.json) for a complete small board and [examples/tiny-controller.kil.json](examples/tiny-controller.kil.json) for a larger example with routing policy, a copper zone, vias, holes, and silkscreen.

## Requirements

- KiCad 10 with `kicad-cli`
- the KiCad symbol and footprint libraries used by the input file
- Python 3.9 or newer when using the bundled autorouter

The current implementation is developed and tested on Windows. The Rust code is intended to be portable, but other operating systems are not yet covered by CI.

## Install

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/mkroplewski/kil/main/install.ps1 | iex
```

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/mkroplewski/kil/main/install.sh | sh
```

The installer downloads the correct release for the current platform, verifies its SHA-256 checksum, and installs `kil` for the current user. It also installs KiCadRoutingTools 0.21.4 and its Python packages in a private environment. Open a new terminal after installation.

Prebuilt releases cover Windows x86-64, Linux x86-64, macOS Intel, and macOS Apple Silicon. KiCad itself remains a system dependency.

### Install from source

Building from source requires Rust 1.90 or newer. Clone the repository, then run:

```console
cargo install --path crates/kil-cli
```

You can also run commands without installing the binary:

```console
cargo run -p kil -- --help
```

## Quick start

Validate an input without publishing generated files:

```console
kil check examples/rc-led.kil.json
```

Build a KiCad project:

```console
kil build examples/two-resistors.kil.json
```

By default, the output goes to `build/kicad/<project>/` beside the input file. Use `--out DIR` to choose another directory. If `kicad-cli` is not on `PATH`, pass it explicitly:

```console
kil --kicad-cli "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe" check board.kil.json
```

Generated KiCad files are disposable build artifacts. Manual changes to them disappear on the next build.

## Commands

| Command | What it does |
| --- | --- |
| `kil check FILE` | Parses and validates the IL, resolves KiCad libraries, generates into a temporary directory, then runs KiCad netlist, ERC, and DRC checks. |
| `kil build FILE [--out DIR]` | Runs the same checks and atomically publishes a structurally valid KiCad project. |
| `kil inspect FILE` | Prints a compact JSON summary without resolving libraries or starting KiCad. |
| `kil inspect FILE --component U1` | Prints one component, its placements, and connected nets. |
| `kil inspect FILE --net GND` | Prints endpoints and geometry for one net. |
| `kil inspect FILE --block power` | Prints one imported block. |
| `kil inspect FILE --region X1 Y1 X2 Y2` | Prints PCB objects with a recorded point inside a rectangular area. |
| `kil schema` | Prints the root project JSON Schema. |
| `kil schema --module` | Prints the module JSON Schema. |
| `kil route FILE --krt PATH` | Runs KiCadRoutingTools on a staged board and writes normalized copper back to a route cache. |

All commands accept `--diagnostics text|json`. JSON diagnostics have stable fields for `severity`, `code`, `message`, `file`, `span`, `path`, `related`, and `help`.

Exit codes are part of the CLI contract:

| Code | Meaning |
| --- | --- |
| `0` | Success, possibly with warnings. |
| `1` | Invalid IL, missing library, generation failure, or unreadable KiCad artifact. |
| `2` | The project was generated and published, but ERC or DRC found design errors. |

## Safe builds

`kil build` never writes directly into the published project while compiling. It performs these steps:

1. Parse strict JSON and collect independent semantic errors.
2. Resolve imported modules and KiCad library entries.
3. Generate all files in a sibling staging directory.
4. Ask `kicad-cli` to read the schematic and PCB, export a netlist, run ERC, refill zones, and run DRC.
5. Publish the artifact set by rename, with rollback if publication fails.

Structural errors stop publication. ERC and DRC findings do not hide readable output, so `build` publishes the project and exits with code `2`.

The same input and library contents produce byte-identical files. Semantic paths in the IL determine generated UUID v5 values.

## Modules

Large projects can split into recursively imported files. A module owns components, local nets, schematic placement, and PCB geometry. The root file places it with independent schematic and PCB transforms:

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

`net_map` connects selected module nets to parent nets. Unmapped nets receive names such as `controller/SPI_CLK`, which prevents unrelated blocks from joining by accident. Component references remain global and collisions are errors.

Import paths must stay inside the project directory. The loader rejects absolute paths, `..` escapes, cycles, duplicate block IDs, duplicate references, and conflicting placements. Set `locked: true` on a placement, route, or via when later routing passes should preserve that geometry.

The complete example is in [examples/modular-resistors](examples/modular-resistors).

## Automatic routing

The release archive includes [KiCadRoutingTools](https://github.com/drandyhaas/KiCadRoutingTools). It does not make native KiCad files authoritative. The router receives a temporary generated board, and `kil` converts its result into a compact `*.kil.routes.json` cache.

After the normal `kil` installation, routing needs no separate setup:

```console
kil route examples/tiny-controller.kil.json
kil build examples/tiny-controller.kil.json
```

The cache contains normalized segments, vias, the router version when available, and a SHA-256 fingerprint of all routing inputs. `check` and `build` reject a missing or stale cache after placement, net, rule, or seed-route changes.

Route selected nets or one imported block instead of rerouting everything:

```console
kil route board.kil.json --net "/USB_*"
kil route board.kil.json --block controller
```

There is one important boundary. A mapped net such as global `GND` belongs to the whole board. Selecting a block that uses `GND` can therefore expose the complete `GND` net to the router. Lock finished copper when routing blocks incrementally.

The release pins KiCadRoutingTools through [KRT_VERSION](KRT_VERSION). Its MIT license is included in every binary archive. See [THIRD_PARTY.md](THIRD_PARTY.md) for the bundled files and attribution. `--krt` and `--python` can override the bundled copies during development.

KiCadRoutingTools currently has no push-and-shove, blind or buried vias, coarse global-routing pass, or region-specific design rules. For larger boards, route by block or net group and review each result in KiCad.

## Current scope

Version 1 supports:

- one schematic sheet;
- single-unit library symbols and library footprints;
- two copper layers;
- polygon board outlines;
- explicit component placement, route polylines, vias, zones, holes, and silkscreen text;
- recursively imported modules with local coordinates;
- deterministic KiCad 10 generation;
- KiCad netlist, ERC, and DRC validation.

It does not yet support importing existing KiCad projects, round-trip editing, electrical sheet hierarchy, buses, derived or multi-unit symbols, embedded libraries, inner copper layers, or native differential-pair and length-tuning constraints in the IL.

## Development

```console
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The workspace contains two crates:

- `kil-core` implements the model, source mapping, validation, module expansion, library resolution, KiCad generation, routing cache, and publication pipeline.
- `kil` is the command-line interface.

The test suite includes deterministic golden generation, source-span diagnostics, stale route-cache rejection, module transforms, rollback after failed publication, and a 50-part size and compile-time fixture.

Pushing a tag such as `v0.1.0` starts [.github/workflows/release.yml](.github/workflows/release.yml). The workflow builds all supported binaries, bundles the pinned router, generates `SHA256SUMS`, and publishes the GitHub Release used by both installers.

## License

Licensed under the [MIT License](LICENSE).
