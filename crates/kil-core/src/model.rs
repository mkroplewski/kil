use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type Point = [f64; 2];

pub const PROJECT_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/mkroplewski/kil/main/schemas/kil-v1.schema.json";
pub const MODULE_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/mkroplewski/kil/main/schemas/kil-module-v1.schema.json";

fn default_units() -> Units {
    Units::Mm
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    Mm,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KilProject {
    #[serde(rename = "$schema", default, skip_serializing)]
    #[schemars(with = "String")]
    #[schemars(description = "JSON Schema URI used by editors for completion and validation.")]
    pub schema: Option<String>,
    pub format_version: u32,
    pub project: ProjectMeta,
    #[serde(default = "default_units")]
    pub units: Units,
    pub components: IndexMap<String, Component>,
    pub nets: IndexMap<String, Vec<String>>,
    #[serde(default)]
    pub imports: Vec<ModuleImport>,
    #[serde(default)]
    pub schematic: Schematic,
    pub pcb: Pcb,
    #[serde(default)]
    pub rules: Rules,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleFile {
    #[serde(rename = "$schema", default, skip_serializing)]
    #[schemars(with = "String")]
    #[schemars(description = "JSON Schema URI used by editors for completion and validation.")]
    pub schema: Option<String>,
    pub format_version: u32,
    pub module: ModuleMeta,
    #[serde(default)]
    pub components: IndexMap<String, Component>,
    #[serde(default)]
    pub nets: IndexMap<String, Vec<String>>,
    #[serde(default)]
    pub imports: Vec<ModuleImport>,
    #[serde(default)]
    pub schematic: Schematic,
    #[serde(default)]
    pub pcb: PcbFragment,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleMeta {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleImport {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub net_map: IndexMap<String, String>,
    #[serde(default)]
    pub schematic: Transform,
    #[serde(default)]
    pub pcb: Transform,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    #[serde(default)]
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            at: [0.0, 0.0],
            rotation: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_kicad_series")]
    pub kicad: u32,
}

fn default_kicad_series() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub symbol: String,
    #[serde(default)]
    pub value: String,
    pub footprint: String,
    #[serde(default)]
    pub fields: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Schematic {
    #[serde(default)]
    pub placement: IndexMap<String, SchematicPlacement>,
    #[serde(default)]
    pub wires: Vec<SchematicWire>,
    #[serde(default)]
    pub labels: Vec<SchematicLabel>,
    #[serde(default)]
    pub no_connect: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicPlacement {
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicWire {
    pub net: String,
    pub path: Vec<Point>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicLabel {
    pub net: String,
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pcb {
    pub outline: Vec<Point>,
    pub placement: IndexMap<String, PcbPlacement>,
    #[serde(default)]
    pub routes: IndexMap<String, Vec<Route>>,
    #[serde(default)]
    pub vias: Vec<Via>,
    #[serde(default)]
    pub zones: Vec<Zone>,
    #[serde(default)]
    pub holes: Vec<Hole>,
    #[serde(default)]
    pub silk: Vec<SilkText>,
    #[serde(default)]
    pub routing: Option<RoutingPolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PcbFragment {
    #[serde(default)]
    pub placement: IndexMap<String, PcbPlacement>,
    #[serde(default)]
    pub routes: IndexMap<String, Vec<Route>>,
    #[serde(default)]
    pub vias: Vec<Via>,
    #[serde(default)]
    pub zones: Vec<Zone>,
    #[serde(default)]
    pub holes: Vec<Hole>,
    #[serde(default)]
    pub silk: Vec<SilkText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingEngine {
    KicadRoutingTools,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPolicy {
    pub engine: RoutingEngine,
    #[serde(default = "default_route_nets")]
    pub nets: Vec<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub cache: Option<String>,
}

fn default_route_nets() -> Vec<String> {
    vec!["*".into()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BoardSide {
    Front,
    Back,
}

fn default_side() -> BoardSide {
    BoardSide::Front
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PcbPlacement {
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default = "default_side")]
    pub side: BoardSide,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Route {
    #[serde(default = "default_front_copper")]
    pub layer: String,
    #[serde(default)]
    pub width: Option<f64>,
    pub path: Vec<Point>,
    #[serde(default)]
    pub locked: bool,
}

fn default_front_copper() -> String {
    "F.Cu".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Via {
    pub net: String,
    pub at: Point,
    #[serde(default)]
    pub size: Option<f64>,
    #[serde(default)]
    pub drill: Option<f64>,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    pub net: String,
    #[serde(default = "default_front_copper")]
    pub layer: String,
    pub outline: Vec<Point>,
    #[serde(default)]
    pub clearance: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub at: Point,
    pub diameter: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SilkText {
    pub text: String,
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default = "default_front_silk")]
    pub layer: String,
}

fn default_front_silk() -> String {
    "F.SilkS".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    #[serde(default = "default_clearance")]
    pub clearance: f64,
    #[serde(default = "default_track_width")]
    pub track_width: f64,
    #[serde(default = "default_via_size")]
    pub via_size: f64,
    #[serde(default = "default_via_drill")]
    pub via_drill: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            clearance: default_clearance(),
            track_width: default_track_width(),
            via_size: default_via_size(),
            via_drill: default_via_drill(),
        }
    }
}

fn default_clearance() -> f64 {
    0.2
}
fn default_track_width() -> f64 {
    0.25
}
fn default_via_size() -> f64 {
    0.8
}
fn default_via_drill() -> f64 {
    0.4
}
