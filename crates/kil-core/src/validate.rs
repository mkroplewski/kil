use crate::diagnostic::Diagnostic;
use crate::library::ResolvedLibraries;
use crate::model::{KilProject, Point};
use crate::source_map::SourceMap;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub fn validate_basic(project: &KilProject, file: &Path, source: &str) -> Vec<Diagnostic> {
    let map = SourceMap::new(source);
    let mut out = Vec::new();
    let mut push = |mut d: Diagnostic, path: String| {
        d.path = Some(path.clone());
        d.span = map.span_for_path(&path);
        out.push(d);
    };

    if project.format_version != 1 {
        push(
            Diagnostic::error(
                "KIL001",
                format!("unsupported format_version {}", project.format_version),
                file,
            )
            .with_help("use format_version: 1"),
            "/format_version".into(),
        );
    }
    if project.project.kicad != 10 {
        push(
            Diagnostic::error("KIL002", "v1 targets KiCad 10 only", file),
            "/project/kicad".into(),
        );
    }
    if !valid_project_name(&project.project.name) {
        push(
            Diagnostic::error("KIL003", "project name must be a safe file stem", file)
                .with_help("use letters, digits, '.', '_' or '-'"),
            "/project/name".into(),
        );
    }
    if project.pcb.outline.len() < 3 {
        push(
            Diagnostic::error("PCB001", "board outline needs at least three points", file),
            "/pcb/outline".into(),
        );
    } else if polygon_area(&project.pcb.outline).abs() < 0.000_001 {
        push(
            Diagnostic::error("PCB002", "board outline has zero area", file),
            "/pcb/outline".into(),
        );
    }

    let mut endpoint_to_net: BTreeMap<&str, &str> = BTreeMap::new();
    for (net, endpoints) in &project.nets {
        if endpoints.len() < 2 {
            push(
                Diagnostic::warning(
                    "NET001",
                    format!("net '{net}' has fewer than two endpoints"),
                    file,
                ),
                format!("/nets/{net}"),
            );
        }
        let mut seen = BTreeSet::new();
        for endpoint in endpoints {
            let path = format!("/nets/{net}/{endpoint}");
            let Some((reference, pin)) = parse_endpoint(endpoint) else {
                push(
                    Diagnostic::error("NET002", format!("invalid endpoint '{endpoint}'"), file)
                        .with_help("use REF.PIN, for example U1.3 or U1.VCC"),
                    path.clone(),
                );
                continue;
            };
            if !project.components.contains_key(reference) {
                push(
                    Diagnostic::error("NET003", format!("unknown component '{reference}'"), file),
                    path.clone(),
                );
            }
            if pin.is_empty() {
                push(
                    Diagnostic::error("NET004", "pin name cannot be empty", file),
                    path.clone(),
                );
            }
            if !seen.insert(endpoint.as_str()) {
                push(
                    Diagnostic::error(
                        "NET005",
                        format!("duplicate endpoint '{endpoint}' in net '{net}'"),
                        file,
                    ),
                    path.clone(),
                );
            }
            if let Some(previous) = endpoint_to_net.insert(endpoint, net)
                && previous != net
            {
                push(
                    Diagnostic::error(
                        "NET006",
                        format!(
                            "endpoint '{endpoint}' is assigned to both '{previous}' and '{net}'"
                        ),
                        file,
                    ),
                    path.clone(),
                );
            }
        }
    }

    for reference in project.components.keys() {
        if !project.schematic.placement.contains_key(reference) {
            push(
                Diagnostic::error(
                    "SCH001",
                    format!("missing schematic placement for '{reference}'"),
                    file,
                ),
                format!("/schematic/placement/{reference}"),
            );
        }
        if !project.pcb.placement.contains_key(reference) {
            push(
                Diagnostic::error(
                    "PCB003",
                    format!("missing PCB placement for '{reference}'"),
                    file,
                ),
                format!("/pcb/placement/{reference}"),
            );
        }
    }
    for reference in project.schematic.placement.keys() {
        if !project.components.contains_key(reference) {
            push(
                Diagnostic::error(
                    "SCH002",
                    format!("placement references unknown component '{reference}'"),
                    file,
                ),
                format!("/schematic/placement/{reference}"),
            );
        }
    }
    for reference in project.pcb.placement.keys() {
        if !project.components.contains_key(reference) {
            push(
                Diagnostic::error(
                    "PCB004",
                    format!("placement references unknown component '{reference}'"),
                    file,
                ),
                format!("/pcb/placement/{reference}"),
            );
        }
    }
    for (reference, placement) in &project.pcb.placement {
        if !point_in_polygon(placement.at, &project.pcb.outline) {
            push(
                Diagnostic::error(
                    "PCB013",
                    format!("component '{reference}' origin is outside the board"),
                    file,
                ),
                format!("/pcb/placement/{reference}/at"),
            );
        }
    }

    for (net, routes) in &project.pcb.routes {
        if !project.nets.contains_key(net) {
            push(
                Diagnostic::error("PCB005", format!("route uses undefined net '{net}'"), file),
                format!("/pcb/routes/{net}"),
            );
        }
        for (idx, route) in routes.iter().enumerate() {
            let path = format!("/pcb/routes/{net}/{idx}");
            if route.path.len() < 2 {
                push(
                    Diagnostic::error("PCB006", "route needs at least two points", file),
                    path.clone(),
                );
            }
            if route.width.is_some_and(|width| width <= 0.0) {
                push(
                    Diagnostic::error("PCB014", "route width must be positive", file),
                    format!("{path}/width"),
                );
            }
            if route.layer != "F.Cu" && route.layer != "B.Cu" {
                push(
                    Diagnostic::error(
                        "PCB007",
                        format!("unsupported copper layer '{}'", route.layer),
                        file,
                    ),
                    format!("{path}/layer"),
                );
            }
            for point in &route.path {
                if !point_in_polygon(*point, &project.pcb.outline) {
                    push(
                        Diagnostic::error(
                            "PCB008",
                            format!("route point {:?} is outside the board", point),
                            file,
                        ),
                        format!("{path}/path"),
                    );
                }
            }
        }
    }
    for (idx, via) in project.pcb.vias.iter().enumerate() {
        let path = format!("/pcb/vias/{idx}");
        if !project.nets.contains_key(&via.net) {
            push(
                Diagnostic::error(
                    "PCB009",
                    format!("via uses undefined net '{}'", via.net),
                    file,
                ),
                path.clone(),
            );
        }
        if !point_in_polygon(via.at, &project.pcb.outline) {
            push(
                Diagnostic::error("PCB010", "via is outside the board", file),
                format!("{path}/at"),
            );
        }
    }
    for (idx, zone) in project.pcb.zones.iter().enumerate() {
        let path = format!("/pcb/zones/{idx}");
        if !project.nets.contains_key(&zone.net) {
            push(
                Diagnostic::error(
                    "PCB011",
                    format!("zone uses undefined net '{}'", zone.net),
                    file,
                ),
                path.clone(),
            );
        }
        if zone.outline.len() < 3 {
            push(
                Diagnostic::error("PCB012", "zone needs at least three points", file),
                format!("{path}/outline"),
            );
        }
        if zone.layer != "F.Cu" && zone.layer != "B.Cu" {
            push(
                Diagnostic::error(
                    "PCB015",
                    format!("unsupported zone layer '{}'", zone.layer),
                    file,
                ),
                format!("{path}/layer"),
            );
        }
        for point in &zone.outline {
            if !point_in_polygon(*point, &project.pcb.outline) {
                push(
                    Diagnostic::error("PCB016", "zone point is outside the board", file),
                    format!("{path}/outline"),
                );
            }
        }
    }
    for (idx, hole) in project.pcb.holes.iter().enumerate() {
        let path = format!("/pcb/holes/{idx}");
        if hole.diameter <= 0.0 {
            push(
                Diagnostic::error("PCB017", "hole diameter must be positive", file),
                format!("{path}/diameter"),
            );
        }
        if !point_in_polygon(hole.at, &project.pcb.outline) {
            push(
                Diagnostic::error("PCB018", "hole is outside the board", file),
                format!("{path}/at"),
            );
        }
    }
    for (idx, wire) in project.schematic.wires.iter().enumerate() {
        if !project.nets.contains_key(&wire.net) {
            push(
                Diagnostic::error(
                    "SCH003",
                    format!("wire uses undefined net '{}'", wire.net),
                    file,
                ),
                format!("/schematic/wires/{idx}/net"),
            );
        }
        if wire.path.len() < 2 {
            push(
                Diagnostic::error("SCH004", "schematic wire needs at least two points", file),
                format!("/schematic/wires/{idx}/path"),
            );
        }
    }
    if project.rules.clearance <= 0.0
        || project.rules.track_width <= 0.0
        || project.rules.via_size <= 0.0
        || project.rules.via_drill <= 0.0
        || project.rules.via_drill >= project.rules.via_size
    {
        push(
            Diagnostic::error(
                "RULE001",
                "rules must be positive and via_drill must be smaller than via_size",
                file,
            ),
            "/rules".into(),
        );
    }
    out
}

