# Routing KIL projects

`kil route` uses the pinned KiCadRoutingTools bundled with KIL. KIL source remains authoritative; never edit native KiCad files or route caches.

## Placement before routing

Run `kil lock FILE` to accept reviewed library contents. A placement or routing edit does not require another lock. Use `kil preview FILE --out build/placement` to generate and validate source geometry without a route cache. Expected opens are reported as a preview warning; real placement errors still fail. Do not route an arbitrary net to unlock a preview.

Resolve copper, courtyard, edge and keepout conflicts before routing. Confirm connector edges, antenna, mechanical envelopes and critical escape lanes before ordinary signals. `kil inspect FILE` reports routing capabilities. Differential-pair impedance/coupling and length tuning are unsupported and must not be represented as satisfied by ordinary D+/D- connectivity.

## Bounded incremental routing

```console
kil route board.kil.json --block controller --timeout-seconds 120
kil route board.kil.json --net 'USB_*' --timeout-seconds 60 --diagnostics json
```

Nets with different layer/via policies are partitioned automatically and routed sequentially against the current accepted geometry. Compatible classes share a run with the strictest selected fabrication floors. Shared nets such as GND still span the entire board. Unselected copper and source-locked geometry are protected.

Only one route process may update a project at a time. `routing.lock` records its PID; remove a stale lock only after verifying that process is no longer running.

The default 120-second budget covers the group routing loop; source preparation and final KiCad validation also take time. A timeout kills the router process group and preserves accepted copper. Two identical attempts without measured progress block another solve. Change the relevant geometry or policy; use `--retry` only when deliberately repeating the same experiment.

Nets already connected by validated KiCad DRC skip the router. Schematic/netlist checks are reused only when their content, library fingerprint and KiCad identity match. Identical generated board, project, rules, schematic and sheet contents can reuse a previously filled board and DRC report; `baseline_reused` makes this explicit. Changed candidates still get fresh PCB DRC and zone filling. Copper-only checks retain the baseline schematic-parity findings because normalization regenerates all footprints and pad net assignments from unchanged source. Full `kil check` and `kil build` always run fresh verification.

## Candidate and accepted copper

Each attempt is retained under `build/routing/PROJECT/attempt-*/`. `latest.json` points to the latest completed comparison. `report.json` contains selection, invalidated nets, phase timings, before/after DRC, exact open endpoints, new/resolved errors and warnings, and acceptance status. Group directories preserve router stdout/stderr, raw candidates, validation and decisions.

When findings remain, open the `repair_views` paths in the report. Clean results skip native repair plots. Each `layers/index.html` links native KiCad plots for every copper layer, including pads, drills, filled zones, outlines, silkscreen and courtyards. Numbered overlays mark every located error, warning and open endpoint in native KiCad millimetres. The first twelve findings also have a crop per layer; `findings.json` records all findings and crop paths. Markers appear on every layer so the agent can compare layers. Candidate-group views show the checked candidate, including rejected candidates; attempt-level views show the resulting accepted geometry. `artifact_errors` and CLI warnings expose plotting failures without changing copper acceptance. The simpler source-coordinate `repair.svg` remains available.

`timings_ms` separates library resolution, generation, zone filling, PCB validation, routing and repair plotting. `prepare`, `baseline_drc`, `group_validation`, and `normalization_drc` are inclusive stage totals, not additional costs to sum. Schematic and baseline validation overlap. Native filling uses the matching KiCad Python API, discovered alongside KiCad or supplied with `KIL_KICAD_PYTHON`. On CLI-only installations without a matching API, validation retains KiCad's combined refill-and-DRC operation and reports `combined_fill_validation` explicitly instead of estimating separate times.

Resolved libraries are cached by selected asset identities, resolved paths and actual dependency bytes. Paths and bytes are checked each invocation, and the project lock is still verified. A library content change, table/path override, missing dependency or corrupt cache triggers resolution again. `library_reused` reports a hit. Native plots are also cached by validated board content and tool identity.

KIL promotes only a candidate that improves connectivity or removes an error without introducing a new DRC error or increasing another net's open count. Safe partial progress may be accepted, but exit 2 still reports remaining design errors. Process success alone never authorizes promotion. Rejected/no-progress results leave the accepted cache unchanged.

To inspect a candidate before promotion:

```console
kil route board.kil.json --net 'CONTROL_*' --candidate-only
kil route board.kil.json --accept build/routing/PROJECT/attempt-ID/candidate.routes.json
```

Acceptance rechecks the source fingerprint, source rules, protected copper and fresh KiCad DRC. It does not run KRT. To reject a candidate, leave it unaccepted; it has no effect on the main cache.

## Placement edits and invalidation

New accepted caches record the source geometry used to route them. Moving a component invalidates its attached nets and cached routes/vias intersecting conservative envelopes around its old and new positions. KIL adds those nets to the selection, retains unaffected copper and validates the whole result. Authored source routes remain authoritative; stale source-authored geometry must be corrected in source.

Changes to rules, libraries, topology, outline, zones, keepouts or seed copper conservatively discard cached copper and select all nets. Legacy caches without a source snapshot also require this full invalidation. `kil check` still rejects a stale cache; `kil route` performs the controlled update. Never move an old cache into source merely to bypass invalidation.

After upgrading KIL, existing `*.routes.json` files may report `ROUTE006` because the fingerprint now includes per-component footprint IDs. Run `kil route FILE` once to regenerate the cache under the current recipe; until then, `kil check` and `kil build` reject the stale cache.

## Layer and fabrication policy

`allowed_layers` controls tracks and, by default, zones. `zone_layers` independently overrides zone layers. An empty list allows all declared layers. `allow_through_vias` overrides the default policy, which permits through-vias only when both outer track layers are allowed.

For an internal GND plane with outer-layer connections:

```json
{
  "nets": ["GND"],
  "clearance": 0.2,
  "minimum_track_width": 0.2,
  "preferred_track_width": 0.3,
  "allowed_layers": ["F.Cu", "B.Cu"],
  "zone_layers": ["In1.Cu"],
  "allow_through_vias": true
}
```

KIL passes minimum width, clearance, via diameter and drill floors to KRT, then validates imported geometry. Through-vias traverse the full stack. Blind/buried vias, native pair coupling and length tuning remain unsupported. Keepouts independently restrict tracks, vias, pads, pours and footprints.

Finish with `kil check` and `kil build`, full native DRC, layer review and handoff requirement review. A render or a lower open count does not establish fabrication readiness.
