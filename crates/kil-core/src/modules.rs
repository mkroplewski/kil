use crate::diagnostic::Diagnostic;
use crate::model::{KilProject, ModuleFile, ModuleImport, PcbFragment, Schematic, Transform};
use crate::source_map::SourceMap;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BlockInfo {
    pub id: String,
    pub file: PathBuf,
    pub components: Vec<String>,
    pub nets: Vec<String>,
    pub schematic: Transform,
    pub pcb: Transform,
}

#[derive(Debug, Default)]
pub struct ModuleResolution {
    pub blocks: IndexMap<String, BlockInfo>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn resolve_modules(project: &mut KilProject, root_file: &Path) -> ModuleResolution {
    let mut resolution = ModuleResolution::default();
    if project.imports.is_empty() {
        return resolution;
    }
    let root_dir = root_file.parent().unwrap_or_else(|| Path::new("."));
    let root_canonical = fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());
    let imports = std::mem::take(&mut project.imports);
    let mut stack = Vec::new();
    for import in imports {
        merge_import(
            project,
            &import,
            root_file,
            &root_canonical,
            "",
            Transform::default(),
            Transform::default(),
            &|net| net.to_owned(),
            &mut stack,
            &mut resolution,
        );
    }
    resolution
}

#[allow(clippy::too_many_arguments)]
fn merge_import(
    project: &mut KilProject,
    import: &ModuleImport,
    importing_file: &Path,
    root_dir: &Path,
    parent_id: &str,
    parent_schematic: Transform,
    parent_pcb: Transform,
    parent_net: &dyn Fn(&str) -> String,
    stack: &mut Vec<PathBuf>,
    resolution: &mut ModuleResolution,
) {
    let block_id = if parent_id.is_empty() {
        import.id.clone()
    } else {
        format!("{parent_id}/{}", import.id)
    };
    if !valid_block_id(&import.id) {
        resolution.diagnostics.push(
            Diagnostic::error(
                "MOD001",
                format!("invalid module import id '{}'", import.id),
                importing_file,
            )
            .with_help("use letters, digits, '_', '-' or '.'"),
        );
        return;
    }
    if Path::new(&import.path).is_absolute()
        || Path::new(&import.path)
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
    {
        resolution.diagnostics.push(
            Diagnostic::error(
                "MOD002",
                format!("module path '{}' must stay inside the project", import.path),
                importing_file,
            )
            .with_help("use a relative path without '..'"),
        );
        return;
    }
    let module_path = importing_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&import.path);
    let canonical = match fs::canonicalize(&module_path) {
        Ok(path) => path,
        Err(err) => {
            resolution.diagnostics.push(Diagnostic::error(
                "MOD003",
                format!("cannot read module '{}': {err}", module_path.display()),
                importing_file,
            ));
            return;
        }
    };
    if !canonical.starts_with(root_dir) {
        resolution.diagnostics.push(Diagnostic::error(
            "MOD002",
            format!(
                "module '{}' resolves outside the project",
                module_path.display()
            ),
            importing_file,
        ));
        return;
    }
    if let Some(position) = stack.iter().position(|path| path == &canonical) {
        let mut chain: Vec<String> = stack[position..]
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        chain.push(canonical.display().to_string());
        resolution.diagnostics.push(
            Diagnostic::error(
                "MOD004",
                format!("module import cycle: {}", chain.join(" -> ")),
                &canonical,
            )
            .with_help("remove one import from the cycle"),
        );
        return;
    }
    if resolution.blocks.contains_key(&block_id) {
        resolution.diagnostics.push(Diagnostic::error(
            "MOD005",
            format!("duplicate block id '{block_id}'"),
            importing_file,
        ));
        return;
    }
    let source = match fs::read_to_string(&canonical) {
        Ok(source) => source,
        Err(err) => {
            resolution.diagnostics.push(Diagnostic::error(
                "MOD003",
                format!("cannot read module: {err}"),
                &canonical,
            ));
            return;
        }
    };
    let module: ModuleFile = match serde_json::from_str(&source) {
        Ok(module) => module,
        Err(err) => {
            let span = SourceMap::new(&source)
                .at_line_column(err.line().saturating_sub(1), err.column().saturating_sub(1));
            resolution.diagnostics.push(
                Diagnostic::error(
                    "MOD006",
                    format!(
                        "invalid module JSON at {}:{}: {err}",
                        err.line(),
                        err.column()
                    ),
                    &canonical,
                )
                .with_span(Some(span)),
            );
            return;
        }
    };
    if module.format_version != 1 {
        resolution.diagnostics.push(Diagnostic::error(
            "MOD007",
            format!(
                "module '{block_id}' uses unsupported format_version {}",
                module.format_version
            ),
            &canonical,
        ));
        return;
    }
    let schematic_transform = compose(parent_schematic, import.schematic);
    let pcb_transform = compose(parent_pcb, import.pcb);
    let net_map = |local: &str| {
        import
            .net_map
            .get(local)
            .map(|target| parent_net(target))
            .unwrap_or_else(|| format!("{block_id}/{local}"))
    };
    let mut components = Vec::new();
    for (reference, component) in module.components {
        if project.components.contains_key(&reference) {
            resolution.diagnostics.push(Diagnostic::error(
                "MOD008",
                format!("component '{reference}' is defined more than once"),
                &canonical,
            ));
        } else {
            components.push(reference.clone());
            project.components.insert(reference, component);
        }
    }
    let mut nets = BTreeSet::new();
    for (local, endpoints) in module.nets {
        let global = net_map(&local);
        nets.insert(global.clone());
        project.nets.entry(global).or_default().extend(endpoints);
    }
    merge_schematic(
        &mut project.schematic,
        module.schematic,
        schematic_transform,
        &net_map,
        &canonical,
        &mut resolution.diagnostics,
    );
    merge_pcb(
        project,
        module.pcb,
        pcb_transform,
        &net_map,
        &canonical,
        &mut resolution.diagnostics,
    );
    resolution.blocks.insert(
        block_id.clone(),
        BlockInfo {
            id: block_id.clone(),
            file: canonical
                .strip_prefix(root_dir)
                .unwrap_or(&canonical)
                .to_path_buf(),
            components,
            nets: nets.into_iter().collect(),
            schematic: schematic_transform,
            pcb: pcb_transform,
        },
    );
    stack.push(canonical.clone());
    for child in module.imports {
        merge_import(
            project,
            &child,
            &canonical,
            root_dir,
            &block_id,
            schematic_transform,
            pcb_transform,
            &net_map,
            stack,
            resolution,
        );
    }
    stack.pop();

    let descendant_prefix = format!("{block_id}/");
    let descendant_components: BTreeSet<_> = resolution
        .blocks
        .iter()
        .filter(|(id, _)| id.starts_with(&descendant_prefix))
        .flat_map(|(_, block)| block.components.iter().cloned())
        .collect();
    let descendant_nets: BTreeSet<_> = resolution
        .blocks
        .iter()
        .filter(|(id, _)| id.starts_with(&descendant_prefix))
        .flat_map(|(_, block)| block.nets.iter().cloned())
        .collect();
    if let Some(block) = resolution.blocks.get_mut(&block_id) {
        block.components.extend(descendant_components);
        block.components.sort();
        block.components.dedup();
        block.nets.extend(descendant_nets);
        block.nets.sort();
        block.nets.dedup();
    }
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
        placement.at = apply(transform, placement.at);
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
            .map(|point| apply(transform, point))
            .collect();
        target.wires.push(wire);
    }
    for mut label in source.labels {
        label.net = map_net(&label.net);
        label.at = apply(transform, label.at);
        label.rotation = normalize_angle(label.rotation + transform.rotation);
        target.labels.push(label);
    }
    target.no_connect.extend(source.no_connect);
}

