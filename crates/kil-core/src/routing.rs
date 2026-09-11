use crate::diagnostic::Diagnostic;
use crate::model::{ResolvedProject, Route, Via};
use indexmap::IndexMap;
use kiutils_kicad::PcbFile;
use kiutils_sexpr::{Atom, CstDocument, Node};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

type NativeSegment = ([f64; 2], [f64; 2]);
type SegmentGroupKey = (String, String, i64, bool);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteCache {
    pub format_version: u32,
    pub project: String,
    pub input_fingerprint: String,
    pub engine: RouterIdentity,
    pub routes: IndexMap<String, Vec<Route>>,
    pub vias: Vec<Via>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouterIdentity {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

pub fn routing_fingerprint(project: &ResolvedProject) -> String {
    let encoded = serde_json::to_vec(&serde_json::json!({
        "compiler": env!("CARGO_PKG_VERSION"),
        "library_fingerprint": project.library_fingerprint,
        "nets": project.nets, "pcb": project.pcb, "rules": project.rules,
    }))
    .expect("routing inputs are serializable");
    let mut hasher = Sha256::new();
    hasher.update(b"kil-routing-input-v2\0");
    hasher.update(encoded);
    format!("{:x}", hasher.finalize())
}

pub fn route_cache_path(input: &Path, project: &ResolvedProject) -> Result<PathBuf, String> {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    if let Some(configured) = project
        .pcb
        .routing
        .as_ref()
        .and_then(|routing| routing.cache.as_deref())
    {
        let relative = Path::new(configured);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
        {
            return Err("routing cache must be a relative path inside the IL directory".into());
        }
        return Ok(parent.join(relative));
    }
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("project.kil");
    Ok(parent.join(format!("{stem}.routes.json")))
}

pub fn apply_route_cache(
    project: &mut ResolvedProject,
    input: &Path,
) -> Result<Option<PathBuf>, Box<Diagnostic>> {
    let Some(_) = project.pcb.routing else {
        return Ok(None);
    };
    let path = route_cache_path(input, project)
        .map_err(|message| Box::new(Diagnostic::error("ROUTE001", message, input)))?;
    if !path.is_file() {
        return Err(Box::new(
            Diagnostic::error("ROUTE002", "routing cache is missing", input)
                .with_help(format!("run 'kil route {}' first", input.display())),
        ));
    }
    let source = fs::read_to_string(&path).map_err(|err| {
        Box::new(Diagnostic::error(
            "ROUTE003",
            format!("cannot read routing cache: {err}"),
            &path,
        ))
    })?;
    let cache: RouteCache = serde_json::from_str(&source).map_err(|err| {
        Box::new(Diagnostic::error(
            "ROUTE004",
            format!("invalid routing cache JSON: {err}"),
            &path,
        ))
    })?;
    if cache.format_version != 2 || cache.project != project.project.name {
        return Err(Box::new(Diagnostic::error(
            "ROUTE005",
            "routing cache belongs to another format or project",
            &path,
        )));
    }
    if cache.input_fingerprint != routing_fingerprint(project) {
        return Err(Box::new(
            Diagnostic::error("ROUTE006", "routing cache is stale", &path)
                .with_help(format!("rerun 'kil route {}'", input.display())),
        ));
    }
    project.pcb.routes = cache.routes;
    project.pcb.vias = cache.vias;
    Ok(Some(path))
}

pub fn extract_route_cache(
    project: &ResolvedProject,
    routed_board: &Path,
    engine_version: Option<String>,
) -> Result<RouteCache, String> {
    let document = PcbFile::read(routed_board)
        .map_err(|err| format!("router output is not a readable KiCad PCB: {err}"))?;
    let ast = document.ast();
    if !ast.arcs.is_empty() {
        return Err("router emitted copper arcs, which route-cache v1 cannot represent".into());
    }
    let net_names: BTreeMap<i32, String> = ast
        .nets
        .iter()
        .filter_map(|net| Some((net.code?, normalize_net(project, net.name.as_deref()?))))
        .collect();
    let segment_labels = cst_net_labels(document.cst(), "segment");
    let via_labels = cst_net_labels(document.cst(), "via");
    let frame = BoardFrame::new(&project.pcb.outline);
    let mut segments: BTreeMap<SegmentGroupKey, Vec<NativeSegment>> = BTreeMap::new();
    for (index, segment) in ast.segments.iter().enumerate() {
        let (Some(start), Some(end), Some(layer)) =
            (segment.start, segment.end, segment.layer.as_deref())
        else {
            continue;
        };
        let net = resolve_item_net(
            project,
            segment.net,
            segment_labels.get(index).and_then(Option::as_deref),
            &net_names,
        )?;
        let width_key = segment
            .width
            .map_or(-1, |width| (width * 1_000_000.0).round() as i64);
        segments
            .entry((net, layer.to_owned(), width_key, segment.locked))
            .or_default()
            .push((frame.unmap(start), frame.unmap(end)));
    }
    let mut grouped: BTreeMap<String, Vec<Route>> = BTreeMap::new();
    for ((net, layer, width_key, locked), net_segments) in segments {
        let width = (width_key >= 0).then_some(width_key as f64 / 1_000_000.0);
        grouped.entry(net).or_default().extend(coalesce_segments(
            &layer,
            width,
            locked,
            &net_segments,
        ));
    }
    for routes in grouped.values_mut() {
        routes.sort_by_key(route_sort_key);
    }
    let mut routes = IndexMap::new();
    for net in project.nets.keys() {
        if let Some(net_routes) = grouped.remove(net) {
            routes.insert(net.clone(), net_routes);
        }
    }
    for (net, net_routes) in grouped {
        routes.insert(net, net_routes);
    }
    let mut vias = Vec::new();
    for (index, via) in ast.vias.iter().enumerate() {
        let Some(at) = via.at else {
            continue;
        };
        let net = resolve_item_net(
            project,
            via.net,
            via_labels.get(index).and_then(Option::as_deref),
            &net_names,
        )?;
        vias.push(Via {
            net,
            at: frame.unmap(at),
            size: via.size,
            drill: via.drill,
            locked: via.locked,
        });
    }
    vias.sort_by_key(via_sort_key);
    Ok(RouteCache {
        format_version: 2,
        project: project.project.name.clone(),
        input_fingerprint: routing_fingerprint(project),
        engine: RouterIdentity {
            name: "kicad-routing-tools".into(),
            version: engine_version,
        },
        routes,
        vias,
    })
}

pub fn write_route_cache(path: &Path, cache: &RouteCache) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let source = serde_json::to_string_pretty(cache).map_err(|err| err.to_string())? + "\n";
    let temporary = path.with_extension("json.kil-new");
    let backup = path.with_extension("json.kil-backup");
    fs::write(&temporary, source).map_err(|err| err.to_string())?;
    if backup.exists() {
        fs::remove_file(&backup).map_err(|err| err.to_string())?;
    }
    if path.exists() {
        fs::rename(path, &backup).map_err(|err| err.to_string())?;
    }
    if let Err(err) = fs::rename(&temporary, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(err.to_string());
    }
    if backup.exists() {
        fs::remove_file(backup).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn normalize_net(project: &ResolvedProject, native: &str) -> String {
    if let Some(stripped) = native.strip_prefix('/')
        && project.nets.contains_key(stripped)
    {
        return stripped.to_owned();
    }
    native.to_owned()
}

fn resolve_item_net(
    project: &ResolvedProject,
    code: Option<i32>,
    label: Option<&str>,
    net_names: &BTreeMap<i32, String>,
) -> Result<String, String> {
    if let Some(label) = label
        && label.parse::<i32>().is_err()
    {
        return Ok(normalize_net(project, label));
    }
    if let Some(code) = code.or_else(|| label.and_then(|value| value.parse().ok())) {
        return net_names
            .get(&code)
            .cloned()
            .ok_or_else(|| format!("router output references unknown net code {code}"));
    }
    Err("router output contains copper without a net".into())
}

fn cst_net_labels(document: &CstDocument, item_head: &str) -> Vec<Option<String>> {
    let Some(Node::List { items, .. }) = document
        .nodes
        .iter()
        .find(|node| node_head(node) == Some("kicad_pcb"))
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|node| node_head(node) == Some(item_head))
        .map(|node| child_atom(node, "net", 1).map(str::to_owned))
        .collect()
}

fn child_atom<'a>(node: &'a Node, head: &str, index: usize) -> Option<&'a str> {
    let Node::List { items, .. } = node else {
        return None;
    };
    items
        .iter()
        .find(|child| node_head(child) == Some(head))
        .and_then(|child| match child {
            Node::List { items, .. } => items.get(index).and_then(atom_text),
            _ => None,
        })
}

