//! Routing transactions, conservative invalidation and machine-readable repair evidence.
use crate::{model::ResolvedProject, routing::RouteCache};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouteReport {
    pub status: String,
    pub baseline_reused: bool,
    pub library_reused: bool,
    pub repair_views: Vec<String>,
    pub artifact_errors: Vec<String>,
    pub reason: String,
    pub selected_nets: Vec<String>,
    pub invalidated_nets: Vec<String>,
    pub before: DrcSummary,
    pub after: DrcSummary,
    pub timings_ms: BTreeMap<String, u64>,
    pub groups: Vec<Vec<String>>,
    pub new_errors: Vec<Value>,
    pub resolved_errors: Vec<Value>,
    pub new_warnings: Vec<Value>,
    pub resolved_warnings: Vec<Value>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DrcSummary {
    pub errors: Vec<Value>,
    pub warnings: usize,
    #[serde(default)]
    pub warning_details: Vec<Value>,
    pub opens: Vec<Value>,
    #[serde(default)]
    pub parity_errors: Vec<Value>,
    #[serde(default)]
    pub parity_warnings: usize,
    #[serde(default)]
    pub parity_warning_details: Vec<Value>,
}
impl DrcSummary {
    pub fn read(path: &Path) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if !value.get("violations").is_some_and(Value::is_array)
            || !value.get("unconnected_items").is_some_and(Value::is_array)
        {
            return Err("DRC report requires violations and unconnected_items arrays".into());
        }
        let mut result = Self::default();
        for key in ["violations", "schematic_parity"] {
            for item in value[key].as_array().into_iter().flatten() {
                if item["severity"] == "error" {
                    result.errors.push(item.clone());
                    if key == "schematic_parity" {
                        result.parity_errors.push(item.clone());
                    }
                } else {
                    result.warnings += 1;
                    result.warning_details.push(item.clone());
                    if key == "schematic_parity" {
                        result.parity_warnings += 1;
                        result.parity_warning_details.push(item.clone());
                    }
                }
            }
        }
        result.opens = value["unconnected_items"].as_array().unwrap().clone();
        Ok(result)
    }
    /// Copper normalization cannot change the generated schematic, component
    /// net assignments or footprints. Carry forward the source parity findings.
    pub fn inherit_parity(&mut self, baseline: &Self) {
        self.parity_errors = baseline.parity_errors.clone();
        self.parity_warnings = baseline.parity_warnings;
        self.parity_warning_details = baseline.parity_warning_details.clone();
        self.warning_details
            .extend(self.parity_warning_details.clone());
        self.errors.extend(self.parity_errors.clone());
        self.warnings += self.parity_warnings;
    }
    pub fn blocking_placement(&self) -> bool {
        self.errors.iter().any(|v| {
            matches!(
                v["type"].as_str().unwrap_or(""),
                "shorting_items"
                    | "clearance"
                    | "tracks_crossing"
                    | "courtyards_overlap"
                    | "copper_edge_clearance"
                    | "items_not_allowed"
            )
        })
    }
}
// UUIDs for routed tracks can change during normalization. Identify findings by their
// type and physical items, retaining locations so one fixed error cannot mask a new one.
fn finding_key(value: &Value) -> String {
    let mut items: Vec<_> = value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            json!({
                "description": i["description"], "pos": i["pos"]
            })
            .to_string()
        })
        .collect();
    items.sort();
    json!([value["type"], value["description"], items]).to_string()
}
pub fn differences(before: &[Value], after: &[Value]) -> Vec<Value> {
    let mut counts = BTreeMap::<String, usize>::new();
    for v in before {
        *counts.entry(finding_key(v)).or_default() += 1;
    }
    after
        .iter()
        .filter(|v| {
            let count = counts.entry(finding_key(v)).or_default();
            if *count > 0 {
                *count -= 1;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect()
}
fn open_net(item: &Value) -> String {
    // KiCad item descriptions include [net] even in localized reports. Fail
    // conservatively to the complete finding identity when that field is absent.
    item["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|i| {
            let s = i["description"].as_str()?;
            let start = s.find('[')?;
            let mut depth = 0;
            for (offset, c) in s[start..].char_indices() {
                if c == '[' {
                    depth += 1;
                }
                if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(s[start + 1..start + offset].to_owned());
                    }
                }
            }
            None
        })
        .unwrap_or_else(|| finding_key(item))
}
fn open_counts(items: &[Value]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(open_net(item)).or_default() += 1;
    }
    counts
}
pub fn regression_reason(before: &DrcSummary, after: &DrcSummary) -> Option<&'static str> {
    if !differences(&before.errors, &after.errors).is_empty() {
        return Some("candidate introduces DRC errors");
    }
    let counts = open_counts(&before.opens);
    if open_counts(&after.opens)
        .iter()
        .any(|(net, n)| *n > *counts.get(net).unwrap_or(&0))
    {
        return Some("candidate regresses connectivity on at least one net");
    }
    None
}
pub fn assess(before: &DrcSummary, after: &DrcSummary) -> (bool, String) {
    if let Some(reason) = regression_reason(before, after) {
        return (false, reason.into());
    }
    if after.opens.len() < before.opens.len() || after.errors.len() < before.errors.len() {
        (
            true,
            "connectivity or DRC improved without regressions".into(),
        )
    } else {
        (
            false,
            "no measured improvement; change placement, constraints or routing order".into(),
        )
    }
}