fn merge_pcb(
    project: &mut KilProject,
    source: PcbFragment,
    transform: Transform,
    map_net: &dyn Fn(&str) -> String,
    file: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (reference, mut placement) in source.placement {
        placement.at = apply(transform, placement.at);
        placement.rotation = normalize_angle(placement.rotation + transform.rotation);
        if project
            .pcb
            .placement
            .insert(reference.clone(), placement)
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                "MOD010",
                format!("duplicate PCB placement for '{reference}'"),
                file,
            ));
        }
    }
    for (net, mut routes) in source.routes {
        for route in &mut routes {
            route.path = route
                .path
                .iter()
                .map(|point| apply(transform, *point))
                .collect();
        }
        project
            .pcb
            .routes
            .entry(map_net(&net))
            .or_default()
            .extend(routes);
    }
    for mut via in source.vias {
        via.net = map_net(&via.net);
        via.at = apply(transform, via.at);
        project.pcb.vias.push(via);
    }
    for mut zone in source.zones {
        zone.net = map_net(&zone.net);
        zone.outline = zone
            .outline
            .iter()
            .map(|point| apply(transform, *point))
            .collect();
        project.pcb.zones.push(zone);
    }
    for mut hole in source.holes {
        hole.at = apply(transform, hole.at);
        project.pcb.holes.push(hole);
    }
    for mut silk in source.silk {
        silk.at = apply(transform, silk.at);
        silk.rotation = normalize_angle(silk.rotation + transform.rotation);
        project.pcb.silk.push(silk);
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
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
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
