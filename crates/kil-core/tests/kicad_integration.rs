//! Explicit integration gates: ignored by ordinary CI, never silently successful.
use kil_core::{BuildOptions, ExitClass, RouteOptions};
use kiutils_sexpr::{Atom, CstDocument, Node};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn cli() -> PathBuf {
    std::env::var_os("KIL_KICAD_CLI")
        .map(PathBuf::from)
        .expect("set KIL_KICAD_CLI to KiCad 10's executable")
}
fn lock(path: &Path) {
    let loaded = kil_core::load_project(path);
    let project = loaded.project.unwrap();
    let (libraries, diagnostics) =
        kil_core::library::LibraryResolver::discover(path.parent().unwrap(), Some(&cli()))
            .resolve_all(&project, path, &loaded.source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    kil_core::lock::write(path, &libraries).unwrap();
}

fn copper_stackup(cst: &CstDocument) -> Vec<(String, f64)> {
    fn items(node: &Node) -> Option<&[Node]> {
        let Node::List { items, .. } = node else {
            return None;
        };
        Some(items)
    }
    fn atom(node: &Node) -> Option<&str> {
        let Node::Atom { atom, .. } = node else {
            return None;
        };
        match atom {
            Atom::Symbol(value) | Atom::Quoted(value) => Some(value),
        }
    }
    fn child<'a>(nodes: &'a [Node], name: &str) -> Option<&'a [Node]> {
        nodes.iter().find_map(|node| {
            let items = items(node)?;
            (items.first().and_then(atom) == Some(name)).then_some(items)
        })
    }

    let root = items(cst.nodes.first().expect("PCB root")).expect("PCB root list");
    let setup = child(root, "setup").expect("PCB setup");
    let stackup = child(setup, "stackup").expect("physical stackup");
    stackup
        .iter()
        .filter_map(|node| {
            let layer = items(node)?;
            if layer.first().and_then(atom) != Some("layer") {
                return None;
            }
            let name = layer.get(1).and_then(atom)?;
            if !name.ends_with(".Cu") {
                return None;
            }
            let thickness = child(layer, "thickness")?.get(1).and_then(atom)?;
            Some((
                name.to_owned(),
                thickness.parse().expect("copper thickness"),
            ))
        })
        .collect()
}
#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI and run --ignored"]
fn checks_repeated_modules_and_detects_schematic_short() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    let mut root: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/modular-resistors/project.kil.json"
    ))
    .unwrap();
    root["circuit"]["instances"]["divider"]["source"] = serde_json::json!("module.json");
    let mut other = root["circuit"]["instances"]["divider"].clone();
    other["pcb"]["at"] = serde_json::json!([18, 5]);
    other["schematic"]["at"] = serde_json::json!([100.33, 50.8]);
    root["circuit"]["instances"]["other"] = other;
    root["pcb"]["outline"] = serde_json::json!([[0, 0], [30, 0], [30, 14], [0, 14]]);
    let mut module: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/modular-resistors/blocks/divider.kil.json"
    ))
    .unwrap();
    let symbols = module["schematic"]
        .as_object_mut()
        .unwrap()
        .remove("symbols")
        .unwrap();
    module["schematic"]["sheets"] = serde_json::json!({"divider": {"symbols": symbols}});
    fs::write(dir.path().join("module.json"), module.to_string()).unwrap();
    fs::write(&input, root.to_string()).unwrap();
    lock(&input);
    let options = BuildOptions {
        input: input.clone(),
        output: Some(dir.path().join("output")),
        kicad_cli: Some(cli()),
    };
    let result = kil_core::build(&options);
    assert_ne!(result.exit, ExitClass::Invalid, "{:?}", result.diagnostics);
    for rotation in [0, 90, 180, 270] {
        let mut simple: serde_json::Value =
            serde_json::from_str(include_str!("../../../examples/two-resistors.kil.json")).unwrap();
        for symbol in simple["schematic"]["symbols"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            symbol["rotation"] = serde_json::json!(rotation);
        }
        fs::write(&input, simple.to_string()).unwrap();
        lock(&input);
        let result = kil_core::check(&options);
        assert_ne!(
            result.exit,
            ExitClass::Invalid,
            "rotation {rotation}: {:?}",
            result.diagnostics
        );
    }
    let mut simple: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/two-resistors.kil.json")).unwrap();
    simple["schematic"]["wires"] =
        serde_json::json!([{"net":"SIGNAL","path":[[50.8,46.99],[50.8,54.61]]}]);
    fs::write(&input, simple.to_string()).unwrap();
    lock(&input);
    let result = kil_core::check(&options);
    assert_eq!(result.exit, ExitClass::Invalid);
    assert!(result.diagnostics.iter().any(|d| d.code == "KICAD006"));
}
#[test]
#[ignore = "requires KiCad 10 and KiCadRoutingTools; set KIL_KICAD_CLI and KIL_KRT"]
fn routes_two_groups_preserves_copper_and_builds_cache() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/routed-divider.kil.json")).unwrap();
    source["pcb"]["stackup"] = serde_json::json!({"layers":4});
    source["rules"]["net_classes"]["signal"]
        .as_object_mut()
        .unwrap()
        .remove("allowed_layers");
    fs::write(&input, source.to_string()).unwrap();
    lock(&input);
    let mut options = RouteOptions {
        input: input.clone(),
        kicad_cli: Some(cli()),
        krt: Some(
            std::env::var_os("KIL_KRT")
                .map(PathBuf::from)
                .expect("set KIL_KRT"),
        ),
        python: std::env::var_os("KIL_PYTHON").map(PathBuf::from),
        nets: vec!["SIGNAL".into()],
        block: None,
        timeout_seconds: 120,
        retry: false,
        candidate_only: false,
        accept: None,
    };
    let a = kil_core::route(&options);
    assert_ne!(a.exit, ExitClass::Invalid, "{:?}", a.diagnostics);
    assert!(
        a.report.artifact_errors.is_empty(),
        "{:?}",
        a.report.artifact_errors
    );
    assert!(!a.report.repair_views.is_empty());
    let layer_index = Path::new(a.report.repair_views.last().unwrap());
    let native = fs::read_to_string(layer_index.parent().unwrap().join("F_Cu.svg")).unwrap();
    assert!(native.contains("Image generated by PCBNEW"));
    assert!(native.contains("kil-findings"));
    let findings: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(layer_index.parent().unwrap().join("findings.json")).unwrap(),
    )
    .unwrap();
    assert!(!findings.as_array().unwrap().is_empty());
    assert!(!findings[0]["crops"].as_array().unwrap().is_empty());
    let first: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(a.cache_file.unwrap()).unwrap()).unwrap();
    options.nets = vec!["GND".into()];
    // A successful router process that makes no progress must never publish.
    let accepted_path = dir.path().join("project.kil.routes.json");
    let accepted_bytes = fs::read(&accepted_path).unwrap();
    let router = options.krt.clone();
    let fake_dir = dir.path().join("fake");
    fs::create_dir(&fake_dir).unwrap();
    fs::write(fake_dir.join("route.py"), "import sys, shutil\nshutil.copyfile(sys.argv[1], sys.argv[2])\nprint('JSON_SUMMARY_MIN: {\"failed\":1,\"pad_pairs_open\":{\"count\":1}}')\n").unwrap();
    options.krt = Some(fake_dir.join("route.py"));
    for _ in 0..2 {
        let rejected = kil_core::route(&options);
        assert_eq!(
            rejected.report.status, "rejected",
            "{:?}",
            rejected.diagnostics
        );
        assert!(rejected.cache_file.is_none());
        assert_eq!(accepted_bytes, fs::read(&accepted_path).unwrap());
    }
    let blocked = kil_core::route(&options);
    assert_eq!(blocked.report.status, "blocked");
    assert!(
        blocked
            .diagnostics
            .iter()
            .any(|d| d.message.contains("two unchanged attempts"))
    );
    fs::write(fake_dir.join("route.py"), "import time\ntime.sleep(30)\n").unwrap();
    options.timeout_seconds = 1;
    let timed_out = kil_core::route(&options);
    assert_eq!(timed_out.report.status, "rejected");
    assert!(
        timed_out
            .diagnostics
            .iter()
            .any(|d| d.message.contains("budget exhausted"))
    );
    assert_eq!(accepted_bytes, fs::read(&accepted_path).unwrap());
    options.timeout_seconds = 120;
    options.krt = router;
    options.candidate_only = true;
    let candidate = kil_core::route(&options);
    assert_eq!(
        candidate.report.status, "candidate",
        "{:?}",
        candidate.diagnostics
    );
    assert_eq!(accepted_bytes, fs::read(&accepted_path).unwrap());
    options.candidate_only = false;
    options.accept = Some(
        candidate
            .report_file
            .unwrap()
            .parent()
            .unwrap()
            .join("candidate.routes.json"),
    );
    let b = kil_core::route(&options);
    options.accept = None;
    let report_path = b.report_file.as_ref().unwrap();
    let latest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            report_path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("latest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(latest["report"], serde_json::to_value(report_path).unwrap());
    assert_eq!(b.exit, ExitClass::Success, "{:?}", b.diagnostics);
    let second: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(b.cache_file.unwrap()).unwrap()).unwrap();
    assert_eq!(
        first["routes"]["SIGNAL"], second["routes"]["SIGNAL"],
        "untouched copper or lock flags changed"
    );
    assert!(
        second["routes"]["GND"]
            .as_array()
            .is_some_and(|routes| !routes.is_empty()),
        "second run produced no copper for GND: {second}"
    );
    let build = kil_core::build(&BuildOptions {
        input: input.clone(),
        output: Some(dir.path().join("output")),
        kicad_cli: Some(cli()),
    });
    assert_eq!(build.exit, ExitClass::Success, "{:?}", build.diagnostics);
    let no_op = kil_core::route(&options);
    assert_eq!(no_op.report.status, "unchanged");
    assert!(no_op.report.baseline_reused);
    assert_eq!(no_op.report.timings_ms.get("router"), Some(&0));
    let output = build.output_dir.unwrap();
    assert!(output.join("routed-divider.kicad_dru").is_file());
    let mut changed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&input).unwrap()).unwrap();
    changed["pcb"]["placement"]["R1"]["at"] = serde_json::json!([6, 5]);
    fs::write(&input, changed.to_string()).unwrap();
    let check = kil_core::check(&BuildOptions {
        input,
        output: None,
        kicad_cli: Some(cli()),
    });
    assert!(
        check.diagnostics.iter().any(|d| d.code == "ROUTE006"),
        "{:?}",
        check.diagnostics
    );
    let repaired = kil_core::route(&options);
    assert_ne!(
        repaired.exit,
        ExitClass::Invalid,
        "{:?}",
        repaired.diagnostics
    );
    assert!(repaired.report.invalidated_nets.contains(&"SIGNAL".into()));
    assert!(repaired.report.invalidated_nets.contains(&"GND".into()));
    assert!(repaired.cache_file.is_some(), "{:?}", repaired.report);
}