fn node_head(node: &Node) -> Option<&str> {
    match node {
        Node::List { items, .. } => items.first().and_then(atom_text),
        _ => None,
    }
}

fn atom_text(node: &Node) -> Option<&str> {
    match node {
        Node::Atom {
            atom: Atom::Symbol(value) | Atom::Quoted(value),
            ..
        } => Some(value),
        _ => None,
    }
}

fn route_sort_key(route: &Route) -> String {
    format!(
        "{}:{:.6}:{}",
        route.layer,
        route.width.unwrap_or_default(),
        route
            .path
            .iter()
            .map(|point| format!("{:.6},{:.6}", point[0], point[1]))
            .collect::<Vec<_>>()
            .join(";")
    )
}

fn via_sort_key(via: &Via) -> String {
    format!(
        "{}:{:.6},{:.6}:{:.6}:{:.6}",
        via.net,
        via.at[0],
        via.at[1],
        via.size.unwrap_or_default(),
        via.drill.unwrap_or_default()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PointKey(i64, i64);

fn point_key(point: [f64; 2]) -> PointKey {
    PointKey(
        (point[0] * 1_000_000.0).round() as i64,
        (point[1] * 1_000_000.0).round() as i64,
    )
}

fn coalesce_segments(
    layer: &str,
    width: Option<f64>,
    locked: bool,
    segments: &[([f64; 2], [f64; 2])],
) -> Vec<Route> {
    let mut points = BTreeMap::new();
    let mut adjacency: BTreeMap<PointKey, Vec<usize>> = BTreeMap::new();
    let edges: Vec<(PointKey, PointKey)> = segments
        .iter()
        .enumerate()
        .map(|(index, (start, end))| {
            let a = point_key(*start);
            let b = point_key(*end);
            points.entry(a).or_insert(*start);
            points.entry(b).or_insert(*end);
            adjacency.entry(a).or_default().push(index);
            adjacency.entry(b).or_default().push(index);
            (a, b)
        })
        .collect();
    let mut visited = vec![false; edges.len()];
    let mut routes = Vec::new();
    let starts: Vec<PointKey> = adjacency
        .iter()
        .filter_map(|(point, incident)| (incident.len() != 2).then_some(*point))
        .collect();
    for start in starts {
        for edge in adjacency.get(&start).into_iter().flatten().copied() {
            if !visited[edge] {
                routes.push(walk_polyline(
                    layer,
                    width,
                    locked,
                    start,
                    edge,
                    &edges,
                    &adjacency,
                    &points,
                    &mut visited,
                ));
            }
        }
    }
    for edge in 0..edges.len() {
        if !visited[edge] {
            routes.push(walk_polyline(
                layer,
                width,
                locked,
                edges[edge].0,
                edge,
                &edges,
                &adjacency,
                &points,
                &mut visited,
            ));
        }
    }
    routes
}

#[allow(clippy::too_many_arguments)]
fn walk_polyline(
    layer: &str,
    width: Option<f64>,
    locked: bool,
    start: PointKey,
    first_edge: usize,
    edges: &[(PointKey, PointKey)],
    adjacency: &BTreeMap<PointKey, Vec<usize>>,
    points: &BTreeMap<PointKey, [f64; 2]>,
    visited: &mut [bool],
) -> Route {
    let mut path = vec![points[&start]];
    let mut current = start;
    let mut edge = first_edge;
    loop {
        visited[edge] = true;
        let (a, b) = edges[edge];
        let next = if a == current { b } else { a };
        path.push(points[&next]);
        let incident = &adjacency[&next];
        if incident.len() != 2 {
            break;
        }
        let Some(next_edge) = incident
            .iter()
            .copied()
            .find(|candidate| !visited[*candidate])
        else {
            break;
        };
        current = next;
        edge = next_edge;
    }
    Route {
        layer: layer.to_owned(),
        width,
        path,
        locked,
    }
}

#[derive(Debug, Clone, Copy)]
struct BoardFrame {
    min_x: f64,
    max_y: f64,
}

impl BoardFrame {
    fn new(outline: &[[f64; 2]]) -> Self {
        Self {
            min_x: outline
                .iter()
                .map(|point| point[0])
                .fold(f64::INFINITY, f64::min),
            max_y: outline
                .iter()
                .map(|point| point[1])
                .fold(f64::NEG_INFINITY, f64::max),
        }
    }

    fn unmap(self, point: [f64; 2]) -> [f64; 2] {
        [
            round_mm(self.min_x + point[0] - 20.0),
            round_mm(self.max_y - (point[1] - 20.0)),
        ]
    }
}

fn round_mm(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// Match root net names with the same '*' and '?' selectors accepted by the CLI.
pub fn selected(patterns: &[String], net: &str) -> bool {
    fn matches(p: &[u8], s: &[u8]) -> bool {
        let mut previous = vec![false; s.len() + 1];
        previous[0] = true;
        for &c in p {
            let mut next = vec![false; s.len() + 1];
            if c == b'*' {
                next[0] = previous[0];
            }
            for j in 1..=s.len() {
                next[j] = if c == b'*' {
                    previous[j] || next[j - 1]
                } else {
                    previous[j - 1] && (c == b'?' || c == s[j - 1])
                };
            }
            previous = next;
        }
        previous[s.len()]
    }
    patterns.iter().any(|p| {
        matches(
            p.trim_start_matches('/').as_bytes(),
            net.trim_start_matches('/').as_bytes(),
        )
    })
}
/// Restore untouched nets and reject changes to locked copper before publication.
pub fn preserve_copper(
    before: &RouteCache,
    after: &mut RouteCache,
    patterns: &[String],
) -> Result<(), String> {
    for (net, routes) in &before.routes {
        if !selected(patterns, net) {
            after.routes.insert(net.clone(), routes.clone());
            continue;
        }
        for route in routes.iter().filter(|r| r.locked) {
            if !after.routes.get(net).is_some_and(|rs| {
                rs.iter()
                    .any(|r| route_sort_key(r) == route_sort_key(route))
            }) {
                return Err(format!("router changed locked copper on '{net}'"));
            }
        }
    }
    after
        .routes
        .retain(|net, _| selected(patterns, net) || before.routes.contains_key(net));
    after.vias.retain(|v| selected(patterns, &v.net));
    after.vias.extend(
        before
            .vias
            .iter()
            .filter(|v| !selected(patterns, &v.net))
            .cloned(),
    );
    for via in before
        .vias
        .iter()
        .filter(|v| v.locked && selected(patterns, &v.net))
    {
        if !after
            .vias
            .iter()
            .any(|v| via_sort_key(v) == via_sort_key(via))
        {
            return Err(format!("router changed a locked via on '{}'", via.net));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_routing_preserves_other_nets_and_rejects_locked_changes() {
        let (project, _) = crate::kicad::tests::fixture();
        let before = RouteCache {
            format_version: 2,
            project: project.project.name.clone(),
            input_fingerprint: routing_fingerprint(&project),
            engine: RouterIdentity {
                name: "test".into(),
                version: None,
            },
            routes: project.pcb.routes.clone(),
            vias: project.pcb.vias.clone(),
        };
        let mut after = before.clone();
        after.routes.shift_remove("GND");
        preserve_copper(&before, &mut after, &["SIGNAL".into()]).unwrap();
        assert_eq!(
            serde_json::to_value(&before.routes["GND"]).unwrap(),
            serde_json::to_value(&after.routes["GND"]).unwrap()
        );
        let mut before = before;
        before.routes["SIGNAL"][0].locked = true;
        after.routes["SIGNAL"].clear();
        assert!(preserve_copper(&before, &mut after, &["SIGNAL".into()]).is_err());
        assert!(selected(&["/SIG*".into()], "SIGNAL"));
        assert!(!selected(&["SIG?".into()], "SIGNAL"));
    }
    #[test]
    fn routing_fingerprint_ignores_schematic_but_tracks_libraries() {
        let (a, _) = crate::kicad::tests::fixture();
        let mut b = a.clone();
        b.schematic.placement["R1"].at = [42., 42.];
        b.components["R1"].value = "different".into();
        assert_eq!(routing_fingerprint(&a), routing_fingerprint(&b));
        b.library_fingerprint = "changed-library-content".into();
        assert_ne!(routing_fingerprint(&a), routing_fingerprint(&b));
    }
    #[test]
    fn fingerprint_changes_when_placement_changes() {
        let source = include_str!("../testdata/resolved/two-resistors.kil.json");
        let first: ResolvedProject = serde_json::from_str(source).unwrap();
        let mut second = first.clone();
        second.pcb.placement.get_mut("R1").unwrap().at[0] += 0.5;
        assert_ne!(routing_fingerprint(&first), routing_fingerprint(&second));
    }

    #[test]
    fn default_cache_stays_next_to_source() {
        let source = include_str!("../testdata/resolved/tiny-controller.kil.json");
        let project: ResolvedProject = serde_json::from_str(source).unwrap();
        let path = route_cache_path(Path::new("design/main.kil.json"), &project).unwrap();
        assert_eq!(path, Path::new("design/main.kil.routes.json"));
    }

    #[test]
    fn adjacent_router_segments_become_one_polyline() {
        let routes = coalesce_segments(
            "F.Cu",
            Some(0.25),
            false,
            &[
                ([0.0, 0.0], [1.0, 0.0]),
                ([1.0, 0.0], [2.0, 1.0]),
                ([2.0, 1.0], [3.0, 1.0]),
            ],
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path.len(), 4);
    }

    #[test]
    fn extracts_krt_string_net_references() {
        let source = include_str!("../testdata/resolved/two-resistors.kil.json");
        let project: ResolvedProject = serde_json::from_str(source).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let board = directory.path().join("router.kicad_pcb");
        fs::write(
            &board,
            r#"(kicad_pcb
  (version 20250114)
  (generator "KiCadRoutingTools")
  (general (thickness 1.6))
  (paper "A4")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (setup (pad_to_mask_clearance 0))
  (segment (start 20 34) (end 21 34) (width 0.25) (layer "F.Cu") (net "/SIGNAL") (uuid "11111111-1111-4111-8111-111111111111"))
  (via (at 21 34) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net "/GND") (uuid "22222222-2222-4222-8222-222222222222"))
)"#,
        )
        .unwrap();
        let cache = extract_route_cache(&project, &board, Some("test".into())).unwrap();
        assert_eq!(cache.routes["SIGNAL"][0].path, [[0.0, 0.0], [1.0, 0.0]]);
        assert_eq!(cache.vias[0].net, "GND");
    }

    #[test]
    fn stale_cache_is_rejected() {
        let source = include_str!("../testdata/resolved/tiny-controller.kil.json");
        let mut project: ResolvedProject = serde_json::from_str(source).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("controller.kil.json");
        let cache_path = route_cache_path(&input, &project).unwrap();
        let cache = RouteCache {
            format_version: 2,
            project: project.project.name.clone(),
            input_fingerprint: "stale".into(),
            engine: RouterIdentity {
                name: "kicad-routing-tools".into(),
                version: None,
            },
            routes: IndexMap::new(),
            vias: Vec::new(),
        };
        write_route_cache(&cache_path, &cache).unwrap();
        let diagnostic = apply_route_cache(&mut project, &input).unwrap_err();
        assert_eq!(diagnostic.code, "ROUTE006");
    }
}
