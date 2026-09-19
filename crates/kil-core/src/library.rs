use crate::diagnostic::Diagnostic;
use crate::model::ResolvedProject;
use crate::source_map::SourceMap;
use indexmap::IndexMap;
use kiutils_kicad::{
    FootprintFile, FpLibTableFile, SymLibTableFile, Symbol, SymbolLibDocument, SymbolLibFile,
};
use kiutils_sexpr::{Atom, Node};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPin {
    pub unit: u32,
    pub number: String,
    pub name: Option<String>,
    pub electrical_type: Option<String>,
    pub graphic_style: Option<String>,
    pub at: [f64; 2],
    pub angle: f64,
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibrarySymbolInfo {
    pub id: String,
    pub source: PathBuf,
    pub derived_from: Vec<String>,
    pub properties: BTreeMap<String, String>,
    pub pins: Vec<ResolvedPin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedComponent {
    pub symbol_id: String,
    pub footprint_id: String,
    pub symbol_node: Node,
    pub footprint_node: Node,
    pub pins: Vec<ResolvedPin>,
    pub pads: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedLibraries {
    pub power_flag: Option<Node>,
    pub components: IndexMap<String, ResolvedComponent>,
}

#[derive(Debug, Clone)]
struct SymbolAsset {
    node: Node,
    pins: Vec<ResolvedPin>,
    derived_from: Vec<String>,
}

#[derive(Debug, Clone)]
struct FootprintAsset {
    node: Node,
    pads: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct AssetError {
    code: &'static str,
    message: String,
}

#[derive(Debug, Clone)]
pub struct LibraryResolver {
    project_dir: PathBuf,
    symbol_dirs: Vec<PathBuf>,
    footprint_dirs: Vec<PathBuf>,
    symbol_tables: BTreeMap<String, PathBuf>,
    footprint_tables: BTreeMap<String, PathBuf>,
    variables: BTreeMap<String, String>,
}

impl LibraryResolver {
    pub fn discover(project_dir: &Path, kicad_cli: Option<&Path>) -> Self {
        let mut resolver = Self {
            project_dir: project_dir.to_path_buf(),
            symbol_dirs: Vec::new(),
            footprint_dirs: Vec::new(),
            symbol_tables: BTreeMap::new(),
            footprint_tables: BTreeMap::new(),
            variables: BTreeMap::new(),
        };
        resolver
            .variables
            .insert("KIPRJMOD".into(), project_dir.display().to_string());
        for key in [
            "KICAD10_SYMBOL_DIR",
            "KICAD10_FOOTPRINT_DIR",
            "KICAD_USER_SYMBOL_DIR",
            "KICAD_USER_FOOTPRINT_DIR",
        ] {
            if let Ok(value) = env::var(key) {
                resolver.variables.insert(key.into(), value.clone());
                if key.contains("SYMBOL") {
                    resolver.symbol_dirs.push(value.clone().into());
                }
                if key.contains("FOOTPRINT") {
                    resolver.footprint_dirs.push(value.into());
                }
            }
        }
        if let Some(cli) = kicad_cli
            && let Some(root) = cli.parent().and_then(Path::parent)
        {
            resolver.add_standard_root(root.join("share").join("kicad"));
        }
        for root in standard_roots() {
            resolver.add_standard_root(root);
        }
        resolver.load_tables();
        resolver
    }

    fn add_standard_root(&mut self, root: PathBuf) {
        let symbols = root.join("symbols");
        let footprints = root.join("footprints");
        if symbols.is_dir() && !self.symbol_dirs.contains(&symbols) {
            self.variables
                .entry("KICAD10_SYMBOL_DIR".into())
                .or_insert_with(|| symbols.display().to_string());
            self.symbol_dirs.push(symbols);
        }
        if footprints.is_dir() && !self.footprint_dirs.contains(&footprints) {
            self.variables
                .entry("KICAD10_FOOTPRINT_DIR".into())
                .or_insert_with(|| footprints.display().to_string());
            self.footprint_dirs.push(footprints);
        }
    }

    fn load_tables(&mut self) {
        let mut roots = global_config_dirs();
        roots.push(self.project_dir.clone());
        for root in roots {
            let sym = root.join("sym-lib-table");
            if sym.is_file()
                && let Ok(doc) = SymLibTableFile::read(&sym)
            {
                for lib in &doc.ast().libraries {
                    if !lib.disabled
                        && let (Some(name), Some(uri)) = (&lib.name, &lib.uri)
                    {
                        let path = self.expand_uri(uri);
                        self.symbol_tables.insert(name.clone(), path);
                    }
                }
            }
            let fp = root.join("fp-lib-table");
            if fp.is_file()
                && let Ok(doc) = FpLibTableFile::read(&fp)
            {
                for lib in &doc.ast().libraries {
                    if !lib.disabled
                        && let (Some(name), Some(uri)) = (&lib.name, &lib.uri)
                    {
                        let path = self.expand_uri(uri);
                        self.footprint_tables.insert(name.clone(), path);
                    }
                }
            }
        }
    }

    fn expand_uri(&self, uri: &str) -> PathBuf {
        let mut value = uri.to_string();
        for (key, replacement) in &self.variables {
            value = value.replace(&format!("${{{key}}}"), replacement);
        }
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            self.project_dir.join(path)
        }
    }

    fn symbol_path<'a>(&self, id: &'a str) -> Option<(PathBuf, &'a str)> {
        let (nickname, entry) = id.split_once(':')?;
        let table_path = self.symbol_tables.get(nickname).cloned();
        let path = table_path.or_else(|| {
            self.symbol_dirs
                .iter()
                .map(|dir| dir.join(format!("{nickname}.kicad_sym")))
                .find(|path| path.is_file())
        })?;
        Some((path, entry))
    }

    fn footprint_path(&self, id: &str) -> Option<PathBuf> {
        let (nickname, entry) = id.split_once(':')?;
        let base = self.footprint_tables.get(nickname).cloned().or_else(|| {
            self.footprint_dirs
                .iter()
                .map(|dir| dir.join(format!("{nickname}.pretty")))
                .find(|path| path.is_dir())
        })?;
        let path = base.join(format!("{entry}.kicad_mod"));
        path.is_file().then_some(path)
    }

    /// Resolve paths and hash actual dependency contents on every call. Only the
    /// expensive parsing/inheritance work is reused; metadata alone is not proof.
    fn resolution_key(&self, project: &ResolvedProject) -> Option<String> {
        use sha2::{Digest, Sha256};
        let mut paths = BTreeSet::new();
        let mut identities = Vec::new();
        for (id, component) in &project.components {
            paths.insert(self.symbol_path(&component.symbol)?.0);
            paths.insert(self.footprint_path(&component.footprint)?);
            identities.push((id, &component.symbol, &component.footprint));
        }
        if !project.power_sources.is_empty() {
            paths.insert(self.symbol_path("power:PWR_FLAG")?.0);
        }
        let mut hash = Sha256::new();
        hash.update(
            serde_json::to_vec(&(
                1,
                env!("CARGO_PKG_VERSION"),
                identities,
                !project.power_sources.is_empty(),
            ))
            .ok()?,
        );
        for path in paths {
            let bytes = std::fs::read(&path).ok()?;
            hash.update(serde_json::to_vec(&path).ok()?);
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        Some(format!("{:x}", hash.finalize()))
    }

    pub fn resolve_cached(
        &self,
        project: &ResolvedProject,
        file: &Path,
        source: &str,
    ) -> (ResolvedLibraries, Vec<Diagnostic>, bool) {
        let key = self.resolution_key(project);
        let cache = key.as_ref().map(|key| {
            self.project_dir
                .join("build/library-cache")
                .join(format!("{key}.json"))
        });
        if let Some(path) = &cache
            && let Ok(bytes) = std::fs::read(path)
            && let Ok(libraries) = serde_json::from_slice(&bytes)
        {
            return (libraries, Vec::new(), true);
        }
        let (libraries, diagnostics) = self.resolve_all(project, file, source);
        // Do not cache failures or a library edited during resolution.
        if diagnostics.is_empty()
            && key.is_some()
            && key == self.resolution_key(project)
            && let Some(path) = cache
            && std::fs::create_dir_all(path.parent().unwrap()).is_ok()
        {
            let _ = crate::route_workflow::write_json_atomic(&path, &libraries);
        }
        (libraries, diagnostics, false)
    }

    pub fn resolve_all(
        &self,
        project: &ResolvedProject,
        file: &Path,
        source: &str,
    ) -> (ResolvedLibraries, Vec<Diagnostic>) {
        let map = SourceMap::new(source);
        let mut resolved = ResolvedLibraries::default();
        let mut diagnostics = Vec::new();
        let mut symbol_cache = BTreeMap::<String, Result<SymbolAsset, AssetError>>::new();
        let mut footprint_cache = BTreeMap::<String, Result<FootprintAsset, AssetError>>::new();
        for (reference, component) in &project.components {
            let symbol_path_key = format!("/components/{reference}/symbol");
            let footprint_path_key = format!("/components/{reference}/footprint");
            let symbol_asset = symbol_cache
                .entry(component.symbol.clone())
                .or_insert_with(|| self.load_symbol(&component.symbol))
                .clone();
            let symbol_asset = match symbol_asset {
                Ok(asset) => asset,
                Err(err) => {
                    diagnostics.push(
                        Diagnostic::error(err.code, err.message, file)
                            .at_path(symbol_path_key.clone())
                            .with_span(map.span_for_path(&symbol_path_key)),
                    );
                    continue;
                }
            };
            let footprint_asset = footprint_cache
                .entry(component.footprint.clone())
                .or_insert_with(|| self.load_footprint(&component.footprint))
                .clone();
            let footprint_asset = match footprint_asset {
                Ok(asset) => asset,
                Err(err) => {
                    diagnostics.push(
                        Diagnostic::error(err.code, err.message, file)
                            .at_path(footprint_path_key.clone())
                            .with_span(map.span_for_path(&footprint_path_key)),
                    );
                    continue;
                }
            };
            let symbol_pin_numbers = symbol_asset
                .pins
                .iter()
                .map(|pin| pin.number.as_str())
                .collect::<BTreeSet<_>>();
            let missing = symbol_pin_numbers
                .iter()
                .filter(|pin| !footprint_asset.pads.contains(**pin))
                .copied()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "LIB003",
                        format!(
                            "footprint '{}' lacks symbol pads: {}",
                            component.footprint,
                            missing.join(", ")
                        ),
                        file,
                    )
                    .at_path(footprint_path_key.clone())
                    .with_span(map.span_for_path(&footprint_path_key)),
                );
                continue;
            }
            resolved.components.insert(
                reference.clone(),
                ResolvedComponent {
                    symbol_id: component.symbol.clone(),
                    footprint_id: component.footprint.clone(),
                    symbol_node: symbol_asset.node,
                    footprint_node: footprint_asset.node,
                    pins: symbol_asset.pins,
                    pads: footprint_asset.pads,
                },
            );
        }
        if !project.power_sources.is_empty() {
            match self.load_symbol("power:PWR_FLAG") {
                Ok(asset) => resolved.power_flag = Some(asset.node),
                Err(err) => diagnostics.push(
                    Diagnostic::error(err.code, err.message, file)
                        .at_path("/circuit/power_sources"),
                ),
            }
        }
        (resolved, diagnostics)
    }

    pub fn show_symbol(&self, id: &str) -> Result<LibrarySymbolInfo, String> {
        let (source, _) = self
            .symbol_path(id)
            .ok_or_else(|| format!("cannot resolve symbol library for '{id}'"))?;
        let asset = self.load_symbol(id).map_err(|error| error.message)?;
        Ok(LibrarySymbolInfo {
            id: id.to_string(),
            source,
            derived_from: asset.derived_from,
            properties: symbol_properties(&asset.node),
            pins: asset.pins,
        })
    }

    fn load_symbol(&self, id: &str) -> Result<SymbolAsset, AssetError> {
        let Some((path, entry)) = self.symbol_path(id) else {
            return Err(AssetError {
                code: "LIB001",
                message: format!("cannot resolve symbol '{id}'"),
            });
        };
        let doc = SymbolLibFile::read(&path).map_err(|err| AssetError {
            code: "LIB006",
            message: format!("failed to parse {}: {err}", path.display()),
        })?;
        resolve_symbol(&doc, entry, id, &mut Vec::new())
    }

    fn load_footprint(&self, id: &str) -> Result<FootprintAsset, AssetError> {
        let path = self.footprint_path(id).ok_or_else(|| AssetError {
            code: "LIB002",
            message: format!("cannot resolve footprint '{id}'"),
        })?;
        let doc = FootprintFile::read(&path).map_err(|err| AssetError {
            code: "LIB006",
            message: format!("failed to parse {}: {err}", path.display()),
        })?;
        let node = doc.cst().nodes.first().cloned().ok_or_else(|| AssetError {
            code: "LIB006",
            message: format!("cannot extract footprint '{id}'"),
        })?;
        let pads = doc
            .ast()
            .pads
            .iter()
            .filter_map(|pad| pad.number.clone())
            .collect();
        Ok(FootprintAsset { node, pads })
    }
}