#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI"]
fn pad_anchors_match_back_side_rotated_footprints() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("back.kil.json");
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/anchored-divider.kil.json")).unwrap();
    for p in source["pcb"]["placement"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        p["side"] = serde_json::json!("back");
        p["rotation"] = serde_json::json!(90);
    }
    source["pcb"]["placement"]["R2"]["at"] = serde_json::json!([9, 5]);
    for routes in source["pcb"]["routes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        for r in routes.as_array_mut().unwrap() {
            r["layer"] = serde_json::json!("B.Cu");
        }
    }
    source["pcb"]["vias"] = serde_json::json!([]);
    source["pcb"]["zones"] = serde_json::json!([]);
    fs::write(&input, source.to_string()).unwrap();
    lock(&input);
    let result = kil_core::check(&BuildOptions {
        input,
        output: None,
        kicad_cli: Some(cli()),
    });
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
}

#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI"]
fn four_layer_board_preserves_inner_copper_and_planes() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    fs::write(
        &input,
        include_str!("../../../examples/four-layer-divider.kil.json"),
    )
    .unwrap();
    lock(&input);
    let output = dir.path().join("output");
    let result = kil_core::build(&BuildOptions {
        input: input.clone(),
        output: Some(output.clone()),
        kicad_cli: Some(cli()),
    });
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    let board = output.join("four-layer-divider.kicad_pcb");
    let doc = kiutils_kicad::PcbFile::read(&board).unwrap();
    assert_eq!(
        copper_stackup(doc.cst()),
        [
            ("F.Cu".into(), 0.035),
            ("In1.Cu".into(), 0.0175),
            ("In2.Cu".into(), 0.0175),
            ("B.Cu".into(), 0.07),
        ]
    );
    assert!(
        doc.ast()
            .segments
            .iter()
            .any(|s| s.layer.as_deref() == Some("In1.Cu"))
    );
    assert!(
        doc.ast()
            .zones
            .iter()
            .any(|z| z.layer.as_deref() == Some("In2.Cu"))
    );
    let loaded = kil_core::load_project(&input);
    let cache =
        kil_core::routing::extract_route_cache(&loaded.project.unwrap(), &board, None).unwrap();
    assert!(cache.routes["SIGNAL"].iter().any(|r| r.layer == "In1.Cu"));
    assert_eq!(cache.vias.len(), 3);
}