pub fn snapshot(project: &ResolvedProject, libraries: &crate::library::ResolvedLibraries) -> Value {
    let mut global = serde_json::to_value(project).unwrap();
    global.as_object_mut().unwrap().remove("schematic");
    for key in ["placement", "silk", "routing"] {
        global["pcb"].as_object_mut().unwrap().remove(key);
    }
    let bounds: BTreeMap<_, _> = project
        .pcb
        .placement
        .iter()
        .filter_map(|(id, p)| {
            let lib = libraries.components.get(id)?;
            // A conservative rotation-independent envelope includes pads and graphics.
            // False positives cost a reroute; false negatives could preserve stale copper.
            // Include arc `mid`/`center` so bulging arcs are not under-bounded by endpoints alone.
            let mut radius: f64 = 0.0;
            fn visit(node: &kiutils_sexpr::Node, radius: &mut f64) {
                use kiutils_sexpr::{Atom, Node};
                fn atom(n: &Node) -> Option<&str> {
                    match n {
                        Node::Atom {
                            atom: Atom::Symbol(s) | Atom::Quoted(s),
                            ..
                        } => Some(s),
                        _ => None,
                    }
                }
                if let Node::List { items, .. } = node {
                    if matches!(
                        items.first().and_then(atom),
                        Some("at" | "start" | "end" | "mid" | "center" | "xy" | "size")
                    ) && let (Some(x), Some(y)) = (
                        items
                            .get(1)
                            .and_then(atom)
                            .and_then(|s| s.parse::<f64>().ok()),
                        items
                            .get(2)
                            .and_then(atom)
                            .and_then(|s| s.parse::<f64>().ok()),
                    ) {
                        *radius = radius.max(x.hypot(y));
                    }
                    for item in items {
                        visit(item, radius);
                    }
                }
            }
            visit(&lib.footprint_node, &mut radius);
            let r = 2.0 * radius + project.rules.clearance + project.rules.via_size;
            Some((
                id.clone(),
                [p.at[0] - r, p.at[1] - r, p.at[0] + r, p.at[1] + r],
            ))
        })
        .collect();
    json!({"global": global, "placement": project.pcb.placement, "bounds": bounds})
}

/// Reuse only copper proved unaffected by a placement edit. Other source changes
/// conservatively invalidate all cached copper. Authored copper stays authoritative.
pub fn rebase(cache: &RouteCache, current: &Value, project: &mut ResolvedProject) -> Vec<String> {
    let Some(old) = &cache.snapshot else {
        return project.nets.keys().cloned().collect();
    };
    if old["global"] != current["global"] {
        return project.nets.keys().cloned().collect();
    }
    let moved: BTreeSet<_> = project
        .components
        .keys()
        .filter(|id| old["placement"][*id] != current["placement"][*id])
        .cloned()
        .collect();
    let envelopes: Vec<[f64; 4]> = moved
        .iter()
        .flat_map(|id| [&old["bounds"][id], &current["bounds"][id]])
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    let mut affected: BTreeSet<String> = project
        .nets
        .iter()
        .filter(|(_, ends)| {
            ends.iter().any(|e| {
                crate::validate::parse_endpoint(e).is_some_and(|(id, _)| moved.contains(id))
            })
        })
        .map(|(net, _)| net.clone())
        .collect();
    for (net, routes) in &cache.routes {
        if routes.iter().any(|route| {
            route.path.windows(2).any(|p| {
                let margin = route.width.unwrap_or(project.rules.width(net)) / 2.0;
                envelopes.iter().any(|r| {
                    p[0][0].min(p[1][0]) - margin <= r[2]
                        && p[0][0].max(p[1][0]) + margin >= r[0]
                        && p[0][1].min(p[1][1]) - margin <= r[3]
                        && p[0][1].max(p[1][1]) + margin >= r[1]
                })
            })
        }) {
            affected.insert(net.clone());
        }
    }
    for via in &cache.vias {
        let margin = via.size.unwrap_or(project.rules.via_size) / 2.0;
        if envelopes.iter().any(|r| {
            via.at[0] + margin >= r[0]
                && via.at[0] - margin <= r[2]
                && via.at[1] + margin >= r[1]
                && via.at[1] - margin <= r[3]
        }) {
            affected.insert(via.net.clone());
        }
    }
    for (net, routes) in &cache.routes {
        if !affected.contains(net) {
            project.pcb.routes.insert(net.clone(), routes.clone());
        }
    }
    project.pcb.vias.extend(
        cache
            .vias
            .iter()
            .filter(|via| {
                !affected.contains(&via.net)
                    && !project
                        .pcb
                        .vias
                        .iter()
                        .any(|v| serde_json::to_value(v).ok() == serde_json::to_value(*via).ok())
            })
            .cloned()
            .collect::<Vec<_>>(),
    );
    affected.into_iter().collect()
}

pub fn run_router(
    command: &mut Command,
    directory: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    let stdout =
        fs::File::create(directory.join("router.stdout.log")).map_err(|e| e.to_string())?;
    let stderr =
        fs::File::create(directory.join("router.stderr.log")).map_err(|e| e.to_string())?;
    command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.success());
        }
        if start.elapsed() >= timeout {
            #[cfg(unix)]
            {
                // Kill the process group, including KiCad oracle children.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .output();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err("routing time budget exhausted; accepted copper was preserved".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

pub fn needs_routing(summary: &DrcSummary, nets: &[String]) -> bool {
    summary.opens.iter().any(|item| {
        let net = open_net(item);
        // Reports without a recognizable net name must not cause a false no-op.
        net.starts_with('[')
            || nets
                .iter()
                .any(|n| n.trim_start_matches('/') == net.trim_start_matches('/'))
    })
}
pub fn attempt_key(
    project: &ResolvedProject,
    cache: &RouteCache,
    nets: &[String],
    router: &Path,
    timeout: u64,
) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!([
                crate::routing::routing_fingerprint(project),
                cache.routes,
                cache.vias,
                nets,
                router,
                fs::metadata(router).ok().and_then(|m| m.modified().ok()),
                timeout
            ]))
            .unwrap()
        )
    )
}