fn resolve_symbol(
    doc: &SymbolLibDocument,
    entry: &str,
    id: &str,
    stack: &mut Vec<String>,
) -> Result<SymbolAsset, AssetError> {
    if stack.iter().any(|name| name == entry) {
        stack.push(entry.to_string());
        return Err(AssetError {
            code: "LIB007",
            message: format!("derived symbol cycle in '{id}': {}", stack.join(" -> ")),
        });
    }
    let symbol = doc
        .ast()
        .symbols
        .iter()
        .find(|symbol| symbol.name.as_deref() == Some(entry))
        .ok_or_else(|| AssetError {
            code: "LIB001",
            message: format!("symbol '{id}' is absent from its library"),
        })?;
    let node = find_named_symbol_node(&doc.cst().nodes, entry)
        .cloned()
        .ok_or_else(|| AssetError {
            code: "LIB006",
            message: format!("cannot extract symbol '{id}'"),
        })?;

    if let Some(base_name) = &symbol.extends {
        stack.push(entry.to_string());
        let mut asset = resolve_symbol(doc, base_name, id, stack)?;
        stack.pop();
        asset.node = merge_derived_symbol(&asset.node, &node, base_name, entry);
        asset.derived_from.insert(0, base_name.clone());
        return Ok(asset);
    }

    symbol_asset_from_base(symbol, node, id)
}

