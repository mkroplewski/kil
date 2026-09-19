#!/usr/bin/env python3
"""Compare two KIL binaries on isolated copies, interleaving cold and unchanged runs."""
import argparse
import json
import pathlib
import platform
import shutil
import statistics
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("before", "after", "project", "kicad-cli", "krt", "python", "out"):
        parser.add_argument(f"--{name}", required=True, type=pathlib.Path)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--net", action="append", default=[])
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    project = args.project.resolve()
    output = args.out.resolve()
    if output.is_relative_to(project.parent):
        parser.error("--out must be outside the source project directory")
    output.mkdir(parents=True, exist_ok=True)
    flags = ["--kicad-cli", str(args.kicad_cli.resolve()), "--krt", str(args.krt.resolve()),
             "--python", str(args.python.resolve()), "--diagnostics", "json"]
    flags += [arg for net in args.net for arg in ("--net", net)]
    rows = []
    for trial in range(args.runs):
        for implementation in ("before", "after"):
            binary = getattr(args, implementation).resolve()
            folder = output / f"{implementation}-{trial}"
            # Refuse to reuse an existing run, which would invalidate the cold comparison.
            shutil.copytree(project.parent, folder,
                            ignore=shutil.ignore_patterns("build", "target", ".git", "tmp", "*.kil.routes.json"))
            for phase in ("cold", "unchanged"):
                started = time.perf_counter()
                result = subprocess.run([str(binary), "route", str(folder / project.name), *flags],
                                        capture_output=True, text=True, check=False)
                elapsed = time.perf_counter() - started
                (folder / f"{phase}.json").write_text(result.stdout)
                (folder / f"{phase}.stderr").write_text(result.stderr)
                if result.returncode not in (0, 2):
                    raise RuntimeError(f"{implementation} {phase} failed; inspect {folder}")
                data = json.loads(result.stdout)
                report = data.get("report", {})
                row = dict(implementation=implementation, trial=trial, phase=phase,
                           seconds=round(elapsed, 3), exit=result.returncode,
                           status=report.get("status"), timings_ms=report.get("timings_ms"),
                           baseline_reused=report.get("baseline_reused"))
                rows.append(row)
                print(json.dumps(row), flush=True)
    medians = {f"{impl}/{phase}": statistics.median(r["seconds"] for r in rows
               if r["implementation"] == impl and r["phase"] == phase)
               for impl in ("before", "after") for phase in ("cold", "unchanged")}
    summary = dict(platform=platform.platform(), project=str(project),
                   binaries={name: str(getattr(args, name).resolve()) for name in ("before", "after")},
                   kiCad=subprocess.check_output([str(args.kicad_cli), "version"], text=True).strip(),
                   medians_seconds=medians, runs=rows)
    (output / "benchmark.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(medians), flush=True)


if __name__ == "__main__":
    main()