#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI"]
fn multi_sheet_connectivity_and_removed_sheet_publication() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    let mut source: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/multi-sheet-divider.kil.json"
    ))
    .unwrap();
    fs::write(&input, source.to_string()).unwrap();
    lock(&input);
    let output = dir.path().join("output");
    let options = BuildOptions {
        input: input.clone(),
        output: Some(output.clone()),
        kicad_cli: Some(cli()),
    };
    let result = kil_core::build(&options);
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    let sheets = output.join("multi-sheet-divider.sheets");
    assert_eq!(fs::read_dir(&sheets).unwrap().count(), 2);
    let pages = source["schematic"]
        .as_object_mut()
        .unwrap()
        .remove("sheets")
        .unwrap();
    let mut symbols = serde_json::Map::new();
    for page in pages.as_object().unwrap().values() {
        symbols.extend(page["symbols"].as_object().unwrap().clone());
    }
    source["schematic"]["symbols"] = symbols.into();
    fs::write(&input, source.to_string()).unwrap();
    let result = kil_core::build(&options);
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    assert_eq!(fs::read_dir(&sheets).unwrap().count(), 0);
}

#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI"]
fn power_sources_preserve_nets_and_keepouts_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/anchored-divider.kil.json")).unwrap();
    source["circuit"]["power_sources"] = serde_json::json!(["R1.1", "R1.2"]);
    source["pcb"]["keepouts"] =
        serde_json::json!([{"outline":[[0.1,0.1],[0.4,0.1],[0.4,0.4],[0.1,0.4]]}]);
    source["pcb"]["zones"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "net": "GND",
            "layer": "F.Cu",
            "solid": true,
            "outline": [[0.5, 0.5], [11.5, 0.5], [11.5, 13.5], [0.5, 13.5]]
        }));
    fs::write(&input, source.to_string()).unwrap();
    lock(&input);
    let options = BuildOptions {
        input: input.clone(),
        output: None,
        kicad_cli: Some(cli()),
    };
    let result = kil_core::check(&options);
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    source["pcb"]["keepouts"][0] = serde_json::json!({
        "outline": [[4, 4], [6, 4], [6, 10], [4, 10]],
        "layers": ["F.Cu"],
        "tracks": false,
        "vias": false,
        "pads": false,
        "copper_pours": true,
        "footprints": true
    });
    fs::write(&input, source.to_string()).unwrap();
    let result = kil_core::check(&options);
    assert_ne!(result.exit, ExitClass::Invalid, "{:?}", result.diagnostics);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == "DRC" && d.severity == kil_core::Severity::Error)
    );
    source["pcb"]["keepouts"][0]["footprints"] = serde_json::json!(false);
    fs::write(&input, source.to_string()).unwrap();
    let result = kil_core::check(&options);
    assert_eq!(
        result.exit,
        ExitClass::Success,
        "copper-pour-only keepout should allow footprint overlap: {:?}",
        result.diagnostics
    );
}