/// A source-coordinate overview with open endpoint markers. Each individual DRC
/// pair retains its exact coordinates and item descriptions in the JSON report.
pub fn write_repair_svg(
    path: &Path,
    project: &ResolvedProject,
    summary: &DrcSummary,
) -> Result<(), String> {
    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    let min_x = project
        .pcb
        .outline
        .iter()
        .map(|p| p[0])
        .fold(f64::INFINITY, f64::min);
    let min_y = project
        .pcb
        .outline
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min);
    let max_x = project
        .pcb
        .outline
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = project
        .pcb
        .outline
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fafafa\"/>",
        min_x - 3.,
        min_y - 3.,
        max_x - min_x + 6.,
        max_y - min_y + 6.,
        min_x - 3.,
        min_y - 3.,
        max_x - min_x + 6.,
        max_y - min_y + 6.
    );
    let points = |p: &[[f64; 2]]| {
        p.iter()
            .map(|p| format!("{},{}", p[0], p[1]))
            .collect::<Vec<_>>()
            .join(" ")
    };
    svg.push_str(&format!(
        "<polygon points=\"{}\" fill=\"none\" stroke=\"#222\" stroke-width=\"0.2\"/>",
        points(&project.pcb.outline)
    ));
    for (net, routes) in &project.pcb.routes {
        for r in routes {
            let color = if r.layer == "F.Cu" {
                "#b94747"
            } else {
                "#426ab3"
            };
            svg.push_str(&format!("<polyline points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{}\" opacity=\"0.6\"><title>{} {}</title></polyline>",points(&r.path),r.width.unwrap_or(project.rules.width(net)),escape(net),escape(&r.layer)));
        }
    }
    for (id, p) in &project.pcb.placement {
        svg.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"0.4\" fill=\"#333\"/><text x=\"{}\" y=\"{}\" font-size=\"1.2\">{}</text>",p.at[0],p.at[1],p.at[0]+0.5,p.at[1],escape(&project.components[id].reference)));
    }
    // KiCad board frame is defined in routing::BoardFrame; use the same transform.
    let frame = crate::routing::BoardFrame::new(&project.pcb.outline);
    for (index, open) in summary.opens.iter().enumerate() {
        let pts: Vec<_> = open["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| Some(frame.unmap([i["pos"]["x"].as_f64()?, i["pos"]["y"].as_f64()?])))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        svg.push_str(&format!("<polyline points=\"{}\" fill=\"none\" stroke=\"#c15b00\" stroke-width=\"0.25\" stroke-dasharray=\"1 0.5\"><title>Open {}: {}</title></polyline>",points(&pts),index+1,escape(&open_net(open))));
    }
    svg.push_str("</svg>");
    // Bound the number of crops; the report always retains every open pair.
    for (index, open) in summary.opens.iter().take(12).enumerate() {
        let pts: Vec<_> = open["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| Some(frame.unmap([i["pos"]["x"].as_f64()?, i["pos"]["y"].as_f64()?])))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let x = pts.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min) - 4.0;
        let y = pts.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min) - 4.0;
        let width = pts.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max) - x + 4.0;
        let height = pts.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max) - y + 4.0;
        let start = svg.find("viewBox=\"").unwrap() + 9;
        let end = start + svg[start..].find('"').unwrap();
        let mut crop = svg.clone();
        crop.replace_range(start..end, &format!("{x} {y} {width} {height}"));
        fs::write(
            path.with_file_name(format!("repair-open-{}.svg", index + 1)),
            crop,
        )
        .map_err(|e| e.to_string())?;
    }
    fs::write(path, svg).map_err(|e| e.to_string())
}

/// Only one process may compare/promote copper for a project at a time.
pub struct RunLock(std::path::PathBuf);
impl RunLock {
    pub fn acquire(directory: &Path) -> Result<Self, String> {
        use std::io::Write;
        let path = directory.join("routing.lock");
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|e| format!("cannot acquire {}: {e}; another route may be active. Remove a stale lock only after checking its recorded PID", path.display()))?;
        if let Err(e) = writeln!(file, "{}", std::process::id()) {
            let _ = fs::remove_file(&path);
            return Err(e.to_string());
        }
        Ok(Self(path))
    }
}
impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Share startup work only when one group's fabrication floors fit every net's
/// preferred width. A high-current trunk must not force a signal to its width.
pub fn partition_nets(project: &ResolvedProject, nets: &[String]) -> Vec<Vec<String>> {
    let layers = |net: &str| {
        project
            .pcb
            .stackup
            .copper_layers()
            .into_iter()
            .filter(|l| project.rules.class(net).is_none_or(|c| c.allows(l)))
            .collect::<Vec<_>>()
    };
    let min_width = |net: &str| {
        project
            .rules
            .class(net)
            .map_or(project.rules.minimum_track_width, |c| c.minimum_track_width)
    };
    let clearance = |net: &str| {
        project
            .rules
            .class(net)
            .map_or(project.rules.clearance, |c| c.clearance)
    };
    let vias = |net: &str| project.rules.class(net).is_none_or(|c| c.allows_vias());
    let mut groups: Vec<Vec<String>> = vec![];
    for net in nets {
        let group = groups.iter_mut().find(|g| {
            let first = &g[0];
            let floor = g
                .iter()
                .map(|n| min_width(n))
                .fold(min_width(net), f64::max);
            layers(first) == layers(net)
                && clearance(first) == clearance(net)
                && vias(first) == vias(net)
                && floor <= project.rules.width(net)
                && g.iter().all(|n| floor <= project.rules.width(n))
        });
        if let Some(group) = group {
            group.push(net.clone());
        } else {
            groups.push(vec![net.clone()]);
        }
    }
    groups.sort_by_key(|g| layers(&g[0]).len());
    groups
}

pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    use std::io::Write;
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    {
        let mut writer = std::io::BufWriter::new(temp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn validation_settings(name: &str, bytes: Option<&[u8]>) -> Vec<u8> {
    let value = bytes.and_then(|b| serde_json::from_slice::<Value>(b).ok());
    // Headless commands update file history, working directories and window
    // state. Those are not validation dependencies. Board design settings and
    // severity rules are in the generated .kicad_pro, hashed separately.
    // Missing preference files must hash like empty ones: first KiCad runs create
    // them and must not invalidate an otherwise identical DRC cache key.
    let relevant = match name {
        "kicad_common.json" => {
            let value = value.unwrap_or(Value::Null);
            json!([value["environment"], value["system"]["language"]])
        }
        "pcbnew.json" => {
            let value = value.unwrap_or(Value::Null);
            json!(value["DRC"]["report_all_track_errors"])
        }
        "eeschema.json" => {
            let value = value.unwrap_or(Value::Null);
            json!(value["ERC"]["show_all_errors"])
        }
        _ => return bytes.unwrap_or_default().to_vec(),
    };
    serde_json::to_vec(&relevant).unwrap()
}

pub fn validation_identity(cli: &Path) -> Value {
    use sha2::{Digest, Sha256};
    let environment: BTreeMap<_, _> = std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("KICAD")
                || k.starts_with("LC_")
                || matches!(k.as_str(), "LANG" | "LANGUAGE" | "XDG_CONFIG_HOME")
        })
        .collect();
    let mut roots = vec![];
    // KiCad stores versioned prefs under $KICAD_CONFIG_HOME/10.0 when set.
    if let Some(root) = std::env::var_os("KICAD_CONFIG_HOME") {
        roots.push(std::path::PathBuf::from(root).join("10.0"));
    } else {
        for key in ["APPDATA", "XDG_CONFIG_HOME"] {
            if let Some(root) = std::env::var_os(key) {
                roots.push(std::path::PathBuf::from(root).join("kicad/10.0"));
            }
        }
        for key in ["HOME", "USERPROFILE"] {
            if let Some(root) = std::env::var_os(key) {
                let root = std::path::PathBuf::from(root);
                roots.push(root.join(".config/kicad/10.0"));
                roots.push(root.join("Library/Preferences/kicad/10.0"));
            }
        }
    }
    let mut configuration = BTreeMap::new();
    for name in ["kicad_common.json", "pcbnew.json", "eeschema.json"] {
        let bytes = roots.iter().find_map(|r| fs::read(r.join(name)).ok());
        configuration.insert(
            name.to_string(),
            format!(
                "{:x}",
                Sha256::digest(validation_settings(name, bytes.as_deref()))
            ),
        );
    }
    for name in ["sym-lib-table", "fp-lib-table"] {
        for root in &roots {
            let path = root.join(name);
            if let Ok(bytes) = fs::read(&path) {
                configuration.insert(
                    path.display().to_string(),
                    format!(
                        "{:x}",
                        Sha256::digest(validation_settings(name, Some(&bytes)))
                    ),
                );
            }
        }
    }
    json!([
        "routing-validation-v3",
        std::env::var_os("KIL_KICAD_PYTHON"),
        cli,
        Command::new(cli)
            .arg("version")
            .output()
            .ok()
            .map(|o| o.stdout),
        fs::metadata(cli).ok().and_then(|m| m.modified().ok()),
        environment,
        configuration
    ])
}