pub fn validate_libraries(
    project: &KilProject,
    libraries: &ResolvedLibraries,
    file: &Path,
    source: &str,
) -> Vec<Diagnostic> {
    let map = SourceMap::new(source);
    let mut out = Vec::new();
    for (net, endpoints) in &project.nets {
        for endpoint in endpoints {
            let Some((reference, pin_query)) = parse_endpoint(endpoint) else {
                continue;
            };
            let Some(resolved) = libraries.components.get(reference) else {
                continue;
            };
            let matches = resolved
                .pins
                .iter()
                .filter(|pin| pin.number == pin_query || pin.name.as_deref() == Some(pin_query))
                .collect::<Vec<_>>();
            let path = format!("/nets/{net}/{endpoint}");
            let mut diag = match matches.len() {
                0 => Some(
                    Diagnostic::error(
                        "LIB004",
                        format!("symbol pin '{endpoint}' does not exist"),
                        file,
                    )
                    .with_help(
                        "use a pin number or a unique pin name from the referenced KiCad symbol",
                    ),
                ),
                1 => None,
                _ if matches
                    .iter()
                    .map(|p| &p.number)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == 1 =>
                {
                    None
                }
                _ => Some(Diagnostic::error(
                    "LIB005",
                    format!("symbol pin name '{endpoint}' is ambiguous"),
                    file,
                )),
            };
            if let Some(d) = diag.as_mut() {
                d.path = Some(path.clone());
                d.span = map.span_for_path(&path);
            }
            if let Some(d) = diag {
                out.push(d);
            }
        }
    }
    for endpoint in &project.schematic.no_connect {
        let Some((reference, pin_query)) = parse_endpoint(endpoint) else {
            let path = format!("/schematic/no_connect/{endpoint}");
            out.push(
                Diagnostic::error(
                    "SCH005",
                    format!("invalid no-connect endpoint '{endpoint}'"),
                    file,
                )
                .at_path(path.clone())
                .with_span(map.span_for_path(&path)),
            );
            continue;
        };
        if let Some(resolved) = libraries.components.get(reference)
            && !resolved
                .pins
                .iter()
                .any(|p| p.number == pin_query || p.name.as_deref() == Some(pin_query))
        {
            let path = format!("/schematic/no_connect/{endpoint}");
            out.push(
                Diagnostic::error(
                    "SCH006",
                    format!("unknown no-connect pin '{endpoint}'"),
                    file,
                )
                .at_path(path.clone())
                .with_span(map.span_for_path(&path)),
            );
        }
    }
    out
}

