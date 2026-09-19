# Changelog

## v0.4.0

KIL v0.4.0 overhauls autorouting into a transactional, DRC-gated workflow with placement preview, selective cache invalidation, and safer candidate acceptance.

### Upgrading

- Route-cache fingerprints now include per-component footprint IDs. Existing `*.routes.json` files may report `ROUTE006` under `kil check` / `kil build` until you run `kil route FILE` once to regenerate them.
- Legacy caches without a source snapshot still trigger conservative full invalidation on placement-driven edits.
- Prefer `kil preview FILE` before routing when inspecting placement. Do not route an unrelated net just to create a cache for review.

### Added

- `kil preview` validates and publishes source placement without requiring or changing a route cache.
- Dedicated routing workflow with timed group loops (default 120s), progress/retry guards, `--candidate-only` review, and `--accept PATH` to revalidate a saved candidate without re-running KRT.
- Source snapshots on accepted caches so placement edits invalidate attached nets and intersecting copper while preserving unaffected routes.
- Net-class `zone_layers` and `allow_through_vias` for independent pour and through-via policy, with matching schema, validation, and KiCad custom rules.
- Structured route reports under `build/routing/PROJECT/` (`latest.json`, timings, failing endpoints, repair views).
- Routing performance documentation and benchmark tooling.

### Fixed

- Pad-anchored routes follow front-footprint rotation correctly.
- Footprint invalidation envelopes include arc `mid`/`center` extents.
- DRC validation-cache identity stays stable when KiCad preference files appear for the first time (missing prefs hash like empty ones; CI seeds standard config before integration tests).

### Validation

CI passed on Windows, Linux, and macOS, including the Ubuntu KiCad 10 and routing integration job.

[Full comparison since v0.3.0](https://github.com/mkroplewski/kil/compare/v0.3.0...v0.4.0)

## v0.3.0

KIL v0.3.0 adds format-v2 circuit authoring, reusable modules, multilayer boards, and multi-sheet schematics. This entry covers all changes since v0.1.0; there was no intervening published release.

### Breaking changes and upgrading

- Format 2 replaces format 1. Existing format-1 files must be rewritten to the new schema; there is no automatic migration or native KiCad importer.
- Physical parts now live in `circuit.parts`, connectivity in `circuit.nets`, and module instances in `circuit.instances`. Stable part identities are separate from printed references. Schematic views refer to physical parts and symbol units.
- Module interfaces use explicit public ports and typed parameters. Internal nets remain private to each instance.
- Routing configuration now belongs in `build.routing`.
- Checks, builds, and routing require a matching `*.kil.lock.json`. Run `kil lock FILE` after reviewing resolved library assets, then commit the lock file. Regenerate stale routing caches with `kil route FILE --net '*'`.
- Use the new `kil-v2.schema.json` and `kil-module-v2.schema.json` editor schemas. KiCad 10 and its symbol/footprint libraries remain required.

### Added

- Reusable circuit modules with independent schematic and PCB transforms, explicit terminal aliases, typed parameters, and stable generated object identities.
- Named schematic sheets with circuit-owned connectivity and support for repeated multi-sheet modules.
- Boards with 2 to 32 copper layers, internal routes and planes, explicit dielectric properties, and per-layer copper thicknesses. Mixed outer/inner copper weights are supported through `copper_thicknesses`.
- Placement and route anchors relative to parts, pads, and board edges; fixed/preferred placement and required/preferred distance constraints.
- Net classes with clearance, minimum/preferred track width, allowed copper layers, and emitted KiCad custom rules.
- Keepouts with independent restrictions for tracks, vias, pads, copper pours, and footprints. Existing keepouts retain their exclude-everything defaults.
- External power-source declarations that generate standard KiCad power flags.
- Targeted inspection by component, net, block, or PCB region, plus `kil library show` for resolved symbol pins and properties.
- Four-layer, multi-sheet, anchored-layout, and multi-unit examples, plus a 203-part mixed-I/O reference with 24 repeated channel modules and 24 child sheets.

### Reliability and fixes

- Verify generated schematic connectivity against circuit intent, including accidental connections introduced by drawn wires.
- Lock resolved library contents and reject stale routing caches. Targeted routing preserves unselected copper and locked geometry, and validates normalized output before publishing the cache.
- Stage generated projects and roll back ordinary publication failures, including replacement of generated sheet directories.
- Treat missing or malformed KiCad ERC/DRC reports as build failures.
- Correct symbol/footprint rotation and back-side geometry behavior; add regression coverage for anchors, net classes, modules, and routing ownership.
- Fix GitHub release publication and add real KiCad 10/KiCadRoutingTools integration checks to CI.
- Update agent guidance, examples, and generated schemas for the current format.

[Full comparison since v0.1.0](https://github.com/mkroplewski/kil/compare/v0.1.0...v0.3.0)