/// Content-addressed validation reuse is only for identical generated inputs.
/// Builds/checks and every changed candidate still run KiCad.
pub fn drc_cache_path(
    directory: &Path,
    identity: &Value,
    library_fingerprint: &str,
    g: &crate::kicad::GeneratedProject,
) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let key = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!([
                "kil-drc-v1",
                identity,
                library_fingerprint,
                g.pcb,
                g.project,
                g.design_rules,
                g.schematic,
                g.sheets
            ]))
            .unwrap()
        )
    );
    directory.join(format!("drc-{key}.json"))
}
pub fn restore_drc(path: &Path, board: &Path, report: &Path) -> bool {
    let Some(value) = fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return false;
    };
    let Some(pcb) = value["board"].as_str() else {
        return false;
    };
    if !value["report"]["violations"].is_array() || !value["report"]["unconnected_items"].is_array()
    {
        return false;
    }
    fs::write(board, pcb).is_ok()
        && fs::write(report, serde_json::to_vec(&value["report"]).unwrap()).is_ok()
}
pub fn remember_drc(
    path: &Path,
    board: &Path,
    report: &Path,
    parity_from: Option<&Path>,
) -> Result<(), String> {
    use std::io::Write;
    let mut drc: Value = serde_json::from_slice(&fs::read(report).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if let Some(baseline) = parity_from {
        let before: Value = serde_json::from_slice(&fs::read(baseline).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        drc["schematic_parity"] = before["schematic_parity"].clone();
    }
    let value = json!({"board":fs::read_to_string(board).map_err(|e|e.to_string())?,"report":drc});
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    {
        let mut writer = std::io::BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, &value).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Use KiCad's own API to isolate filling from DRC. A CLI-only installation
/// retains the combined path; its timing is labelled rather than estimated.
pub fn fill_zones(cli: &Path, board: &Path) -> Result<bool, String> {
    let document = kiutils_kicad::PcbFile::read(board).map_err(|e| e.to_string())?;
    if document.ast().zones.is_empty() {
        return Ok(true);
    }
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("KIL_KICAD_PYTHON") {
        candidates.push(p.into());
    }
    if let Some(contents) = cli.parent().and_then(Path::parent) {
        let versions = contents.join("Frameworks/Python.framework/Versions");
        if let Ok(entries) = fs::read_dir(versions) {
            for entry in entries.flatten() {
                let bin = entry.path().join("bin/python3");
                if bin.is_file() {
                    candidates.push(bin);
                }
            }
        }
        for relative in ["bin/python.exe", "bin/python3", "python/bin/python.exe"] {
            let p = contents.join(relative);
            if p.is_file() {
                candidates.push(p);
            }
        }
    }
    candidates.push("python3".into());
    candidates.push("python".into());
    const SCRIPT: &str = r#"
import sys
try:
    import pcbnew
except ImportError:
    sys.exit(77)
if pcbnew.Version().strip() != sys.argv[2].strip():
    sys.exit(77)
board = pcbnew.LoadBoard(sys.argv[1])
if board is None:
    raise RuntimeError('KiCad could not load board')
if not pcbnew.ZONE_FILLER(board).Fill(board.Zones()):
    raise RuntimeError('KiCad zone fill did not complete')
if not pcbnew.SaveBoard(sys.argv[1], board, aSkipSettings=True):
    raise RuntimeError('KiCad could not save filled board')
"#;
    let version = Command::new(cli)
        .arg("version")
        .output()
        .map_err(|e| e.to_string())?;
    let version = String::from_utf8_lossy(&version.stdout);
    for python in candidates {
        let output = match Command::new(python)
            .args(["-c", SCRIPT])
            .arg(board)
            .arg(version.trim())
            .output()
        {
            Ok(output) => output,
            Err(_) => continue,
        };
        if output.status.success() {
            return Ok(true);
        }
        if output.status.code() == Some(77) {
            continue;
        }
        return Err(format!(
            "KiCad zone fill failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn finding_points(finding: &Value) -> Vec<[f64; 2]> {
    finding["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let p = [item["pos"]["x"].as_f64()?, item["pos"]["y"].as_f64()?];
            p.iter().all(|v| v.is_finite()).then_some(p)
        })
        .collect()
}

fn bounds(points: &[[f64; 2]], margin: f64) -> Option<[f64; 4]> {
    if points.is_empty() {
        return None;
    }
    let x = points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min) - margin;
    let y = points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min) - margin;
    Some([
        x,
        y,
        points
            .iter()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max)
            - x
            + margin,
        points
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max)
            - y
            + margin,
    ])
}

fn crop_svg(svg: &str, rect: [f64; 4]) -> Result<String, String> {
    let mut svg = svg.to_string();
    let start = svg.find("viewBox=\"").ok_or("native SVG has no viewBox")? + 9;
    let end = start + svg[start..].find('"').ok_or("invalid native SVG viewBox")?;
    svg.replace_range(
        start..end,
        &format!("{} {} {} {}", rect[0], rect[1], rect[2], rect[3]),
    );
    // Keep intrinsic dimensions consistent with the crop, including in viewers
    // that thumbnail the physical SVG page rather than the viewBox.
    let scale = 1000.0 / rect[2].max(rect[3]);
    for (attribute, size) in [("width", rect[2] * scale), ("height", rect[3] * scale)] {
        let root = svg.find("<svg").ok_or("missing SVG root")?;
        let root_end = root + svg[root..].find('>').ok_or("invalid SVG root")?;
        let marker = format!("{attribute}=\"");
        if let Some(offset) = svg[root..root_end].find(&marker) {
            let start = root + offset + marker.len();
            let end = start + svg[start..].find('"').ok_or("invalid SVG dimension")?;
            svg.replace_range(start..end, &format!("{}px", size.round().max(1.0)));
        }
    }
    Ok(svg)
}

pub struct RepairContext<'a> {
    pub cli: &'a Path,
    pub cache_root: &'a Path,
    pub identity: &'a Value,
}