pub fn parse_endpoint(endpoint: &str) -> Option<(&str, &str)> {
    let (reference, pin) = endpoint.split_once('.')?;
    (!reference.is_empty() && !pin.is_empty()).then_some((reference, pin))
}

fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn polygon_area(points: &[Point]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        / 2.0
}

pub fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let pi = polygon[i];
        let pj = polygon[j];
        let on_edge = cross(pj, pi, point).abs() < 1e-7
            && point[0] >= pj[0].min(pi[0]) - 1e-7
            && point[0] <= pj[0].max(pi[0]) + 1e-7
            && point[1] >= pj[1].min(pi[1]) - 1e-7
            && point[1] <= pj[1].max(pi[1]) + 1e-7;
        if on_edge {
            return true;
        }
        if ((pi[1] > point[1]) != (pj[1] > point[1]))
            && point[0] < (pj[0] - pi[0]) * (point[1] - pi[1]) / (pj[1] - pi[1]) + pi[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn cross(a: Point, b: Point, p: Point) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validator_collects_independent_errors() {
        let source = r#"{
          "format_version": 1,
          "project": { "name": "bad", "kicad": 10 },
          "components": { "R1": { "symbol": "Device:R", "value": "1k", "footprint": "Resistor_SMD:R_0603_1608Metric" } },
          "nets": { "N": ["MISSING.1"] },
          "schematic": { "placement": {} },
          "pcb": { "outline": [[0,0], [1,0]], "placement": {}, "routes": { "OTHER": [{ "path": [[0,0]] }] } }
        }"#;
        let project: KilProject = serde_json::from_str(source).unwrap();
        let diagnostics = validate_basic(&project, Path::new("bad.kil.json"), source);
        let codes = diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("PCB001"));
        assert!(codes.contains("NET003"));
        assert!(codes.contains("SCH001"));
        assert!(codes.contains("PCB005"));
        assert!(
            diagnostics
                .iter()
                .filter(|d| d.severity == crate::diagnostic::Severity::Error)
                .all(|d| d.span.is_some())
        );
    }

    #[test]
    fn polygon_boundary_counts_as_inside() {
        let polygon = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(point_in_polygon([0.0, 5.0], &polygon));
        assert!(point_in_polygon([5.0, 5.0], &polygon));
        assert!(!point_in_polygon([11.0, 5.0], &polygon));
    }
}