fn symbol_asset_from_base(
    symbol: &Symbol,
    node: Node,
    _id: &str,
) -> Result<SymbolAsset, AssetError> {
    let pins = symbol
        .pins
        .iter()
        .map(|pin| (0, pin))
        .chain(symbol.units.iter().flat_map(|unit| {
            let number = unit
                .name
                .as_deref()
                .and_then(symbol_unit_number)
                .unwrap_or(1);
            unit.pins.iter().map(move |pin| (number, pin))
        }))
        .filter_map(|(unit, pin)| {
            Some(ResolvedPin {
                unit,
                number: pin.number.clone()?,
                name: pin.name.clone(),
                electrical_type: pin.electrical_type.clone(),
                graphic_style: pin.graphic_style.clone(),
                at: pin.at.unwrap_or([0.0, 0.0]),
                angle: pin.angle.unwrap_or(0.0),
                hidden: pin.hide,
            })
        })
        .collect();
    Ok(SymbolAsset {
        node,
        pins,
        derived_from: Vec::new(),
    })
}

fn merge_derived_symbol(base: &Node, derived: &Node, base_name: &str, name: &str) -> Node {
    let mut merged = base.clone();
    set_node_name(&mut merged, name);
    let Node::List {
        items: merged_items,
        ..
    } = &mut merged
    else {
        return merged;
    };
    for child in merged_items.iter_mut().skip(2) {
        if node_head(child) == Some("symbol")
            && let Some(unit_name) = node_second(child)
            && let Some(suffix) = unit_name.strip_prefix(base_name)
        {
            set_node_name(child, &format!("{name}{suffix}"));
        }
    }
    let Node::List {
        items: derived_items,
        ..
    } = derived
    else {
        return merged;
    };
    for child in derived_items.iter().skip(2) {
        let Some(head) = node_head(child) else {
            continue;
        };
        if head == "extends" {
            continue;
        }
        let existing = if head == "property" {
            let key = node_second(child);
            merged_items.iter().position(|candidate| {
                node_head(candidate) == Some("property") && node_second(candidate) == key
            })
        } else {
            merged_items
                .iter()
                .position(|candidate| node_head(candidate) == Some(head))
        };
        if let Some(index) = existing {
            merged_items[index] = child.clone();
        } else {
            merged_items.push(child.clone());
        }
    }
    merged
}

