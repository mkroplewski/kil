//! Elaborate immutable source documents into globally addressed compiler objects.
use crate::{diagnostic::Diagnostic, model::*, source::*};
use indexmap::IndexMap;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct Origin {
    pub file: PathBuf,
    pub path: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct BlockInfo {
    pub id: String,
    pub file: PathBuf,
    pub components: Vec<String>,
    pub nets: Vec<String>,
    pub schematic: Transform,
    pub pcb: Transform,
}
pub struct Resolution {
    pub layouts: Vec<crate::layout::LayoutPlan>,
    pub project: ResolvedProject,
    pub blocks: IndexMap<String, BlockInfo>,
    pub origins: IndexMap<String, Origin>,
    pub diagnostics: Vec<Diagnostic>,
}
pub fn resolve(source: &Project, file: &Path) -> Resolution {
    let project = ResolvedProject {
        schema: source.schema.clone(),
        library_fingerprint: String::new(),
        format_version: source.format_version,
        project: source.project.clone(),
        units: Units::Mm,
        components: IndexMap::new(),
        nets: IndexMap::new(),
        schematic: Schematic::default(),
        pcb: Pcb {
            outline: source.pcb.outline.clone(),
            placement: IndexMap::new(),
            routes: IndexMap::new(),
            vias: vec![],
            zones: vec![],
            holes: vec![],
            silk: vec![],
            routing: source.build.routing.clone(),
        },
        rules: source.rules.clone(),
    };
    let mut result = Resolution {
        layouts: vec![crate::layout::LayoutPlan {
            design: source.pcb.clone(),
            prefix: String::new(),
            nets: IndexMap::new(),
            transform: Transform::default(),
            file: file.into(),
        }],
        project,
        blocks: IndexMap::new(),
        origins: IndexMap::new(),
        diagnostics: vec![],
    };
    let root = match fs::canonicalize(
        file.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    ) {
        Ok(root) => root,
        Err(e) => {
            result.diagnostics.push(Diagnostic::error(
                "MOD002",
                format!("cannot resolve project directory: {e}"),
                file,
            ));
            return result;
        }
    };
    let mut prefixes = IndexMap::new();
    expand(
        &mut result,
        &source.circuit,
        &source.schematic,
        file,
        &root,
        "",
        &IndexMap::new(),
        Some(Transform::default()),
        Some(Transform::default()),
        &mut vec![],
        &mut prefixes,
    );
    let mut used = BTreeSet::new();
    for part in result.project.components.values() {
        if !part.reference.is_empty() && !used.insert(part.reference.clone()) {
            result.diagnostics.push(Diagnostic::error(
                "ID002",
                format!("duplicate printed reference '{}'", part.reference),
                file,
            ));
        }
    }
    let mut ids: Vec<_> = result.project.components.keys().cloned().collect();
    ids.sort();
    for id in ids {
        let part = result.project.components.get_mut(&id).unwrap();
        if part.reference.is_empty() {
            let prefix = &prefixes[&id];
            let mut n = 1;
            while used.contains(&format!("{prefix}{n}")) {
                n += 1;
            }
            part.reference = format!("{prefix}{n}");
            used.insert(part.reference.clone());
        }
    }
    let canonical = |endpoint: &str| {
        if let Some((id, terminal)) = endpoint.rsplit_once('.')
            && let Some(part) = result.project.components.get(id)
        {
            return format!(
                "{id}.{}",
                part.aliases
                    .get(terminal)
                    .map(String::as_str)
                    .unwrap_or(terminal)
            );
        }
        endpoint.to_owned()
    };
    let nets = result
        .project
        .nets
        .iter()
        .map(|(n, e)| (n.clone(), e.iter().map(|s| canonical(s)).collect()))
        .collect();
    let unconnected = result
        .project
        .schematic
        .no_connect
        .iter()
        .map(|s| canonical(s))
        .collect();
    result.project.nets = nets;
    result.project.schematic.no_connect = unconnected;
    let preliminary = crate::layout::resolve_layout(
        &mut result.project,
        &result.layouts,
        &crate::library::ResolvedLibraries::default(),
        false,
    );
    result.diagnostics.extend(preliminary);
    result
}
fn qualify(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.into()
    } else {
        format!("{prefix}/{local}")
    }
}
#[allow(clippy::too_many_arguments)]
fn expand(
    r: &mut Resolution,
    circuit: &Circuit,
    view: &SchematicView,
    file: &Path,
    root: &Path,
    prefix: &str,
    net_bindings: &IndexMap<String, String>,
    sch: Option<Transform>,
    pcb: Option<Transform>,
    stack: &mut Vec<PathBuf>,
    prefixes: &mut IndexMap<String, String>,
) {
    let net = |n: &str| {
        net_bindings
            .get(n)
            .cloned()
            .unwrap_or_else(|| qualify(prefix, n))
    };
    for (id, part) in &circuit.parts {
        if !valid_block_id(id)
            || !part
                .reference_prefix
                .chars()
                .all(|c| c.is_ascii_alphabetic())
            || part.reference_prefix.is_empty()
        {
            r.diagnostics.push(Diagnostic::error(
                "ID001",
                format!("invalid part id or reference prefix '{id}'"),
                file,
            ));
            continue;
        }
        if let Some(reference) = &part.reference {
            let number = reference
                .strip_prefix(&part.reference_prefix)
                .and_then(|n| n.parse::<u32>().ok());
            if number.is_none_or(|n| n == 0) {
                r.diagnostics.push(Diagnostic::error("ID004",format!("printed reference '{reference}' must use its prefix followed by a positive integer"),file));
            }
        }
        for (alias, terminal) in &part.terminals {
            if alias.is_empty()
                || alias.chars().all(|c| c.is_ascii_digit())
                || alias.contains(['.', '/'])
                || terminal.is_empty()
            {
                r.diagnostics.push(Diagnostic::error(
                    "ID005",
                    format!("invalid terminal alias '{alias}'"),
                    file,
                ));
            }
        }
        let key = qualify(prefix, id);
        prefixes.insert(key.clone(), part.reference_prefix.clone());
        r.origins.insert(
            key.clone(),
            Origin {
                file: file.into(),
                path: format!("/circuit/parts/{id}"),
            },
        );
        let resolved = Component {
            reference: part.reference.clone().unwrap_or_default(),
            aliases: part.terminals.clone(),
            symbol: part.symbol.clone(),
            footprint: part.footprint.clone(),
            value: part.value.clone(),
            fields: part.fields.clone(),
        };
        if r.project.components.insert(key.clone(), resolved).is_some() {
            r.diagnostics.push(Diagnostic::error(
                "ID003",
                format!("duplicate identity '{key}'"),
                file,
            ));
        }
    }
    for (name, endpoints) in &circuit.nets {
        if name.is_empty() || name.contains('/') {
            r.diagnostics.push(Diagnostic::error(
                "NET008",
                "local net names must be nonempty and cannot contain '/'",
                file,
            ));
        }
        for endpoint in endpoints {
            if endpoint.split_once('.').is_none_or(|(part, terminal)| {
                terminal.is_empty() || !circuit.parts.contains_key(part)
            }) {
                r.diagnostics.push(Diagnostic::error("NET009",format!("'{endpoint}' is not a local part terminal; connect instances through their ports"),file));
            }
        }
        r.project
            .nets
            .entry(net(name))
            .or_default()
            .extend(endpoints.iter().map(|e| qualify(prefix, e)));
    }
    r.project
        .schematic
        .no_connect
        .extend(circuit.unconnected.iter().map(|e| qualify(prefix, e)));
    if let Some(transform) = sch {
        let mut schematic = Schematic {
            placement: IndexMap::new(),
            wires: view.wires.clone(),
            labels: view.labels.clone(),
            no_connect: vec![],
        };
        for (id, symbol) in &view.symbols {
            let mut symbol = symbol.clone();
            symbol.part = qualify(prefix, &symbol.part);
            schematic.placement.insert(qualify(prefix, id), symbol);
        }
        merge_schematic(
            &mut r.project.schematic,
            schematic,
            transform,
            &net,
            file,
            &mut r.diagnostics,
        );
    }
    for (id, instance) in &circuit.instances {
        if !valid_block_id(id) || circuit.parts.contains_key(id) {
            r.diagnostics.push(Diagnostic::error(
                "MOD001",
                format!("invalid or conflicting instance id '{id}'"),
                file,
            ));
            continue;
        }
        let path = file
            .parent()
            .unwrap_or(Path::new("."))
            .join(&instance.source);
        let canonical = match fs::canonicalize(&path) {
            Ok(p) if p.starts_with(root) => p,
            _ => {
                r.diagnostics.push(Diagnostic::error(
                    "MOD002",
                    format!("module is missing or outside project: {}", path.display()),
                    file,
                ));
                continue;
            }
        };
        if stack.contains(&canonical) {
            r.diagnostics
                .push(Diagnostic::error("MOD004", "module cycle", &canonical));
            continue;
        }
        let module = (|| -> Result<Module, String> {
            let mut value: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&canonical).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let declarations: IndexMap<String, Parameter> = serde_json::from_value(
                value
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            )
            .map_err(|e| e.to_string())?;
            let mut values = IndexMap::new();
            for key in instance.parameters.keys() {
                if !declarations.contains_key(key) {
                    return Err(format!("unknown parameter '{key}'"));
                }
            }
            for (key, p) in &declarations {
                let v = instance
                    .parameters
                    .get(key)
                    .or(p.default.as_ref())
                    .ok_or_else(|| format!("missing parameter '{key}'"))?;
                if !match p.kind {
                    ParameterType::String => v.is_string(),
                    ParameterType::Number => v.is_number(),
                    ParameterType::Boolean => v.is_boolean(),
                } {
                    return Err(format!("wrong type for parameter '{key}'"));
                }
                values.insert(key.clone(), v.clone());
            }
            substitute(&mut value, &values)?;
            serde_json::from_value(value).map_err(|e| e.to_string())
        })();
        let m = match module {
            Ok(m) => m,
            Err(e) => {
                r.diagnostics
                    .push(Diagnostic::error("MOD003", e, &canonical));
                continue;
            }
        };
        if m.format_version != 2 {
            r.diagnostics.push(Diagnostic::error(
                "MOD007",
                "expected format_version 2",
                &canonical,
            ));
            continue;
        }
        if !m.pcb.outline.is_empty() {
            r.diagnostics.push(Diagnostic::error(
                "MOD015",
                "module layouts cannot redefine the board outline",
                &canonical,
            ));
            continue;
        }
        let key = qualify(prefix, id);
        let mut bindings = IndexMap::new();
        for port in instance.connections.keys() {
            if !m.ports.contains_key(port) {
                r.diagnostics.push(Diagnostic::error(
                    "MOD011",
                    format!("unknown port '{port}'"),
                    file,
                ));
            }
        }
        for (port, local) in &m.ports {
            if !m.circuit.nets.contains_key(local) {
                r.diagnostics.push(Diagnostic::error(
                    "MOD012",
                    format!("port '{port}' exposes unknown net '{local}'"),
                    &canonical,
                ));
            }
            match instance.connections.get(port) {
                Some(target) => {
                    if let Some(previous) = bindings.insert(local.clone(), net(target))
                        && previous != net(target)
                    {
                        r.diagnostics.push(Diagnostic::error(
                            "MOD013",
                            "ports on the same net have conflicting bindings",
                            file,
                        ));
                    }
                }
                None => r.diagnostics.push(Diagnostic::error(
                    "MOD014",
                    format!("missing connection for port '{port}'"),
                    file,
                )),
            }
        }
        let st = sch
            .zip(instance.schematic)
            .map(|(a, b)| compose_schematic(a, b));
        let pt = pcb.zip(instance.pcb).map(|(a, b)| compose(a, b));
        if let Some(t) = pt {
            r.layouts.push(crate::layout::LayoutPlan {
                design: m.pcb.clone(),
                prefix: key.clone(),
                nets: bindings.clone(),
                transform: t,
                file: canonical.clone(),
            });
        }
        stack.push(canonical.clone());
        expand(
            r,
            &m.circuit,
            &m.schematic,
            &canonical,
            root,
            &key,
            &bindings,
            st,
            pt,
            stack,
            prefixes,
        );
        stack.pop();
        r.blocks.insert(
            key.clone(),
            BlockInfo {
                id: key.clone(),
                file: canonical,
                components: r
                    .project
                    .components
                    .keys()
                    .filter(|p| p.starts_with(&format!("{key}/")))
                    .cloned()
                    .collect(),
                nets: r
                    .project
                    .nets
                    .keys()
                    .filter(|n| {
                        n.starts_with(&format!("{key}/")) || bindings.values().any(|v| v == *n)
                    })
                    .cloned()
                    .collect(),
                schematic: st.unwrap_or_default(),
                pcb: pt.unwrap_or_default(),
            },
        );
    }
}
fn substitute(
    value: &mut serde_json::Value,
    parameters: &IndexMap<String, serde_json::Value>,
) -> Result<(), String> {
    match value {
        serde_json::Value::String(s) => {
            if let Some(key) = s.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
                *value = parameters
                    .get(key)
                    .ok_or_else(|| format!("unknown parameter '{key}'"))?
                    .clone();
            }
        }
        serde_json::Value::Array(a) => {
            for v in a {
                substitute(v, parameters)?;
            }
        }
        serde_json::Value::Object(o) => {
            for (k, v) in o {
                if k != "parameters" {
                    substitute(v, parameters)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
fn merge_schematic(
    target: &mut Schematic,
    mut source: Schematic,
    transform: Transform,
    map_net: &dyn Fn(&str) -> String,
    file: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (reference, mut placement) in source.placement.drain(..) {
        placement.at = apply_schematic(transform, placement.at);
        placement.rotation = normalize_angle(placement.rotation + transform.rotation);
        if target
            .placement
            .insert(reference.clone(), placement)
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                "MOD009",
                format!("duplicate schematic placement for '{reference}'"),
                file,
            ));
        }
    }
    for mut wire in source.wires {
        wire.net = map_net(&wire.net);
        wire.path = wire
            .path
            .into_iter()
            .map(|point| apply_schematic(transform, point))
            .collect();
        target.wires.push(wire);
    }
    for mut label in source.labels {
        label.net = map_net(&label.net);
        label.at = apply_schematic(transform, label.at);
        label.rotation = normalize_angle(label.rotation + transform.rotation);
        target.labels.push(label);
    }
    target.no_connect.extend(source.no_connect);
}

fn apply_schematic(t: Transform, p: [f64; 2]) -> [f64; 2] {
    apply(
        Transform {
            at: t.at,
            rotation: -t.rotation,
        },
        p,
    )
}
fn compose_schematic(parent: Transform, child: Transform) -> Transform {
    Transform {
        at: apply_schematic(parent, child.at),
        rotation: normalize_angle(parent.rotation + child.rotation),
    }
}

fn compose(parent: Transform, child: Transform) -> Transform {
    Transform {
        at: apply(parent, child.at),
        rotation: normalize_angle(parent.rotation + child.rotation),
    }
}

fn apply(transform: Transform, point: [f64; 2]) -> [f64; 2] {
    let radians = transform.rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    [
        round_mm(transform.at[0] + point[0] * cos - point[1] * sin),
        round_mm(transform.at[1] + point[0] * sin + point[1] * cos),
    ]
}

fn normalize_angle(angle: f64) -> f64 {
    let normalized = angle.rem_euclid(360.0);
    if normalized == -0.0 { 0.0 } else { normalized }
}

fn round_mm(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn valid_block_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_composition_uses_local_coordinates() {
        let parent = Transform {
            at: [10.0, 20.0],
            rotation: 90.0,
        };
        let child = Transform {
            at: [2.0, 0.0],
            rotation: 90.0,
        };
        let combined = compose(parent, child);
        assert_eq!(combined.at, [10.0, 22.0]);
        assert_eq!(combined.rotation, 180.0);
        assert_eq!(apply(combined, [1.0, 0.0]), [9.0, 22.0]);
    }
}

#[cfg(test)]
mod instance_tests {
    use super::*;
    fn example() -> (Project, PathBuf) {
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/modular-resistors/project.kil.json");
        (
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap(),
            file,
        )
    }
    #[test]
    fn empty_terminal_is_rejected() {
        let (mut source, file) = example();
        source.circuit.instances.clear();
        source.circuit.parts.insert("R1".into(), serde_json::from_value(serde_json::json!({
            "symbol":"Device:R", "footprint":"Resistor_SMD:R_0603_1608Metric", "reference_prefix":"R"
        })).unwrap());
        source
            .circuit
            .nets
            .insert("SIGNAL".into(), vec!["R1.".into()]);
        assert!(
            resolve(&source, &file)
                .diagnostics
                .iter()
                .any(|d| d.code == "NET009")
        );
    }

    #[test]
    fn missing_project_directory_is_an_error() {
        let (source, _) = example();
        let dir = tempfile::tempdir().unwrap();
        let result = resolve(&source, &dir.path().join("missing/project.kil.json"));
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "MOD002");
    }

    #[test]
    fn repeated_instances_are_private_and_source_is_immutable() {
        let (mut source, file) = example();
        let copy = source.circuit.instances["divider"].clone();
        source.circuit.instances.insert("other".into(), copy);
        let before = serde_json::to_value(&source).unwrap();
        let r = resolve(&source, &file);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.project.components.len(), 4);
        assert_ne!(
            r.project.components["divider/R1"].reference,
            r.project.components["other/R1"].reference
        );
        assert_eq!(r.project.nets["SIGNAL"].len(), 4);
        assert!(r.origins["other/R1"].file.ends_with("divider.kil.json"));
        assert_eq!(serde_json::to_value(&source).unwrap(), before);
    }
    #[test]
    fn missing_and_unknown_ports_are_errors() {
        let (mut source, file) = example();
        source.circuit.instances["divider"]
            .connections
            .shift_remove("GND");
        source.circuit.instances["divider"]
            .connections
            .insert("secret".into(), "X".into());
        let r = resolve(&source, &file);
        assert!(r.diagnostics.iter().any(|d| d.code == "MOD011"));
        assert!(r.diagnostics.iter().any(|d| d.code == "MOD014"));
    }
    #[test]
    fn aliases_canonicalize_before_conflict_detection() {
        let file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/two-resistors.kil.json");
        let mut source: Project =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        source.circuit.parts["R1"]
            .terminals
            .insert("positive".into(), "1".into());
        source
            .circuit
            .nets
            .insert("OTHER".into(), vec!["R1.positive".into()]);
        let r = resolve(&source, &file);
        assert_eq!(r.project.nets["OTHER"], ["R1.1"]);
        assert!(
            crate::validate::validate_basic(&r.project, &file, "")
                .iter()
                .any(|d| d.code == "NET006")
        );
    }
    #[test]
    fn numeric_parameters_are_substituted_before_typed_layout_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let (mut source, _) = example();
        let mut module: serde_json::Value = serde_json::from_str(include_str!(
            "../../../examples/modular-resistors/blocks/divider.kil.json"
        ))
        .unwrap();
        module["parameters"] = serde_json::json!({"foo.bar":{"type":"number","default":2.0}});
        module["pcb"]["placement"]["R1"]["at"][0] = serde_json::json!("${foo.bar}");
        fs::write(dir.path().join("module.json"), module.to_string()).unwrap();
        source.circuit.instances["divider"].source = "module.json".into();
        let r = resolve(&source, &dir.path().join("project.json"));
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.project.pcb.placement["divider/R1"].at[0], 7.0);
    }
}
