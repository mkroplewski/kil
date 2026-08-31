use crate::diagnostic::{Diagnostic, Severity};
use crate::kicad::{generate, validate_generated};
use crate::library::LibraryResolver;
use crate::model::KilProject;
use crate::routing::{apply_route_cache, extract_route_cache, route_cache_path, write_route_cache};
use crate::source_map::SourceMap;
use crate::validate::{validate_basic, validate_libraries};
use schemars::schema_for;
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::Builder;

#[derive(Debug, Clone)]
pub struct LoadedProject {
    pub project: Option<KilProject>,
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

pub fn load_project(path: &Path) -> LoadedProject {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            return LoadedProject {
                project: None,
                source: String::new(),
                diagnostics: vec![Diagnostic::error(
                    "IO001",
                    format!("cannot read input: {err}"),
                    path,
                )],
            };
        }
    };
    match serde_json::from_str::<KilProject>(&source) {
        Ok(project) => {
            let diagnostics = validate_basic(&project, path, &source);
            LoadedProject {
                project: Some(project),
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
                source,
                diagnostics: vec![
                    Diagnostic::error("JSON001", err.to_string(), path).with_span(span),
                ],
            }
        }
    }
}

pub fn check(options: &BuildOptions) -> BuildOutcome {
    run(options, false)
}

pub fn build(options: &BuildOptions) -> BuildOutcome {
    run(options, true)
}

pub fn route(options: &RouteOptions) -> RouteOutcome {
    let loaded = load_project(&options.input);
    let mut diagnostics = loaded.diagnostics;
    let Some(project) = loaded.project else {
        return invalid_route(diagnostics, loaded.source);
    };
    if has_errors(&diagnostics) {
        return invalid_route(diagnostics, loaded.source);
    }
    let Some(policy) = project.pcb.routing.as_ref() else {
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
            .with_help("clone KiCadRoutingTools and pass --krt PATH"),
        );
        return invalid_route(diagnostics, loaded.source);
    };
    let python = options
        .python
        .clone()
        .unwrap_or_else(|| PathBuf::from("python"));
    if !python_works(&python) {
        diagnostics.push(
            Diagnostic::error("ROUTE009", "Python 3.9+ was not found", &options.input)
                .with_help("pass --python PATH to a Python installation with numpy/scipy/shapely"),
        );
        return invalid_route(diagnostics, loaded.source);
    }
    let project_dir = options.input.parent().unwrap_or_else(|| Path::new("."));
    let resolver = LibraryResolver::discover(project_dir, Some(&cli));
    let (libraries, library_diags) = resolver.resolve_all(&project, &options.input, &loaded.source);
    diagnostics.extend(library_diags);
    diagnostics.extend(validate_libraries(
        &project,
        &libraries,
        &options.input,
        &loaded.source,
    ));
    if has_errors(&diagnostics) {
        return invalid_route(diagnostics, loaded.source);
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
    let generated = generate(&project, &libraries);
    if let Err(err) = generated.write_to(staging.path(), &project.project.name) {
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
    let selected_nets = if options.nets.is_empty() {
        &policy.nets
    } else {
        &options.nets
    };
    let mut command = Command::new(&python);
    command.arg(&router).arg(&input_board).arg(&routed_board);
    if !selected_nets.is_empty() {
        command.arg("--nets").args(selected_nets);
    }
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
    if let Err(err) = fs::copy(&routed_board, &input_board) {
        diagnostics.push(Diagnostic::error(
            "ROUTE013",
            format!("cannot stage routed board for validation: {err}"),
            &options.input,
        ));
        return invalid_route(diagnostics, loaded.source);
    }
    let validation =
        validate_with_kicad(&cli, staging.path(), &project.project.name, &options.input);
    let structural_failure = validation
        .iter()
        .any(|diag| diag.severity == Severity::Error && diag.code.starts_with("KICAD"));
    diagnostics.extend(validation);
    if structural_failure {
        return invalid_route(diagnostics, loaded.source);
    }
    let router_version = router
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("VERSION"))
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|version| version.trim().to_owned());
    let cache = match extract_route_cache(&project, &input_board, router_version) {
        Ok(cache) => cache,
        Err(message) => {
            diagnostics.push(Diagnostic::error("ROUTE012", message, &options.input));
            return invalid_route(diagnostics, loaded.source);
        }
    };
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

    if project.pcb.routing.is_some() {
        if let Err(diagnostic) = apply_route_cache(&mut project, &options.input) {
            diagnostics.push(*diagnostic);
            return invalid(diagnostics, loaded.source);
        }
        diagnostics.extend(validate_basic(&project, &options.input, &loaded.source));
        if has_errors(&diagnostics) {
            return invalid(diagnostics, loaded.source);
        }
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
    diagnostics.extend(validate_libraries(
        &project,
        &libraries,
        &options.input,
        &loaded.source,
    ));
    if has_errors(&diagnostics) {
        return invalid(diagnostics, loaded.source);
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
    let generated = generate(&project, &libraries);
    if let Err(err) = generated.write_to(staging.path(), &project.project.name) {
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

    let validation =
        validate_with_kicad(&cli, staging.path(), &project.project.name, &options.input);
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
    name: &str,
    source_file: &Path,
) -> Vec<Diagnostic> {
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
    let Ok(source) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&source) else {
        return Vec::new();
    };
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
        .or_else(|| std::env::var_os("KICAD_ROUTING_TOOLS").map(PathBuf::from))?;
    if candidate.is_file() {
        return candidate
            .file_name()
            .is_some_and(|name| name == "route.py")
            .then_some(candidate);
    }
    let script = candidate.join("py_router").join("route.py");
    script.is_file().then_some(script)
}

fn python_works(program: &Path) -> bool {
    Command::new(program)
        .arg("-c")
        .arg("import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)")
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
    serde_json::to_value(schema_for!(KilProject)).expect("schema is serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn schema_exposes_format_version() {
        let text = schema().to_string();
        assert!(text.contains("format_version"));
        assert!(text.contains("components"));
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
