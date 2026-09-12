use crate::diagnostic::{Diagnostic, Severity};
use crate::kicad::{generate, validate_generated};
use crate::library::LibraryResolver;
use crate::model::{MODULE_SCHEMA_URL, PROJECT_SCHEMA_URL, ResolvedProject};
use crate::modules::{BlockInfo, Origin, resolve};
use crate::routing::{apply_route_cache, extract_route_cache, route_cache_path, write_route_cache};
use crate::source::{Module, Project};
use crate::source_map::SourceMap;
use crate::validate::{validate_basic, validate_libraries};
use indexmap::IndexMap;
use schemars::schema_for;
use serde::Serialize;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::Builder;

#[derive(Debug, Clone)]
pub struct LoadedProject {
    pub layouts: Vec<crate::layout::LayoutPlan>,
    pub source_document: Option<Project>,
    pub origins: IndexMap<String, Origin>,
    pub project: Option<ResolvedProject>,
    pub blocks: IndexMap<String, BlockInfo>,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub kicad_cli: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RouteOptions {
    pub input: PathBuf,
    pub kicad_cli: Option<PathBuf>,
    pub krt: Option<PathBuf>,
    pub python: Option<PathBuf>,
    pub nets: Vec<String>,
    pub block: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InspectOptions {
    pub input: PathBuf,
    pub component: Option<String>,
    pub net: Option<String>,
    pub block: Option<String>,
    pub region: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitClass {
    Success,
    Invalid,
    DesignViolations,
}

impl ExitClass {
    pub fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Invalid => 1,
            Self::DesignViolations => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildOutcome {
    pub exit: ExitClass,
    pub diagnostics: Vec<Diagnostic>,
    pub output_dir: Option<PathBuf>,
    #[serde(skip)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteOutcome {
    pub exit: ExitClass,
    pub diagnostics: Vec<Diagnostic>,
    pub cache_file: Option<PathBuf>,
    #[serde(skip)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectOutcome {
    pub exit: ExitClass,
    pub diagnostics: Vec<Diagnostic>,
    pub data: Option<Value>,
    #[serde(skip)]
    pub source: String,
}

pub fn load_project(path: &Path) -> LoadedProject {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            return LoadedProject {
                project: None,
                source_document: None,
                layouts: vec![],
                origins: IndexMap::new(),
                blocks: IndexMap::new(),
                source: String::new(),
                diagnostics: vec![Diagnostic::error(
                    "IO001",
                    format!("cannot read input: {err}"),
                    path,
                )],
            };
        }
    };
    match serde_json::from_str::<Project>(&source) {
        Ok(document) => {
            let resolution = resolve(&document, path);
            let project = resolution.project;
            let mut diagnostics = resolution.diagnostics;
            diagnostics.extend(
                validate_basic(&project, path, &source)
                    .into_iter()
                    .filter(|d| !matches!(d.code.as_str(), "PCB003" | "SCH001")),
            );
            remap_diagnostics(&mut diagnostics, &resolution.origins);
            LoadedProject {
                project: Some(project),
                source_document: Some(document),
                layouts: resolution.layouts,
                origins: resolution.origins,
                blocks: resolution.blocks,
                source,
                diagnostics,
            }
        }
        Err(err) => {
            let span = Some(
                SourceMap::new(&source)
                    .at_line_column(err.line().saturating_sub(1), err.column().saturating_sub(1)),
            );
            LoadedProject {
                project: None,
                source_document: None,
                layouts: vec![],
                origins: IndexMap::new(),
                blocks: IndexMap::new(),
                source,
                diagnostics: vec![
                    Diagnostic::error("JSON001", err.to_string(), path).with_span(span),
                ],
            }
        }
    }
}

fn remap_diagnostics(diagnostics: &mut [Diagnostic], origins: &IndexMap<String, Origin>) {
    for diagnostic in diagnostics {
        let Some(path) = diagnostic.path.clone() else {
            continue;
        };
        if let Some((id, origin)) = origins
            .iter()
            .filter(|(id, _)| {
                path == format!("/components/{id}")
                    || path.starts_with(&format!("/components/{id}/"))
            })
            .max_by_key(|(id, _)| id.len())
        {
            let suffix = &path[format!("/components/{id}").len()..];
            diagnostic.file = origin.file.display().to_string();
            diagnostic.path = Some(format!("{}{suffix}", origin.path));
        } else if path.starts_with("/nets/") {
            diagnostic.path = Some(format!("/circuit{path}"));
        }
        if let Ok(source) = fs::read_to_string(&diagnostic.file) {
            diagnostic.span =
                SourceMap::new(&source).span_for_path(diagnostic.path.as_deref().unwrap_or(""));
        }
    }
}

pub fn check(options: &BuildOptions) -> BuildOutcome {
    run(options, false)
}

pub fn build(options: &BuildOptions) -> BuildOutcome {
    run(options, true)
}

pub fn inspect(options: &InspectOptions) -> InspectOutcome {
    let loaded = load_project(&options.input);
    let mut diagnostics = loaded.diagnostics;
    let Some(project) = loaded.project else {
        return InspectOutcome {
            exit: ExitClass::Invalid,
            diagnostics,
            data: None,
            source: loaded.source,
        };
    };
    if has_errors(&diagnostics) {
        return InspectOutcome {
            exit: ExitClass::Invalid,
            diagnostics,
            data: None,
            source: loaded.source,
        };
    }

    let data = if let Some(refdes) = &options.component {
        project.components.get(refdes).map(|component| {
            let nets: Vec<_> = project
                .nets
                .iter()
                .filter(|(_, endpoints)| {
                    endpoints
                        .iter()
                        .any(|endpoint| endpoint.starts_with(&format!("{refdes}.")))
                })
                .map(|(name, endpoints)| json!({"name": name, "endpoints": endpoints}))
                .collect();
            json!({
                "kind": "component",
                "ref": refdes,
                "component": component,
                "nets": nets,
                "schematic": project.schematic.placement.iter().filter(|(_,p)|p.part==*refdes).collect::<IndexMap<_,_>>(),
                "origin": loaded.origins.get(refdes),
                "geometry_stage": "preliminary",
                "pcb": project.pcb.placement.get(refdes)
            })
        })
    } else if let Some(name) = &options.net {
        project.nets.get(name).map(|endpoints| {
            json!({
                "kind": "net",
                "name": name,
                "endpoints": endpoints,
                "schematic_wires": project.schematic.wires.iter().filter(|wire| wire.net == *name).collect::<Vec<_>>(),
                "schematic_labels": project.schematic.labels.iter().filter(|label| label.net == *name).collect::<Vec<_>>(),
                "routes": project.pcb.routes.get(name),
                "vias": project.pcb.vias.iter().filter(|via| via.net == *name).collect::<Vec<_>>(),
                "zones": project.pcb.zones.iter().filter(|zone| zone.net == *name).collect::<Vec<_>>()
            })
        })
    } else if let Some(id) = &options.block {
        loaded.blocks.get(id).map(|block| {
            let components: IndexMap<_, _> = block
                .components
                .iter()
                .filter_map(|refdes| project.components.get(refdes).map(|value| (refdes, value)))
                .collect();
            json!({
                "kind": "block",
                "block": block,
                "components": components,
                "nets": block.nets.iter().filter_map(|name| project.nets.get(name).map(|endpoints| (name, endpoints))).collect::<IndexMap<_, _>>(),
                "schematic_placement": block.components.iter().filter_map(|refdes| project.schematic.placement.get(refdes).map(|value| (refdes, value))).collect::<IndexMap<_, _>>(),
                "pcb_placement": block.components.iter().filter_map(|refdes| project.pcb.placement.get(refdes).map(|value| (refdes, value))).collect::<IndexMap<_, _>>()
            })
        })
    } else if let Some([x1, y1, x2, y2]) = options.region {
        let min_x = x1.min(x2);
        let max_x = x1.max(x2);
        let min_y = y1.min(y2);
        let max_y = y1.max(y2);
        let inside = |point: &[f64; 2]| {
            point[0] >= min_x && point[0] <= max_x && point[1] >= min_y && point[1] <= max_y
        };
        Some(json!({
            "kind": "region",
            "geometry_stage": "preliminary",
            "bounds": [min_x, min_y, max_x, max_y],
            "components": project.pcb.placement.iter().filter(|(_, placement)| inside(&placement.at)).collect::<IndexMap<_, _>>(),
            "routes": project.pcb.routes.iter().filter_map(|(net, routes)| {
                let hits: Vec<_> = routes.iter().filter(|route| route.path.iter().any(&inside)).collect();
                (!hits.is_empty()).then_some((net, hits))
            }).collect::<IndexMap<_, _>>(),
            "vias": project.pcb.vias.iter().filter(|via| inside(&via.at)).collect::<Vec<_>>(),
            "holes": project.pcb.holes.iter().filter(|hole| inside(&hole.at)).collect::<Vec<_>>(),
            "silk": project.pcb.silk.iter().filter(|text| inside(&text.at)).collect::<Vec<_>>()
        }))
    } else {
        Some(json!({
            "kind": "summary",
            "geometry_stage": "preliminary",
            "project": project.project,
            "components": project.components.len(),
            "nets": project.nets.len(),
            "blocks": loaded.blocks.values().collect::<Vec<_>>(),
            "pcb": {
                "outline": project.pcb.outline,
                "placements": project.pcb.placement.len(),
                "route_polylines": project.pcb.routes.values().map(Vec::len).sum::<usize>(),
                "vias": project.pcb.vias.len(),
                "zones": project.pcb.zones.len()
            }
        }))
    };

    if data.is_none() {
        let (kind, value) = if let Some(value) = &options.component {
            ("component", value.as_str())
        } else if let Some(value) = &options.net {
            ("net", value.as_str())
        } else {
            ("block", options.block.as_deref().unwrap_or_default())
        };
        diagnostics.push(
            Diagnostic::error(
                "INSPECT001",
                format!("unknown {kind} '{value}'"),
                &options.input,
            )
            .with_help("run 'kil inspect FILE' to list the project summary"),
        );
    }
    InspectOutcome {
        exit: if data.is_some() {
            ExitClass::Success
        } else {
            ExitClass::Invalid
        },
        diagnostics,
        data,
        source: loaded.source,
    }
}

pub fn route(options: &RouteOptions) -> RouteOutcome {
    let loaded = load_project(&options.input);
    let mut diagnostics = loaded.diagnostics;
    let Some(mut project) = loaded.project else {
        return invalid_route(diagnostics, loaded.source);
    };
    if has_errors(&diagnostics) {
        return invalid_route(diagnostics, loaded.source);
    }
    let Some(policy) = project.pcb.routing.clone() else {
        diagnostics.push(
            Diagnostic::error(
                "ROUTE007",
                "pcb.routing is required by 'kil route'",
                &options.input,
            )
            .with_help("add a kicad-routing-tools routing policy to the PCB section"),
        );
        return invalid_route(diagnostics, loaded.source);
    };
    if let Err(message) = route_cache_path(&options.input, &project) {
        diagnostics.push(Diagnostic::error("ROUTE001", message, &options.input));
        return invalid_route(diagnostics, loaded.source);
    }
    let Some(cli) = resolve_kicad_cli(options.kicad_cli.as_deref()) else {
        diagnostics.push(
            Diagnostic::error("KICAD001", "kicad-cli was not found", &options.input)
                .with_help("install KiCad 10, add kicad-cli to PATH, or pass --kicad-cli"),
        );
        return invalid_route(diagnostics, loaded.source);
    };
    let Some(router) = resolve_krt(options.krt.as_deref()) else {
        diagnostics.push(
            Diagnostic::error(
                "ROUTE008",
                "KiCadRoutingTools route.py was not found",
                &options.input,
            )
            .with_help("reinstall kil, or pass --krt PATH to a KiCadRoutingTools checkout"),
        );
        return invalid_route(diagnostics, loaded.source);
    };
    let python = resolve_python(options.python.as_deref());
    if !python_works(&python) {
        diagnostics.push(
            Diagnostic::error(
                "ROUTE009",
                "Python 3.9+ with numpy, scipy and shapely was not found",
                &options.input,
            )
            .with_help("reinstall kil, or pass --python PATH to a compatible Python environment"),
        );
        return invalid_route(diagnostics, loaded.source);
    }
    let project_dir = options.input.parent().unwrap_or_else(|| Path::new("."));
    let resolver = LibraryResolver::discover(project_dir, Some(&cli));
    let (libraries, library_diags) = resolver.resolve_all(&project, &options.input, &loaded.source);
    diagnostics.extend(library_diags);
    if !has_errors(&diagnostics) {
        match crate::lock::verify(&options.input, &libraries) {
            Ok(fingerprint) => project.library_fingerprint = fingerprint,
            Err(d) => diagnostics.push(*d),
        }
    }

    diagnostics.extend(crate::layout::resolve_layout(
        &mut project,
        &loaded.layouts,
        &libraries,
        true,
    ));
    diagnostics.extend(validate_basic(&project, &options.input, &loaded.source));
    diagnostics.extend(validate_libraries(
        &project,
        &libraries,
        &options.input,
        &loaded.source,
    ));
    remap_diagnostics(&mut diagnostics, &loaded.origins);
    if has_errors(&diagnostics) {
        return invalid_route(diagnostics, loaded.source);
    }
    let selected_nets = if !options.nets.is_empty() {
        options.nets.clone()
    } else if let Some(block_id) = &options.block {
        let Some(block) = loaded.blocks.get(block_id) else {
            diagnostics.push(
                Diagnostic::error(
                    "ROUTE017",
                    format!("unknown block '{block_id}'"),
                    &options.input,
                )
                .with_help("run 'kil inspect FILE' to list block ids"),
            );
            return invalid_route(diagnostics, loaded.source);
        };
        block.nets.iter().map(|net| format!("/{net}")).collect()
    } else {
        policy.nets.clone()
    };

    if !project
        .nets
        .keys()
        .any(|n| crate::routing::selected(&selected_nets, n))
    {
        diagnostics.push(Diagnostic::error(
            "ROUTE018",
            "net selection matches no nets",
            &options.input,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    let routing_input = project.clone();
    if route_cache_path(&options.input, &project).is_ok_and(|p| p.is_file())
        && let Err(d) = apply_route_cache(&mut project, &options.input)
    {
        let full = project
            .nets
            .keys()
            .all(|n| crate::routing::selected(&selected_nets, n));
        if !full {
            diagnostics.push(*d);
            return invalid_route(diagnostics, loaded.source);
        }
    }
    let route_seed = project.clone();
    // Copper outside this operation's selection is immutable router input.
    for (net, routes) in &mut project.pcb.routes {
        if !crate::routing::selected(&selected_nets, net) {
            for route in routes {
                route.locked = true;
            }
        }
    }
    for via in &mut project.pcb.vias {
        if !crate::routing::selected(&selected_nets, &via.net) {
            via.locked = true;
        }
    }
    let staging = match Builder::new().prefix(".kil-route-").tempdir() {
        Ok(dir) => dir,
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                "IO002",
                format!("cannot create routing staging directory: {err}"),
                &options.input,
            ));
            return invalid_route(diagnostics, loaded.source);
        }
    };
    let baseline = staging.path().join("baseline.kicad_pcb");
    if let Err(e) = generate(&route_seed, &libraries).and_then(|g| fs::write(&baseline, g.pcb)) {
        diagnostics.push(Diagnostic::error("GEN001", e.to_string(), &options.input));
        return invalid_route(diagnostics, loaded.source);
    }
    let before = match extract_route_cache(&route_seed, &baseline, None) {
        Ok(cache) => cache,
        Err(e) => {
            diagnostics.push(Diagnostic::error("ROUTE012", e, &options.input));
            return invalid_route(diagnostics, loaded.source);
        }
    };
    if let Err(err) = generate(&project, &libraries)
        .and_then(|g| g.write_to(staging.path(), &project.project.name))
    {
        diagnostics.push(Diagnostic::error(
            "GEN001",
            format!("cannot write staged project: {err}"),
            &options.input,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    let input_board = staging
        .path()
        .join(format!("{}.kicad_pcb", project.project.name));
    let pre_route_drc = staging.path().join("pre-route-drc.json");
    if let Err(message) = run_command_extra(
        &cli,
        &[
            "pcb",
            "drc",
            "--format",
            "json",
            "--severity-all",
            "--refill-zones",
            "--save-board",
            "--output",
        ],
        Some(&pre_route_drc),
        &input_board,
    ) {
        diagnostics.push(Diagnostic::error(
            "KICAD005",
            format!("KiCad could not prepare the staged board for routing: {message}"),
            &options.input,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    let routed_board = staging
        .path()
        .join(format!("{}.routed.kicad_pcb", project.project.name));

    let initial_footprints = match footprint_state(&input_board) {
        Ok(state) => state,
        Err(e) => {
            diagnostics.push(Diagnostic::error("ROUTE021", e, &options.input));
            return invalid_route(diagnostics, loaded.source);
        }
    };
    let mut command = Command::new(&python);
    command.arg(&router).arg(&input_board).arg(&routed_board);
    if !selected_nets.is_empty() {
        command.arg("--nets").args(&selected_nets);
    }
    command
        .arg("--layers")
        .args(project.pcb.stackup.copper_layers());
    command.args(&policy.extra_args);
    let result = match command.output() {
        Ok(result) => result,
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                "ROUTE010",
                format!("cannot start KiCadRoutingTools: {err}"),
                &options.input,
            ));
            return invalid_route(diagnostics, loaded.source);
        }
    };
    if !result.status.success() || !routed_board.is_file() {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_owned();
        diagnostics.push(
            Diagnostic::error(
                "ROUTE011",
                format!(
                    "KiCadRoutingTools failed: {}",
                    if stderr.is_empty() { stdout } else { stderr }
                ),
                &options.input,
            )
            .with_help("inspect router output, placement and routing policy"),
        );
        return invalid_route(diagnostics, loaded.source);
    }
    let router_stdout = String::from_utf8_lossy(&result.stdout);
    diagnostics.extend(router_summary_diagnostics(&router_stdout, &options.input));

    let routed_state = match footprint_state(&routed_board) {
        Ok(state) => state,
        Err(e) => {
            diagnostics.push(Diagnostic::error("ROUTE021", e, &options.input));
            return invalid_route(diagnostics, loaded.source);
        }
    };
    if initial_footprints != routed_state {
        diagnostics.push(Diagnostic::error(
            "ROUTE019",
            "router changed footprint placement; placement changes require source edits",
            &options.input,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    if let Err(err) = fs::copy(&routed_board, &input_board) {
        diagnostics.push(Diagnostic::error(
            "ROUTE013",
            format!("cannot stage routed board for validation: {err}"),
            &options.input,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    let router_version = router
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("VERSION"))
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|version| version.trim().to_owned());
    let mut cache = match extract_route_cache(&routing_input, &input_board, router_version) {
        Ok(cache) => cache,
        Err(message) => {
            diagnostics.push(Diagnostic::error("ROUTE012", message, &options.input));
            return invalid_route(diagnostics, loaded.source);
        }
    };

    if let Err(e) = crate::routing::preserve_copper(&before, &mut cache, &selected_nets) {
        diagnostics.push(Diagnostic::error("ROUTE020", e, &options.input));
        return invalid_route(diagnostics, loaded.source);
    }
    crate::routing::normalize_cache_order(&mut cache, &routing_input);
    let mut normalized = routing_input.clone();
    normalized.pcb.routes = cache.routes.clone();
    normalized.pcb.vias = cache.vias.clone();
    let normalized_diagnostics = validate_basic(&normalized, &options.input, &loaded.source);
    if has_errors(&normalized_diagnostics) {
        diagnostics.extend(normalized_diagnostics);
        return invalid_route(diagnostics, loaded.source);
    }
    if let Err(e) = generate(&normalized, &libraries)
        .and_then(|g| g.write_to(staging.path(), &normalized.project.name))
    {
        diagnostics.push(Diagnostic::error("GEN001", e.to_string(), &options.input));
        return invalid_route(diagnostics, loaded.source);
    }
    let validation = validate_with_kicad(&cli, staging.path(), &normalized, &options.input);
    let invalid = validation
        .iter()
        .any(|d| d.severity == Severity::Error && d.code.starts_with("KICAD"));
    diagnostics.extend(validation);
    if invalid {
        return invalid_route(diagnostics, loaded.source);
    }
    if cache.routes.values().all(Vec::is_empty)
        && cache.vias.is_empty()
        && project.nets.values().any(|endpoints| endpoints.len() >= 2)
    {
        let summary = router_stdout
            .lines()
            .rev()
            .find(|line| line.starts_with("JSON_SUMMARY_MIN:") || line.starts_with("JSON_SUMMARY:"))
            .unwrap_or("router produced no machine-readable completion summary");
        diagnostics.push(
            Diagnostic::error(
                "ROUTE015",
                format!("router returned success but emitted no copper; {summary}"),
                &options.input,
            )
            .with_help(
                "adjust placement/router options; the previous route cache was not replaced",
            ),
        );
        return invalid_route(diagnostics, loaded.source);
    }
    let cache_file = match route_cache_path(&options.input, &project) {
        Ok(path) => path,
        Err(message) => {
            diagnostics.push(Diagnostic::error("ROUTE001", message, &options.input));
            return invalid_route(diagnostics, loaded.source);
        }
    };
    if let Err(message) = write_route_cache(&cache_file, &cache) {
        diagnostics.push(Diagnostic::error(
            "ROUTE014",
            format!("cannot publish routing cache: {message}"),
            &cache_file,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    let design_errors = diagnostics.iter().any(|diag| {
        diag.severity == Severity::Error && matches!(diag.code.as_str(), "ERC" | "DRC" | "ROUTER")
    });
    RouteOutcome {
        exit: if design_errors {
            ExitClass::DesignViolations
        } else {
            ExitClass::Success
        },
        diagnostics,
        cache_file: Some(cache_file),
        source: loaded.source,
    }
}

fn run(options: &BuildOptions, publish: bool) -> BuildOutcome {
    let loaded = load_project(&options.input);
    let mut diagnostics = loaded.diagnostics;
    let Some(mut project) = loaded.project else {
        return invalid(diagnostics, loaded.source);
    };
    if has_errors(&diagnostics) {
        return invalid(diagnostics, loaded.source);
    }

    let cli = resolve_kicad_cli(options.kicad_cli.as_deref());
    let Some(cli) = cli else {
        diagnostics.push(
            Diagnostic::error("KICAD001", "kicad-cli was not found", &options.input)
                .with_help("install KiCad 10, add kicad-cli to PATH, or pass --kicad-cli"),
        );
        return invalid(diagnostics, loaded.source);
    };
    let project_dir = options.input.parent().unwrap_or_else(|| Path::new("."));
    let resolver = LibraryResolver::discover(project_dir, Some(&cli));
    let (libraries, library_diags) = resolver.resolve_all(&project, &options.input, &loaded.source);
    diagnostics.extend(library_diags);
    if !has_errors(&diagnostics) {
        match crate::lock::verify(&options.input, &libraries) {
            Ok(fingerprint) => project.library_fingerprint = fingerprint,
            Err(d) => diagnostics.push(*d),
        }
    }

    diagnostics.extend(crate::layout::resolve_layout(
        &mut project,
        &loaded.layouts,
        &libraries,
        true,
    ));
    diagnostics.extend(validate_basic(&project, &options.input, &loaded.source));
    diagnostics.extend(validate_libraries(
        &project,
        &libraries,
        &options.input,
        &loaded.source,
    ));
    remap_diagnostics(&mut diagnostics, &loaded.origins);
    if has_errors(&diagnostics) {
        return invalid(diagnostics, loaded.source);
    }

    if project.pcb.routing.is_some() {
        let before_cache = validate_basic(&project, &options.input, &loaded.source);
        if let Err(diagnostic) = apply_route_cache(&mut project, &options.input) {
            diagnostics.push(*diagnostic);
            return invalid(diagnostics, loaded.source);
        }
        diagnostics.extend(
            validate_basic(&project, &options.input, &loaded.source)
                .into_iter()
                .filter(|diagnostic| !before_cache.contains(diagnostic)),
        );
        if has_errors(&diagnostics) {
            return invalid(diagnostics, loaded.source);
        }
    }

    let output_dir = options.output.clone().unwrap_or_else(|| {
        project_dir
            .join("build")
            .join("kicad")
            .join(&project.project.name)
    });
    let staging_parent = if publish {
        output_dir.parent().unwrap_or(project_dir).to_path_buf()
    } else {
        std::env::temp_dir()
    };
    if let Err(err) = fs::create_dir_all(&staging_parent) {
        diagnostics.push(Diagnostic::error(
            "IO002",
            format!("cannot create staging parent: {err}"),
            &options.input,
        ));
        return invalid(diagnostics, loaded.source);
    }
    let staging = match Builder::new()
        .prefix(".kil-staging-")
        .tempdir_in(&staging_parent)
    {
        Ok(dir) => dir,
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                "IO002",
                format!("cannot create staging directory: {err}"),
                &options.input,
            ));
            return invalid(diagnostics, loaded.source);
        }
    };
    if let Err(err) = generate(&project, &libraries)
        .and_then(|g| g.write_to(staging.path(), &project.project.name))
    {
        diagnostics.push(Diagnostic::error(
            "GEN001",
            format!("cannot write staged project: {err}"),
            &options.input,
        ));
        return invalid(diagnostics, loaded.source);
    }
    if let Err(err) = validate_generated(staging.path(), &project.project.name) {
        diagnostics.push(Diagnostic::error("GEN002", err, &options.input));
        return invalid(diagnostics, loaded.source);
    }

    let validation = validate_with_kicad(&cli, staging.path(), &project, &options.input);
    let structural_failure = validation
        .iter()
        .any(|diag| diag.severity == Severity::Error && diag.code.starts_with("KICAD"));
    diagnostics.extend(validation);
    if structural_failure {
        return invalid(diagnostics, loaded.source);
    }

    if publish
        && let Err(err) = publish_atomically(staging.path(), &output_dir, &project.project.name)
    {
        diagnostics.push(Diagnostic::error(
            "IO003",
            format!("atomic publish failed: {err}"),
            &options.input,
        ));
        return invalid(diagnostics, loaded.source);
    }
    let design_errors = diagnostics.iter().any(|diag| {
        diag.severity == Severity::Error && matches!(diag.code.as_str(), "ERC" | "DRC")
    });
    BuildOutcome {
        exit: if design_errors {
            ExitClass::DesignViolations
        } else {
            ExitClass::Success
        },
        diagnostics,
        output_dir: publish.then_some(output_dir),
        source: loaded.source,
    }
}

fn validate_with_kicad(
    cli: &Path,
    directory: &Path,
    project: &ResolvedProject,
    source_file: &Path,
) -> Vec<Diagnostic> {
    let name = &project.project.name;
    let schematic = directory.join(format!("{name}.kicad_sch"));
    let pcb = directory.join(format!("{name}.kicad_pcb"));
    let netlist = directory.join(format!("{name}.net"));
    let erc = directory.join("erc.json");
    let drc = directory.join("drc.json");
    let mut out = Vec::new();
    if let Err(message) = run_command(
        cli,
        [
            "sch",
            "export",
            "netlist",
            "--format",
            "kicadsexpr",
            "--output",
        ],
        Some(&netlist),
        &schematic,
    ) {
        out.push(Diagnostic::error(
            "KICAD002",
            format!("KiCad rejected generated schematic: {message}"),
            &schematic,
        ));
        return out;
    }
    match fs::read_to_string(&netlist)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::connectivity::verify(project, &text))
    {
        Ok(()) => {}
        Err(message) => {
            out.push(Diagnostic::error("KICAD006", message, source_file));
            return out;
        }
    }
    if let Err(message) = run_command(
        cli,
        [
            "sch",
            "erc",
            "--format",
            "json",
            "--severity-all",
            "--output",
        ],
        Some(&erc),
        &schematic,
    ) {
        out.push(Diagnostic::error(
            "KICAD003",
            format!("ERC could not run: {message}"),
            &schematic,
        ));
        return out;
    }
    if let Err(message) = run_command_extra(
        cli,
        &[
            "pcb",
            "drc",
            "--format",
            "json",
            "--severity-all",
            "--schematic-parity",
            "--refill-zones",
            "--save-board",
            "--output",
        ],
        Some(&drc),
        &pcb,
    ) {
        out.push(Diagnostic::error(
            "KICAD004",
            format!("KiCad rejected generated PCB: {message}"),
            &pcb,
        ));
        return out;
    }
    out.extend(read_kicad_report(&erc, source_file, "ERC"));
    out.extend(read_kicad_report(&drc, source_file, "DRC"));
    out
}

fn run_command<const N: usize>(
    cli: &Path,
    args: [&str; N],
    output: Option<&Path>,
    input: &Path,
) -> Result<(), String> {
    run_command_extra(cli, &args, output, input)
}

fn run_command_extra(
    cli: &Path,
    args: &[&str],
    output: Option<&Path>,
    input: &Path,
) -> Result<(), String> {
    let mut command = Command::new(cli);
    command.args(args);
    if let Some(output) = output {
        command.arg(output);
    }
    command.arg(input);
    let result = command.output().map_err(|err| err.to_string())?;
    if result.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn read_kicad_report(path: &Path, source_file: &Path, code: &str) -> Vec<Diagnostic> {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return vec![Diagnostic::error(
                "KICAD007",
                format!("cannot read {code} report: {e}"),
                source_file,
            )];
        }
    };
    let value: Value = match serde_json::from_str(&source) {
        Ok(v) => v,
        Err(e) => {
            return vec![Diagnostic::error(
                "KICAD007",
                format!("invalid {code} report: {e}"),
                source_file,
            )];
        }
    };
    if !value.is_object()
        || !(value.get("violations").is_some_and(Value::is_array)
            || value.get("sheets").is_some_and(Value::is_array))
    {
        return vec![Diagnostic::error(
            "KICAD007",
            format!("unrecognized {code} report structure"),
            source_file,
        )];
    }
    let mut out = Vec::new();
    collect_violations(&value, source_file, code, &mut out);
    out
}

fn collect_violations(value: &Value, file: &Path, code: &str, out: &mut Vec<Diagnostic>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_violations(item, file, code, out);
            }
        }
        Value::Object(map) => {
            let looks_like_violation = map.contains_key("severity")
                && (map.contains_key("description") || map.contains_key("message"));
            if looks_like_violation {
                let severity_text = map
                    .get("severity")
                    .and_then(Value::as_str)
                    .unwrap_or("warning")
                    .to_ascii_lowercase();
                let severity = if severity_text.contains("error") {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                let message = map
                    .get("description")
                    .or_else(|| map.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("KiCad violation");
                let kind = map.get("type").and_then(Value::as_str).unwrap_or(code);
                let mut diag = if severity == Severity::Error {
                    Diagnostic::error(code, format!("{kind}: {message}"), file)
                } else {
                    Diagnostic::warning(code, format!("{kind}: {message}"), file)
                };
                diag.help = Some(
                    "fix the IL source and rebuild; generated KiCad files are not authoritative"
                        .into(),
                );
                out.push(diag);
            } else {
                for child in map.values() {
                    collect_violations(child, file, code, out);
                }
            }
        }
        _ => {}
    }
}

fn router_summary_diagnostics(stdout: &str, source_file: &Path) -> Vec<Diagnostic> {
    let summary = stdout.lines().rev().find_map(|line| {
        line.strip_prefix("JSON_SUMMARY_MIN: ")
            .or_else(|| line.strip_prefix("JSON_SUMMARY: "))
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
    });
    let Some(summary) = summary else {
        return vec![Diagnostic::warning(
            "ROUTE016",
            "router emitted no JSON completion summary",
            source_file,
        )];
    };
    let failed = summary.get("failed").and_then(Value::as_u64).unwrap_or(0);
    let open = summary
        .pointer("/pad_pairs_open/count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let multipoint_deficit = summary
        .get("multipoint_deficit")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if failed == 0 && open == 0 && multipoint_deficit == 0 {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                "ROUTER",
                format!(
                    "KiCadRoutingTools produced a partial route: failed={failed}, open_pad_pairs={open}, multipoint_deficit={multipoint_deficit}"
                ),
                source_file,
            )
            .with_help("the partial cache is preserved for inspection; adjust placement or routing options and rerun"),
        ]
    }
}

fn publish_atomically(staging: &Path, output: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(output)?;
    let backup = output.join(".kil-backup");
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    fs::create_dir(&backup)?;
    let names = [
        format!("{name}.kicad_pro"),
        format!("{name}.kicad_dru"),
        format!("{name}.kicad_sch"),
        format!("{name}.kicad_pcb"),
    ];
    let mut moved_old = Vec::new();
    let mut moved_new = Vec::new();
    let operation = (|| {
        for filename in &names {
            let destination = output.join(filename);
            if destination.exists() {
                fs::rename(&destination, backup.join(filename))?;
                moved_old.push(filename.clone());
            }
        }
        for filename in &names {
            fs::rename(staging.join(filename), output.join(filename))?;
            moved_new.push(filename.clone());
        }
        Ok::<_, std::io::Error>(())
    })();
    if let Err(err) = operation {
        for filename in moved_new.into_iter().rev() {
            let _ = fs::remove_file(output.join(filename));
        }
        for filename in moved_old.into_iter().rev() {
            let _ = fs::rename(backup.join(&filename), output.join(filename));
        }
        let _ = fs::remove_dir_all(&backup);
        return Err(err);
    }
    fs::remove_dir_all(backup)?;
    Ok(())
}

fn resolve_kicad_cli(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return path.is_file().then(|| path.to_path_buf());
    }
    if command_works("kicad-cli") {
        return Some(PathBuf::from("kicad-cli"));
    }
    #[cfg(target_os = "windows")]
    {
        let path = PathBuf::from(r"C:\Program Files\KiCad\10.0\bin\kicad-cli.exe");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn resolve_krt(explicit: Option<&Path>) -> Option<PathBuf> {
    let candidate = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("KICAD_ROUTING_TOOLS").map(PathBuf::from))
        .or_else(|| bundled_root().map(|root| root.join("lib").join("krt")))?;
    if candidate.is_file() {
        return candidate
            .file_name()
            .is_some_and(|name| name == "route.py")
            .then_some(candidate);
    }
    let script = candidate.join("py_router").join("route.py");
    script.is_file().then_some(script)
}

fn resolve_python(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(root) = bundled_root() {
        #[cfg(target_os = "windows")]
        let bundled = root.join("python").join("Scripts").join("python.exe");
        #[cfg(not(target_os = "windows"))]
        let bundled = root.join("python").join("bin").join("python");
        if bundled.is_file() {
            return bundled;
        }
    }
    if command_works_with("python3", &["--version"]) {
        PathBuf::from("python3")
    } else {
        PathBuf::from("python")
    }
}

fn bundled_root() -> Option<PathBuf> {
    bundled_root_from_executable(&std::env::current_exe().ok()?)
}

fn bundled_root_from_executable(executable: &Path) -> Option<PathBuf> {
    let executable = fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    executable.parent()?.parent().map(Path::to_path_buf)
}

fn python_works(program: &Path) -> bool {
    Command::new(program)
        .arg("-c")
        .arg(
            "import sys, numpy, scipy, shapely; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)",
        )
        .output()
        .is_ok_and(|result| result.status.success())
}

fn command_works_with(program: impl AsRef<OsStr>, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|result| result.status.success())
}

fn command_works(program: impl AsRef<OsStr>) -> bool {
    Command::new(program)
        .arg("version")
        .output()
        .is_ok_and(|result| result.status.success())
}

fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|diag| diag.severity == Severity::Error)
}
fn invalid(diagnostics: Vec<Diagnostic>, source: String) -> BuildOutcome {
    BuildOutcome {
        exit: ExitClass::Invalid,
        diagnostics,
        output_dir: None,
        source,
    }
}

fn invalid_route(diagnostics: Vec<Diagnostic>, source: String) -> RouteOutcome {
    RouteOutcome {
        exit: ExitClass::Invalid,
        diagnostics,
        cache_file: None,
        source,
    }
}

pub fn schema() -> Value {
    schema_with_id(
        serde_json::to_value(schema_for!(Project)).expect("schema is serializable"),
        PROJECT_SCHEMA_URL,
    )
}

pub fn module_schema() -> Value {
    schema_with_id(
        serde_json::to_value(schema_for!(Module)).expect("schema is serializable"),
        MODULE_SCHEMA_URL,
    )
}

fn allow_parameter_values(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            for child in map.values_mut() {
                allow_parameter_values(child);
            }
            if !map.contains_key("const")
                && map
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| matches!(t, "number" | "integer" | "boolean"))
            {
                let original = std::mem::take(schema);
                *schema = json!({"anyOf":[original,{"type":"string","pattern":"^\\$\\{[\\s\\S]*\\}(?![\\s\\S])"}]});
            }
        }
        Value::Array(items) => {
            for item in items {
                allow_parameter_values(item);
            }
        }
        _ => {}
    }
}