fn symbol_properties(node: &Node) -> BTreeMap<String, String> {
    let mut properties = BTreeMap::new();
    let Node::List { items, .. } = node else {
        return properties;
    };
    for child in items {
        if node_head(child) == Some("property")
            && let (Some(key), Some(value)) = (node_second(child), node_third(child))
        {
            properties.insert(key.to_string(), value.to_string());
        }
    }
    properties
}

fn set_node_name(node: &mut Node, value: &str) {
    if let Node::List { items, .. } = node
        && let Some(Node::Atom { atom, .. }) = items.get_mut(1)
    {
        *atom = Atom::Quoted(value.to_string());
    }
}

fn node_head(node: &Node) -> Option<&str> {
    let Node::List { items, .. } = node else {
        return None;
    };
    atom_text(items.first()?)
}

fn node_second(node: &Node) -> Option<&str> {
    let Node::List { items, .. } = node else {
        return None;
    };
    atom_text(items.get(1)?)
}

fn node_third(node: &Node) -> Option<&str> {
    let Node::List { items, .. } = node else {
        return None;
    };
    atom_text(items.get(2)?)
}

fn atom_text(node: &Node) -> Option<&str> {
    match node {
        Node::Atom {
            atom: Atom::Quoted(value) | Atom::Symbol(value),
            ..
        } => Some(value),
        _ => None,
    }
}