#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI"]
fn mixed_io_reference_preserves_all_module_connectivity() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    fs::create_dir(dir.path().join("blocks")).unwrap();
    for (name, text) in [
        (
            "project.kil.json",
            include_str!("../../../examples/mixed-io/project.kil.json"),
        ),
        (
            "blocks/analog.kil.json",
            include_str!("../../../examples/mixed-io/blocks/analog.kil.json"),
        ),
        (
            "blocks/digital.kil.json",
            include_str!("../../../examples/mixed-io/blocks/digital.kil.json"),
        ),
    ] {
        fs::write(dir.path().join(name), text).unwrap();
    }
    let mut source: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&input).unwrap()).unwrap();
    source["build"] = serde_json::json!({});
    fs::write(&input, source.to_string()).unwrap();
    let loaded = kil_core::load_project(&input);
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    let project = loaded.project.unwrap();
    assert_eq!(project.components.len(), 203);
    assert_eq!(project.schematic.sheets.len(), 24);
    lock(&input);
    let started = std::time::Instant::now();
    let result = kil_core::build(&BuildOptions {
        input,
        output: Some(dir.path().join("output")),
        kicad_cli: Some(cli()),
    });
    assert_eq!(
        result.exit,
        ExitClass::DesignViolations,
        "{:?}",
        result.diagnostics
    );
    // This source fixture has no route cache. Every electrical error must be a PCB airwire.
    assert!(
        result
            .diagnostics
            .iter()
            .filter(|d| d.severity == kil_core::Severity::Error)
            .all(|d| d.code == "DRC" && d.message.contains("unconnected")),
        "{:?}",
        result.diagnostics
    );
    assert!(
        !result.diagnostics.iter().any(|d| d.code == "ERC"),
        "{:?}",
        result.diagnostics
    );
    let output = result.output_dir.unwrap();
    assert_eq!(
        fs::read_dir(output.join("mixed-io.sheets"))
            .unwrap()
            .count(),
        24
    );
    eprintln!(
        "203 parts, 24 child sheets, {} nets: generated and checked in {:?}",
        project.nets.len(),
        started.elapsed()
    );
}