fn schema_with_id(mut schema: Value, id: &str) -> Value {
    schema["properties"]["format_version"]["const"] = json!(2);
    schema["$defs"]["DistanceConstraint"]["properties"]["max"]["exclusiveMinimum"] = json!(0.0);
    if id == MODULE_SCHEMA_URL {
        allow_parameter_values(&mut schema);
    }
    schema
        .as_object_mut()
        .expect("root schema is an object")
        .insert("$id".into(), Value::String(id.into()));
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_state_rejects_bad_boards_and_ignores_order() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board.kicad_pcb");
        assert!(footprint_state(&board).is_err());
        fs::write(&board, "(kicad_pcb").unwrap();
        assert!(footprint_state(&board).is_err());
        let a = r#"(footprint "R" (layer "F.Cu") (at 1 2 90) (property "Reference" "R1"))"#;
        let b = r#"(footprint "R" (layer "B.Cu") (at 3 4 0) (property "Reference" "R2"))"#;
        fs::write(&board, format!("(kicad_pcb {a} {b})")).unwrap();
        let first = footprint_state(&board).unwrap();
        assert_eq!(first.len(), 2);
        fs::write(&board, format!("(kicad_pcb {b} {a})")).unwrap();
        assert_eq!(first, footprint_state(&board).unwrap());
        fs::write(&board, format!("(kicad_pcb {a} {a})")).unwrap();
        assert!(footprint_state(&board).unwrap_err().contains("duplicate"));
        let hole = r#"(footprint "MountingHole" (uuid "hole-1") (layer "F.Cu") (at 5 6) (property "Reference" ""))"#;
        fs::write(&board, format!("(kicad_pcb {a} {hole})")).unwrap();
        let first = footprint_state(&board).unwrap();
        fs::write(&board, format!("(kicad_pcb {hole} {a})")).unwrap();
        assert_eq!(first, footprint_state(&board).unwrap());
    }

    #[test]
    fn missing_or_malformed_reports_are_structural_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.json");
        assert_eq!(read_kicad_report(&file, &file, "DRC")[0].code, "KICAD007");
        for text in ["{broken", "{}", "[]"] {
            fs::write(&file, text).unwrap();
            assert_eq!(read_kicad_report(&file, &file, "ERC")[0].code, "KICAD007");
        }
    }
    #[cfg(unix)]
    #[test]
    fn bundled_root_follows_symlinked_executable() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("share/kil");
        let installed_binary = install_root.join("bin/kil");
        let launcher = dir.path().join("bin/kil");
        fs::create_dir_all(installed_binary.parent().unwrap()).unwrap();
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        fs::write(&installed_binary, "test").unwrap();
        symlink(&installed_binary, &launcher).unwrap();

        assert_eq!(
            bundled_root_from_executable(&launcher),
            Some(fs::canonicalize(install_root).unwrap())
        );
    }

    #[test]
    fn invalid_json_has_a_source_span() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.kil.json");
        fs::write(&path, r#"{ "format_version": 1, "project": }"#).unwrap();
        let loaded = load_project(&path);
        assert!(loaded.project.is_none());
        assert!(loaded.diagnostics[0].span.is_some());
    }

    #[test]
    fn layout_schema_bounds_match_runtime() {
        for schema in [schema(), module_schema()] {
            let maximum = &schema["$defs"]["DistanceConstraint"]["properties"]["max"];
            let maximum = maximum.get("anyOf").map(|v| &v[0]).unwrap_or(maximum);
            assert_eq!(maximum["exclusiveMinimum"], 0.0);
            let fraction = &schema["$defs"]["EdgeAnchor"]["properties"]["fraction"];
            let fraction = fraction.get("anyOf").map(|v| &v[0]).unwrap_or(fraction);
            assert_eq!(fraction["minimum"], 0.0);
            assert_eq!(fraction["maximum"], 1.0);
        }
    }

    #[test]
    fn schema_exposes_format_version() {
        let root = schema();
        let text = root.to_string();
        assert!(text.contains("format_version"));
        assert!(text.contains("parts"));
        assert_eq!(root["$id"], PROJECT_SCHEMA_URL);
        assert_eq!(root["properties"]["format_version"]["const"], 2);
        assert!(root["properties"]["$schema"].is_object());

        let module = module_schema();
        assert!(module.to_string().contains("module"));
        assert_eq!(module["$id"], MODULE_SCHEMA_URL);
        assert_eq!(module["properties"]["format_version"]["const"], 2);
        assert!(module["properties"]["$schema"].is_object());
    }

    #[test]
    fn schema_hint_is_accepted_but_not_serialized() {
        let source = include_str!("../../../examples/rc-led.kil.json");
        let project: Project = serde_json::from_str(source).unwrap();
        assert_eq!(project.schema.as_deref(), Some(PROJECT_SCHEMA_URL));
        assert!(
            serde_json::to_value(project)
                .unwrap()
                .get("$schema")
                .is_none()
        );
    }

    #[test]
    fn modular_fixture_is_merged_and_transformed() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/modular-resistors/project.kil.json");
        let loaded = load_project(&path);
        assert_eq!(loaded.diagnostics, Vec::<Diagnostic>::new());
        let project = loaded.project.unwrap();
        assert_eq!(project.components.len(), 2);
        assert_eq!(project.pcb.placement["divider/R1"].at, [5.0, 5.0]);
        assert_eq!(project.pcb.placement["divider/R2"].at, [5.0, 9.0]);
        assert_eq!(project.pcb.routes["SIGNAL"][0].path[0], [4.175, 5.0]);
        assert!(project.pcb.routes["SIGNAL"][0].locked);
        assert_eq!(loaded.blocks["divider"].nets.len(), 2);
    }

    #[test]
    fn failed_publish_restores_previous_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output");
        let staging = dir.path().join("staging");
        fs::create_dir_all(&output).unwrap();
        fs::create_dir_all(&staging).unwrap();
        for extension in ["kicad_pro", "kicad_sch", "kicad_pcb"] {
            fs::write(
                output.join(format!("demo.{extension}")),
                format!("old-{extension}"),
            )
            .unwrap();
        }
        fs::write(staging.join("demo.kicad_pro"), "new-project").unwrap();
        fs::write(staging.join("demo.kicad_sch"), "new-schematic").unwrap();
        assert!(publish_atomically(&staging, &output, "demo").is_err());
        for extension in ["kicad_pro", "kicad_sch", "kicad_pcb"] {
            assert_eq!(
                fs::read_to_string(output.join(format!("demo.{extension}"))).unwrap(),
                format!("old-{extension}")
            );
        }
    }
}

