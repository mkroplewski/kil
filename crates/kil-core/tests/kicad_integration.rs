//! Explicit integration gates: ignored by ordinary CI, never silently successful.
use kil_core::{BuildOptions, ExitClass, RouteOptions};
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
    };
    let a = kil_core::route(&options);
    assert_ne!(a.exit, ExitClass::Invalid, "{:?}", a.diagnostics);
    let first: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(a.cache_file.unwrap()).unwrap()).unwrap();
    options.nets = vec!["GND".into()];
    let b = kil_core::route(&options);
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
    fs::write(&input, source.to_string()).unwrap();
    lock(&input);
    let options = BuildOptions {
        input: input.clone(),
        output: None,
        kicad_cli: Some(cli()),
    };
    let result = kil_core::check(&options);
    assert_eq!(result.exit, ExitClass::Success, "{:?}", result.diagnostics);
    source["pcb"]["keepouts"][0]["outline"] = serde_json::json!([[4, 4], [6, 4], [6, 10], [4, 10]]);
    fs::write(&input, source.to_string()).unwrap();
    let result = kil_core::check(&options);
    assert_ne!(result.exit, ExitClass::Invalid, "{:?}", result.diagnostics);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == "DRC" && d.severity == kil_core::Severity::Error)
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