#[test]
#[ignore = "requires KiCad 10; set KIL_KICAD_CLI"]
fn placement_preview_does_not_require_or_modify_route_cache() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    fs::write(
        &input,
        include_str!("../../../examples/routed-divider.kil.json"),
    )
    .unwrap();
    lock(&input);
    let options = BuildOptions {
        input,
        output: Some(dir.path().join("preview")),
        kicad_cli: Some(cli()),
    };
    let result = kil_core::preview(&options);
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    assert!(result.diagnostics.iter().any(|d| d.code == "PREVIEW001"));
    assert!(!dir.path().join("project.kil.routes.json").exists());
    assert!(
        result
            .output_dir
            .unwrap()
            .join("routed-divider.kicad_pcb")
            .is_file()
    );
    let mut source: serde_json::Value =
        serde_json::from_slice(&fs::read(&options.input).unwrap()).unwrap();
    source["pcb"]["stackup"] = serde_json::json!({"layers":4});
    source["pcb"]["zones"][0]["layer"] = serde_json::json!("In1.Cu");
    source["pcb"]["vias"] = serde_json::json!([{"net":"GND","at":[6,7],"size":0.8,"drill":0.4}]);
    source["rules"]["net_classes"]["ground"] = serde_json::json!({"nets":["GND"],"minimum_track_width":0.2,"preferred_track_width":0.3,"clearance":0.2,"allowed_layers":["F.Cu","B.Cu"],"zone_layers":["In1.Cu"],"allow_through_vias":true});
    fs::write(&options.input, source.to_string()).unwrap();
    let independent = kil_core::preview(&options);
    assert_eq!(
        independent.exit,
        ExitClass::Success,
        "{:?}",
        independent.diagnostics
    );
    source["rules"]["net_classes"]["ground"]["allow_through_vias"] = serde_json::json!(false);
    fs::write(&options.input, source.to_string()).unwrap();
    assert!(
        kil_core::preview(&options)
            .diagnostics
            .iter()
            .any(|d| d.code == "RULE005")
    );
}