/// Plot the actual validated KiCad board, including pads, holes and filled zones.
/// Page mode 1 and scale 1 preserve native DRC coordinates without guessed offsets.
pub fn native_repair_views(
    context: &RepairContext<'_>,
    board: &Path,
    directory: &Path,
    project: &ResolvedProject,
    summary: &DrcSummary,
) -> Result<Vec<String>, String> {
    let cli = context.cli;
    let cache_root = context.cache_root;
    use sha2::{Digest, Sha256};
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let layers = project.pcb.stackup.copper_layers();
    let mut hash = Sha256::new();
    hash.update(fs::read(board).map_err(|e| e.to_string())?);
    hash.update(serde_json::to_vec(&json!([1, layers, context.identity])).unwrap());
    let key = format!("{:x}", hash.finalize());
    let cache = cache_root.join("layer-plots").join(key);
    let expected: Vec<_> = layers
        .iter()
        .map(|l| {
            format!(
                "{}-{}.svg",
                board.file_stem().unwrap().to_string_lossy(),
                l.replace('.', "_")
            )
        })
        .collect();
    if !expected.iter().all(|name| cache.join(name).is_file()) {
        fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let output = Command::new(cli)
            .args([
                "pcb",
                "export",
                "svg",
                "--layers",
                &layers.join(","),
                "--common-layers",
                "Edge.Cuts,F.SilkS,B.SilkS,F.CrtYd,B.CrtYd",
                "--mode-multi",
                "--page-size-mode",
                "1",
                "--scale",
                "1",
                "--exclude-drawing-sheet",
                "--output",
            ])
            .arg(&cache)
            .arg(board)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
    }
    let findings: Vec<_> = summary
        .errors
        .iter()
        .map(|v| ("error", v))
        .chain(summary.opens.iter().map(|v| ("open", v)))
        .chain(summary.warning_details.iter().map(|v| ("warning", v)))
        .collect();
    let mut overlay = String::from(
        "<g id=\"kil-findings\" fill=\"none\" stroke=\"#ff8c00\" stroke-width=\"0.15\">",
    );
    let mut index = Vec::new();
    for (n, (kind, finding)) in findings.iter().enumerate() {
        let pts = finding_points(finding);
        let title = xml_escape(&finding.to_string());
        overlay.push_str(&format!(
            "<g id=\"finding-{}\"><title>{title}</title>",
            n + 1
        ));
        for p in &pts {
            overlay.push_str(&format!("<circle cx=\"{}\" cy=\"{}\" r=\"0.8\"/><text x=\"{}\" y=\"{}\" fill=\"#9b4200\" stroke=\"none\" font-size=\"1\">{}</text>", p[0],p[1],p[0]+1.,p[1],n+1));
        }
        if *kind == "open" {
            let points = pts
                .iter()
                .map(|p| format!("{},{}", p[0], p[1]))
                .collect::<Vec<_>>()
                .join(" ");
            overlay.push_str(&format!(
                "<polyline points=\"{points}\" stroke-dasharray=\"0.5 0.3\"/>"
            ));
        }
        overlay.push_str("</g>");
        index.push(json!({"id":n+1, "kind":kind, "finding":finding, "coordinates":"KiCad millimetres", "crops":[]}));
    }
    overlay.push_str("</g>");
    let frame = crate::routing::BoardFrame::new(&project.pcb.outline);
    let outline: Vec<_> = project.pcb.outline.iter().map(|p| frame.map(*p)).collect();
    let mut views = Vec::new();
    for (layer, native) in layers.iter().zip(expected) {
        let svg = fs::read_to_string(cache.join(native)).map_err(|e| e.to_string())?;
        let end = svg.rfind("</svg>").ok_or("invalid native SVG")?;
        let svg = format!("{}{}{}", &svg[..end], overlay, &svg[end..]);
        let name = format!("{}.svg", layer.replace('.', "_"));
        fs::write(
            directory.join(&name),
            crop_svg(&svg, bounds(&outline, 3.).ok_or("empty outline")?)?,
        )
        .map_err(|e| e.to_string())?;
        views.push(name.clone());
        for (n, (_, finding)) in findings.iter().enumerate().take(12) {
            if let Some(rect) = bounds(&finding_points(finding), 3.) {
                let crop = format!("finding-{}-{}", n + 1, name);
                fs::write(directory.join(&crop), crop_svg(&svg, rect)?)
                    .map_err(|e| e.to_string())?;
                index[n]["crops"].as_array_mut().unwrap().push(json!(crop));
            }
        }
    }
    write_json_atomic(&directory.join("findings.json"), &index)?;
    let mut html = String::from(
        "<!doctype html><meta charset=\"utf-8\"><title>KIL repair views</title><h1>Validated board layers</h1><p>All numbered findings appear on every layer for comparison. Orange markers use native KiCad coordinates. Crops cover the first twelve findings; the index contains every finding.</p>",
    );
    for view in &views {
        html.push_str(&format!("<p><a href=\"{view}\">{view}</a></p>"));
    }
    for item in index {
        html.push_str(&format!(
            "<h2>Finding {}: {}</h2><pre>{}</pre>",
            item["id"],
            item["kind"].as_str().unwrap(),
            xml_escape(&serde_json::to_string_pretty(&item["finding"]).unwrap())
        ));
        for crop in item["crops"].as_array().unwrap() {
            let crop = crop.as_str().unwrap();
            html.push_str(&format!("<a href=\"{crop}\">{crop}</a> "));
        }
    }
    fs::write(directory.join("index.html"), html).map_err(|e| e.to_string())?;
    Ok(views)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn open(net: &str, x: f64) -> Value {
        json!({"type":"unconnected_items","severity":"error","description":"Missing connection", "items":[{"description":format!("Pad 1 [{net}] of R1"),"pos":{"x":x,"y":4.0}},{"description":format!("Pad 2 [{net}] of R2"),"pos":{"x":8.0,"y":4.0}}]})
    }
    fn error(x: f64) -> Value {
        json!({"type":"clearance","severity":"error","description":"Too close", "items":[{"description":"Track","pos":{"x":x,"y":0.0}}]})
    }
    #[test]
    fn native_crops_preserve_coordinates_and_reject_missing_viewbox() {
        let points = finding_points(
            &json!({"items":[{"pos":{"x":21.5,"y":34.0}}, {"pos":{"x":25.0,"y":36.0}}]}),
        );
        let rect = bounds(&points, 3.).unwrap();
        assert_eq!(rect, [18.5, 31., 9.5, 8.]);
        let svg = crop_svg(
            "<svg width=\"297mm\" height=\"210mm\" viewBox=\"0 0 297 210\"><circle cx=\"21.5\" cy=\"34\"/></svg>",
            rect,
        )
        .unwrap();
        assert!(svg.contains("viewBox=\"18.5 31 9.5 8\""));
        assert!(svg.contains("cx=\"21.5\" cy=\"34\""));
        assert!(svg.contains("width=\"1000px\""));
        assert!(crop_svg("broken", rect).is_err());
        assert_eq!(xml_escape("<&\""), "&lt;&amp;&quot;");
    }

    #[test]
    fn rejects_new_errors_even_when_counts_fall() {
        let before = DrcSummary {
            errors: vec![error(1.), error(2.)],
            opens: vec![open("A", 0.)],
            ..Default::default()
        };
        let after = DrcSummary {
            errors: vec![error(3.)],
            ..Default::default()
        };
        assert!(!assess(&before, &after).0);
    }
    #[test]
    fn accepts_partial_progress_but_never_trades_another_nets_connectivity() {
        let before = DrcSummary {
            opens: vec![open("A", 0.), open("A", 1.), open("B", 2.)],
            ..Default::default()
        };
        let after = DrcSummary {
            opens: vec![open("A", 0.), open("B", 2.)],
            ..Default::default()
        };
        assert!(assess(&before, &after).0);
        let bad = DrcSummary {
            opens: vec![open("B", 2.), open("B", 3.)],
            ..Default::default()
        };
        assert!(!assess(&before, &bad).0);
        assert!(!assess(&before, &before).0);
    }
    #[test]
    fn parses_localized_net_names_and_does_not_skip_unknown_reports() {
        let d = DrcSummary {
            opens: vec![open("/module/A", 0.)],
            ..Default::default()
        };
        assert!(needs_routing(&d, &["module/A".into()]));
        assert!(!needs_routing(&d, &["B".into()]));
        let indexed = DrcSummary {
            opens: vec![open("/DATA[0]", 0.)],
            ..Default::default()
        };
        assert!(needs_routing(&indexed, &["DATA[0]".into()]));
        assert!(!needs_routing(&indexed, &["DATA[1]".into()]));
        let unknown = DrcSummary {
            opens: vec![json!({"type":"unconnected_items","items":[]})],
            ..Default::default()
        };
        assert!(needs_routing(&unknown, &["B".into()]));
    }
    #[test]
    fn rejects_malformed_drc_instead_of_treating_it_as_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drc.json");
        fs::write(&path, "{\"violations\":[]}").unwrap();
        assert!(DrcSummary::read(&path).is_err());
    }
    #[test]
    fn footprint_envelope_includes_arc_midpoints() {
        let (project, mut libraries) = crate::kicad::tests::fixture();
        let without = snapshot(&project, &libraries);
        let without_bounds: [f64; 4] =
            serde_json::from_value(without["bounds"]["R1"].clone()).unwrap();
        let without_radius = (without_bounds[2] - without_bounds[0]) / 2.0;
        let arc = kiutils_sexpr::parse_one(
            r#"(fp_arc (start 1 0) (mid 0 20) (end -1 0) (layer "F.SilkS") (width 0.12))"#,
        )
        .unwrap()
        .nodes
        .into_iter()
        .next()
        .expect("arc node");
        let kiutils_sexpr::Node::List { items, .. } =
            &mut libraries.components["R1"].footprint_node
        else {
            panic!("expected footprint list");
        };
        items.push(arc);
        let with = snapshot(&project, &libraries);
        let with_bounds: [f64; 4] = serde_json::from_value(with["bounds"]["R1"].clone()).unwrap();
        let with_radius = (with_bounds[2] - with_bounds[0]) / 2.0;
        assert!(
            with_radius > without_radius + 10.0,
            "arc mid should expand envelope: {without_radius} -> {with_radius}"
        );
    }

    #[test]
    fn placement_rebase_invalidates_connected_and_crossing_nets_only() {
        let (mut project, libraries) = crate::kicad::tests::fixture();
        project.nets.insert("NEAR".into(), vec![]);
        project.nets.insert("FAR".into(), vec![]);
        let old = snapshot(&project, &libraries);
        let route = |path| crate::model::Route {
            layer: "F.Cu".into(),
            width: Some(0.2),
            path,
            locked: false,
        };
        let mut cache = RouteCache {
            format_version: 2,
            project: project.project.name.clone(),
            input_fingerprint: String::new(),
            snapshot: Some(old),
            engine: crate::routing::RouterIdentity {
                name: "test".into(),
                version: None,
            },
            routes: Default::default(),
            vias: vec![],
        };
        cache
            .routes
            .insert("NEAR".into(), vec![route(vec![[-100., 5.], [100., 5.]])]);
        cache
            .routes
            .insert("FAR".into(), vec![route(vec![[100., 100.], [101., 101.]])]);
        project.pcb.placement["R1"].at[0] += 1.;
        let current = snapshot(&project, &libraries);
        let affected = rebase(&cache, &current, &mut project);
        assert!(affected.contains(&"SIGNAL".into()));
        assert!(affected.contains(&"GND".into()));
        assert!(affected.contains(&"NEAR".into()));
        assert!(!affected.contains(&"FAR".into()));
        assert!(project.pcb.routes.contains_key("FAR"));
        assert!(!project.pcb.routes.contains_key("NEAR"));
    }
    #[test]
    fn global_rule_change_discards_all_cached_copper() {
        let (mut project, libraries) = crate::kicad::tests::fixture();
        let cache = RouteCache {
            format_version: 2,
            project: project.project.name.clone(),
            input_fingerprint: String::new(),
            snapshot: Some(snapshot(&project, &libraries)),
            engine: crate::routing::RouterIdentity {
                name: "test".into(),
                version: None,
            },
            routes: Default::default(),
            vias: vec![],
        };
        project.rules.clearance += 0.1;
        let current = snapshot(&project, &libraries);
        assert_eq!(
            rebase(&cache, &current, &mut project).len(),
            project.nets.len()
        );
    }
    #[test]
    fn groups_compatible_layers_without_widening_signals_to_power_trunks() {
        let (mut p, _) = crate::kicad::tests::fixture();
        p.pcb.stackup.layers = 4;
        let class = |nets, layers, min, preferred| crate::model::NetClass {
            nets,
            allowed_layers: layers,
            clearance: 0.2,
            minimum_track_width: min,
            preferred_track_width: preferred,
            zone_layers: None,
            allow_through_vias: None,
        };
        p.rules.net_classes.insert(
            "signal".into(),
            class(vec!["SIGNAL".into()], vec!["F.Cu".into()], 0.2, 0.3),
        );
        let nets = vec!["SIGNAL".into(), "GND".into()];
        assert_eq!(partition_nets(&p, &nets).len(), 2);
        p.rules
            .net_classes
            .get_mut("signal")
            .unwrap()
            .allowed_layers
            .clear();
        assert_eq!(partition_nets(&p, &nets).len(), 1);
        p.rules
            .net_classes
            .insert("power".into(), class(vec!["GND".into()], vec![], 1.0, 1.2));
        assert_eq!(partition_nets(&p, &nets).len(), 2);
    }
    #[test]
    fn concurrent_runs_cannot_share_an_accepted_cache() {
        let dir = tempfile::tempdir().unwrap();
        let lock = RunLock::acquire(dir.path()).unwrap();
        assert!(RunLock::acquire(dir.path()).is_err());
        drop(lock);
        assert!(RunLock::acquire(dir.path()).is_ok());
    }
    #[test]
    fn validation_cache_keys_include_geometry_rules_libraries_and_tool_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, libraries) = crate::kicad::tests::fixture();
        let g = crate::kicad::generate(&p, &libraries).unwrap();
        let original = drc_cache_path(dir.path(), &json!("kicad-10"), "libraries-a", &g);
        assert_ne!(
            original,
            drc_cache_path(dir.path(), &json!("kicad-11"), "libraries-a", &g)
        );
        assert_ne!(
            original,
            drc_cache_path(dir.path(), &json!("kicad-10"), "libraries-b", &g)
        );
        p.rules.clearance += 0.1;
        let changed = crate::kicad::generate(&p, &libraries).unwrap();
        assert_ne!(
            original,
            drc_cache_path(dir.path(), &json!("kicad-10"), "libraries-a", &changed)
        );
        p.pcb.placement["R1"].at[0] += 1.0;
        let moved = crate::kicad::generate(&p, &libraries).unwrap();
        assert_ne!(
            drc_cache_path(dir.path(), &json!("kicad-10"), "libraries-a", &changed),
            drc_cache_path(dir.path(), &json!("kicad-10"), "libraries-a", &moved)
        );
        let board = dir.path().join("board.kicad_pcb");
        let report = dir.path().join("report.json");
        fs::write(&board, &g.pcb).unwrap();
        fs::write(&report, r#"{"violations":[],"unconnected_items":[]}"#).unwrap();
        remember_drc(&original, &board, &report, None).unwrap();
        let restored = dir.path().join("restored.kicad_pcb");
        assert!(restore_drc(&original, &restored, &report));
        assert_eq!(fs::read(&board).unwrap(), fs::read(&restored).unwrap());
        fs::write(&original, "broken cache").unwrap();
        assert!(!restore_drc(&original, &restored, &report));
    }
    #[test]
    fn copper_only_validation_retains_existing_schematic_parity_errors() {
        let before = DrcSummary {
            errors: vec![error(1.)],
            parity_errors: vec![error(1.)],
            parity_warnings: 2,
            ..Default::default()
        };
        let mut after = DrcSummary::default();
        after.inherit_parity(&before);
        assert_eq!(after.errors, before.errors);
        assert_eq!(after.warnings, 2);
        assert!(!assess(&before, &after).0);
    }
    #[test]
    fn ui_history_does_not_invalidate_checks_but_validation_settings_do() {
        let first =
            json!({"environment":{"vars":{}},"system":{"language":"en","working_dir":"one"}});
        let mut next = first.clone();
        next["system"]["working_dir"] = json!("two");
        assert_eq!(
            validation_settings(
                "kicad_common.json",
                Some(&serde_json::to_vec(&first).unwrap())
            ),
            validation_settings(
                "kicad_common.json",
                Some(&serde_json::to_vec(&next).unwrap())
            )
        );
        next["system"]["language"] = json!("pl");
        assert_ne!(
            validation_settings(
                "kicad_common.json",
                Some(&serde_json::to_vec(&first).unwrap())
            ),
            validation_settings(
                "kicad_common.json",
                Some(&serde_json::to_vec(&next).unwrap())
            )
        );
        assert_ne!(
            validation_settings(
                "pcbnew.json",
                Some(br#"{"DRC":{"report_all_track_errors":true}}"#)
            ),
            validation_settings(
                "pcbnew.json",
                Some(br#"{"DRC":{"report_all_track_errors":false}}"#)
            )
        );
        assert_eq!(
            validation_settings("pcbnew.json", None),
            validation_settings("pcbnew.json", Some(b"{}"))
        );
        assert_eq!(
            validation_settings("kicad_common.json", None),
            validation_settings("kicad_common.json", Some(b"{}"))
        );
        assert_eq!(
            validation_settings("eeschema.json", None),
            validation_settings("eeschema.json", Some(b"{}"))
        );
    }
    #[cfg(unix)]
    #[test]
    fn router_deadline_terminates_process_group_and_keeps_logs() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo started; sleep 30 & wait"]);
        let start = Instant::now();
        assert!(run_router(&mut command, dir.path(), Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(
            fs::read_to_string(dir.path().join("router.stdout.log"))
                .unwrap()
                .contains("started")
        );
    }
}
