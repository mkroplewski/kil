# Routing workflow and performance

KIL now keeps candidate copper separate from accepted copper. It compares KiCad DRC before and after each group, accepts non-regressing progress, preserves the previous cache on failures, and checkpoints accepted groups. Saved candidates can be accepted with a fresh validation pass without invoking KRT again.

The implementation adds placement previews, conservative local invalidation after footprint movement, independent track/zone/via policy, automatic net grouping, bounded router execution, a repeated-failure guard, and per-attempt repair reports. The [agent workflow](../skills/kil/SKILL.md) establishes mechanics and placement before routing and keeps electrical requirements separate from connectivity and DRC. Existing module support already provides relative placement and pad-anchored local copper; the workflow now explicitly calls for using those together.

## Measurements

Measured on macOS with release builds and KiCadRoutingTools 0.21.4 on 2026-09-18. Both binaries used the same KiCad, Python, libraries and source. Each trial used a fresh project copy, followed by an unchanged repeat. Before/after runs were interleaved. The baseline binary included the local compiler fixes already present when this task started.

Three-run medians on the two-resistor routing fixture:

| Workload | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| First route, quieter sample | 8.19 s | 7.44 s | Small difference; cold routing remains dominated by tool startup and validation |
| Unchanged repeat, quieter sample | 4.55 s | 0.42 s | About 11 times faster |
| First route, heavily loaded sample | 31.00 s | 30.29 s | Effectively unchanged within observed variation |
| Unchanged repeat, heavily loaded sample | 21.99 s | 1.67 s | About 13 times faster |

The system load average exceeded 40 during the later sample. Absolute times varied widely, so these measurements establish a repeated-work improvement, not a general cold-routing or A* speedup. [Raw samples](routing-benchmark.json) include all trials and phase timings.

On an isolated copy of the 267-component amplifier, selecting the already-connected `BT5_CT` net required no KRT solve. Bootstrapping and validating the normalized cache took 31.60 s under load; the unchanged repeat took 1.86 s. Both runs correctly returned exit 2 and retained four hole-clearance errors and 97 open connections. This was a pipeline test, not completion of the board.

A Python profile of KRT on the small fixture spent approximately 2.57 s importing modules, including about 2.18 s in dependency startup checks. A separate initial route reported 0.01 s of actual search. This explains why repeatedly launching KRT for already-connected nets was expensive. KRT's search algorithm itself was not changed.

## What is reused

- Library paths and dependency contents are checked on every routing call. Parsed/resolved assets are reused when those bytes and selected identities match; the library lock is still verified. `library_reused` records cache hits.
- Schematic/netlist/ERC results are keyed by generated schematic and sheet content, circuit/component data, library fingerprint, compiler version and KiCad identity.
- Filled boards and DRC results are keyed by the complete generated PCB, project settings, custom rules, schematic and sheets, library fingerprint, KiCad executable/version, relevant configuration and locale/environment inputs.
- A report explicitly says whether its baseline was reused. Corrupt/unreadable validation-cache records fall back to validation.
- Changed candidate copper always gets a fresh KiCad DRC and zone fill. Copper-only validation carries forward source schematic-parity findings, since all footprints and pad net assignments are regenerated from unchanged source.
- First-time normalization of authored copper is separately validated before it becomes a route cache.
- Full `kil check` and `kil build` still run fresh checks.

Placement-only changes clear attached nets and copper intersecting conservative old/new footprint envelopes. Global source changes and legacy caches without snapshots invalidate all derived copper. A whole-board comparison still checks retained geometry. Conservative envelopes can reroute more than the minimum necessary area.

## Reproduce

Build optimized binaries before measuring. Use the same pinned KRT and KiCad installations for both versions. Keep other CPU-intensive work out of the measurement if possible.

```sh
python3 tools/benchmark_route.py \
  --before /path/to/baseline/kil \
  --after /path/to/updated/kil \
  --project examples/routed-divider.kil.json \
  --kicad-cli /path/to/kicad-cli \
  --krt /path/to/krt \
  --python /path/to/python \
  --out target/route-benchmark \
  --runs 3
```

The tool refuses to reuse an existing trial directory. It preserves diagnostics and emits `benchmark.json`; invalid-source/tool failures abort the measurement. Source libraries must already match the project's reviewed lock.

## Verification and remaining boundaries

Regression coverage includes new errors despite lower error counts, connectivity regression on another net, safe partial progress, malformed reports, geometry/tool/library cache invalidation, movement and crossing-copper invalidation, incompatible layer groups, process-group termination, and concurrent update exclusion.

Real KiCad tests exercise preview without a cache, independent ground-plane/track/via rules, byte-for-byte cache preservation on rejection and timeout, repeated-failure blocking, candidate-only routing and explicit acceptance, unchanged-run reuse, movement repair, and normalization of authored copper. Existing compiler, connectivity and publication tests remain in place.

The local checks ran on macOS. Linux/Windows execution remains covered by the repository CI configuration rather than a local run. Differential-pair coupling, impedance verification, length tuning, blind/buried vias and electrical/thermal design signoff are still unsupported. `kil inspect` exposes these boundaries. Fewer opens or a successful render does not prove those requirements.

## Repair reporting follow-up

Native KiCad layer plots now supplement the source-coordinate overview. Every copper layer includes pads, holes and filled zones, with numbered error/warning/open markers, per-layer crops for the first twelve findings, and a complete JSON index. Candidate and final views are separate. Rendering failures are reported explicitly. Plot results are reused for identical validated boards.

The route report separately measures library resolution, generation, zone filling, PCB validation, router execution and repair plotting. Stage totals overlap these detailed measurements, and schematic validation runs concurrently with baseline PCB validation; do not sum all keys. Native zone filling uses a KiCad Python API with exactly the CLI version. CLI-only installations retain combined refill/DRC and label it `combined_fill_validation`. Full check/build validation remains unchanged.

Workflow milestones remain agent guidance. No enforced milestone state machine was added.

A three-run interleaved follow-up on 2026-09-19 compared the prior optimized binary with these reporting/cache additions. Median cold routing was 6.747 → 6.733 s; unchanged routing was 0.248 → 0.233 s. Those small differences are not a new speedup claim. Library-resolution time in the new binary was 24–28 ms cold and 5 ms on unchanged calls. Clean results skip unnecessary repair plots. A separate selected-net attempt with opens produced native layer views and crops successfully. [Follow-up raw samples](routing-reporting-benchmark.json).

The follow-up passed 69 unit tests and targeted real KiCad tests covering routing transactions, library reuse, native artifacts, and matching native-fill versus CLI-refill DRC results without changes to project settings. Linux/Windows API discovery was not exercised locally; CLI-only fallback remains available.