#[test]
#[ignore = "requires KiCad 10 and KiCadRoutingTools; set KIL_KICAD_CLI and KIL_KRT"]
fn authored_routes_bootstrap_a_verified_cache() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/anchored-divider.kil.json")).unwrap();
    source["build"] = serde_json::json!({"routing":{"engine":"kicad-routing-tools"}});
    fs::write(&input, source.to_string()).unwrap();
    lock(&input);
    let options = RouteOptions {
        input,
        kicad_cli: Some(cli()),
        krt: Some(std::env::var_os("KIL_KRT").unwrap().into()),
        python: std::env::var_os("KIL_PYTHON").map(Into::into),
        nets: vec![],
        block: None,
        timeout_seconds: 120,
        retry: false,
        candidate_only: false,
        accept: None,
    };
    let first = kil_core::route(&options);
    assert_eq!(first.exit, ExitClass::Success, "{:?}", first.diagnostics);
    assert!(first.report.timings_ms.contains_key("normalization_drc"));
    let bytes = fs::read(first.cache_file.unwrap()).unwrap();
    let second = kil_core::route(&options);
    assert_eq!(second.exit, ExitClass::Success, "{:?}", second.diagnostics);
    assert!(second.report.baseline_reused);
    assert!(second.report.library_reused);
    assert!(!first.report.library_reused);
    assert!(
        first.report.artifact_errors.is_empty(),
        "{:?}",
        first.report.artifact_errors
    );
    assert!(first.report.repair_views.is_empty());
    assert!(first.report.timings_ms.contains_key("generation"));
    assert!(first.report.timings_ms.contains_key("pcb_validation"));
    assert!(!second.report.timings_ms.contains_key("normalization_drc"));
    assert_eq!(bytes, fs::read(second.cache_file.unwrap()).unwrap());
}

#[test]
#[ignore = "requires KiCad 10 with matching Python API; set KIL_KICAD_CLI"]
fn native_fill_preserves_settings_and_matches_cli_drc() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("project.kil.json");
    fs::write(
        &input,
        include_str!("../../../examples/four-layer-divider.kil.json"),
    )
    .unwrap();
    lock(&input);
    let output = dir.path().join("output");
    let result = kil_core::build(&BuildOptions {
        input,
        output: Some(output.clone()),
        kicad_cli: Some(cli()),
    });
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    let board = fs::read_dir(&output)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|s| s == "kicad_pcb"))
        .unwrap();
    let pro = board.with_extension("kicad_pro");
    let settings = fs::read(&pro).unwrap();
    assert!(kil_core::route_workflow::fill_zones(&cli(), &board).unwrap());
    assert_eq!(settings, fs::read(&pro).unwrap());
    let native = dir.path().join("native.json");
    let combined = dir.path().join("combined.json");
    for (report, refill) in [(&native, false), (&combined, true)] {
        let mut command = std::process::Command::new(cli());
        command.args([
            "pcb",
            "drc",
            "--format",
            "json",
            "--severity-all",
            "--schematic-parity",
        ]);
        if refill {
            command.arg("--refill-zones");
        }
        let result = command
            .arg("--output")
            .arg(report)
            .arg(&board)
            .output()
            .unwrap();
        assert!(result.status.success(), "{:?}", result);
    }
    let native = kil_core::route_workflow::DrcSummary::read(&native).unwrap();
    let combined = kil_core::route_workflow::DrcSummary::read(&combined).unwrap();
    assert_eq!(native.errors.len(), combined.errors.len());
    assert_eq!(native.opens.len(), combined.opens.len());
    assert_eq!(native.warnings, combined.warnings);
}