/// Read placements by reference (or UUID for mechanical footprints), independent of file order.
#[derive(Debug, PartialEq)]
struct FootprintPlacement {
    at: Option<[f64; 2]>,
    rotation: Option<f64>,
    layer: Option<String>,
}
fn footprint_state(
    path: &Path,
) -> Result<std::collections::BTreeMap<String, FootprintPlacement>, String> {
    let document = kiutils_kicad::PcbFile::read(path)
        .map_err(|e| format!("cannot parse board '{}': {e}", path.display()))?;
    let mut state = std::collections::BTreeMap::new();
    for footprint in &document.ast().footprints {
        // Mechanical footprints have no printed reference; their UUID remains stable.
        let reference = footprint
            .reference
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(|r| format!("reference:{r}"))
            .or_else(|| {
                footprint
                    .uuid
                    .as_deref()
                    .filter(|u| !u.is_empty())
                    .map(|u| format!("uuid:{u}"))
            })
            .ok_or_else(|| {
                format!(
                    "board '{}' has a footprint without a reference or UUID",
                    path.display()
                )
            })?;
        if state
            .insert(
                reference.clone(),
                FootprintPlacement {
                    at: footprint.at,
                    rotation: footprint.rotation,
                    layer: footprint.layer.clone(),
                },
            )
            .is_some()
        {
            return Err(format!(
                "board '{}' has duplicate footprint reference '{reference}'",
                path.display()
            ));
        }
    }
    Ok(state)
}