fn find_named_symbol_node<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
    let Node::List { items, .. } = nodes.first()? else {
        return None;
    };
    items.iter().find(|node| {
        let Node::List { items, .. } = node else { return false };
        matches!(items.first(), Some(Node::Atom { atom: Atom::Symbol(head), .. }) if head == "symbol")
            && matches!(items.get(1), Some(Node::Atom { atom: Atom::Quoted(value), .. }) if value == name)
    })
}

fn symbol_unit_number(name: &str) -> Option<u32> {
    let mut parts = name.rsplit('_');
    let _style = parts.next()?;
    parts.next()?.parse().ok()
}

fn standard_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/share/kicad"),
        PathBuf::from("/usr/local/share/kicad"),
        #[cfg(target_os = "windows")]
        PathBuf::from(r"C:\Program Files\KiCad\10.0\share\kicad"),
        #[cfg(target_os = "macos")]
        PathBuf::from("/Applications/KiCad/KiCad.app/Contents/SharedSupport"),
    ]
}

fn global_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(appdata) = env::var("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("kicad").join("10.0"));
    }
    if let Ok(config) = env::var("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(config).join("kicad").join("10.0"));
    }
    if let Ok(user_profile) = env::var("USERPROFILE") {
        dirs.push(
            PathBuf::from(user_profile)
                .join(".config")
                .join("kicad")
                .join("10.0"),
        );
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiutils_sexpr::Span;

    const ZERO: Span = Span { start: 0, end: 0 };

    fn symbol(value: &str) -> Node {
        Node::Atom {
            atom: Atom::Symbol(value.to_string()),
            span: ZERO,
        }
    }

    fn quoted(value: &str) -> Node {
        Node::Atom {
            atom: Atom::Quoted(value.to_string()),
            span: ZERO,
        }
    }

    fn list(items: Vec<Node>) -> Node {
        Node::List { items, span: ZERO }
    }

    #[test]
    fn resolution_cache_verifies_content_paths_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let sym = dir.path().join("test.kicad_sym");
        std::fs::write(&sym, include_str!("../testdata/test.kicad_sym")).unwrap();
        let fp = dir.path().join("test.pretty");
        std::fs::create_dir(&fp).unwrap();
        std::fs::write(
            fp.join("R.kicad_mod"),
            include_str!("../testdata/R.kicad_mod"),
        )
        .unwrap();
        let mut project: ResolvedProject =
            serde_json::from_str(include_str!("../testdata/resolved/two-resistors.kil.json"))
                .unwrap();
        project.power_sources.clear();
        for component in project.components.values_mut() {
            component.symbol = "test:R".into();
            component.footprint = "test:R".into();
        }
        let mut resolver = LibraryResolver::discover(dir.path(), None);
        resolver.symbol_tables.insert("test".into(), sym.clone());
        resolver.footprint_tables.insert("test".into(), fp.clone());
        let file = dir.path().join("project.kil.json");
        let (first, errors, hit) = resolver.resolve_cached(&project, &file, "{}");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!hit);
        let (second, errors, hit) = resolver.resolve_cached(&project, &file, "{}");
        assert!(errors.is_empty());
        assert!(hit);
        assert_eq!(
            crate::lock::snapshot(&first),
            crate::lock::snapshot(&second)
        );
        crate::lock::write(&file, &first).unwrap();
        let path = fp.join("R.kicad_mod");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("(size 0.8 0.9)", "(size 0.9 0.9)")).unwrap();
        let (changed, _, hit) = resolver.resolve_cached(&project, &file, "{}");
        assert!(!hit);
        assert!(crate::lock::verify(&file, &changed).is_err());
        let key = resolver.resolution_key(&project).unwrap();
        std::fs::write(
            dir.path()
                .join("build/library-cache")
                .join(format!("{key}.json")),
            b"broken",
        )
        .unwrap();
        assert!(!resolver.resolve_cached(&project, &file, "{}").2);
        assert!(resolver.resolve_cached(&project, &file, "{}").2);
        let other = dir.path().join("other.kicad_sym");
        std::fs::copy(&sym, &other).unwrap();
        resolver.symbol_tables.insert("test".into(), other);
        assert!(!resolver.resolve_cached(&project, &file, "{}").2);
    }

    #[test]
    fn derived_symbol_overrides_properties_and_renames_units() {
        let base = list(vec![
            symbol("symbol"),
            quoted("Base"),
            list(vec![symbol("property"), quoted("Value"), quoted("Base")]),
            list(vec![symbol("symbol"), quoted("Base_0_1")]),
            list(vec![symbol("symbol"), quoted("Base_1_1")]),
        ]);
        let derived = list(vec![
            symbol("symbol"),
            quoted("Child"),
            list(vec![symbol("extends"), quoted("Base")]),
            list(vec![symbol("property"), quoted("Value"), quoted("Child")]),
        ]);

        let merged = merge_derived_symbol(&base, &derived, "Base", "Child");
        let Node::List { items, .. } = &merged else {
            unreachable!();
        };

        assert_eq!(node_second(&merged), Some("Child"));
        assert!(!items.iter().any(|node| node_head(node) == Some("extends")));
        assert_eq!(symbol_properties(&merged).get("Value").unwrap(), "Child");
        assert!(
            items
                .iter()
                .any(|node| node_second(node) == Some("Child_0_1"))
        );
        assert!(
            items
                .iter()
                .any(|node| node_second(node) == Some("Child_1_1"))
        );
    }
}
