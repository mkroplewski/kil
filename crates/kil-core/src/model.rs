use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type Point = [f64; 2];

pub const PROJECT_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/mkroplewski/kil/main/schemas/kil-v2.schema.json";
pub const MODULE_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/mkroplewski/kil/main/schemas/kil-module-v2.schema.json";

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
pub struct ResolvedProject {
    #[serde(default)]
    pub power_sources: Vec<String>,
    #[serde(default)]
    pub library_fingerprint: String,
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
    pub schematic: Schematic,
    pub pcb: Pcb,
    #[serde(default)]
    pub rules: Rules,
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
    pub reference: String,
    #[serde(default)]
    pub aliases: IndexMap<String, String>,
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
    pub sheets: std::collections::BTreeSet<String>,
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
    /// Resolved page identity, assigned by the enclosing source page.
    #[serde(skip)]
    pub sheet: String,
    pub part: String,
    pub unit: u32,
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicWire {
    /// Resolved page identity, assigned by the enclosing source page.
    #[serde(skip)]
    pub sheet: String,
    pub net: String,
    pub path: Vec<Point>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicLabel {
    /// Resolved page identity, assigned by the enclosing source page.
    #[serde(skip)]
    pub sheet: String,
    pub net: String,
    pub at: Point,
    #[serde(default)]
    pub rotation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pcb {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keepouts: Vec<Keepout>,
    #[serde(default)]
    pub stackup: crate::stackup::Stackup,
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
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub solid: bool,
    #[serde(default)]
    pub thermal_gap: Option<f64>,
    #[serde(default)]
    pub thermal_width: Option<f64>,
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
    #[serde(default = "default_min_width")]
    pub minimum_track_width: f64,
    #[serde(default)]
    pub net_classes: IndexMap<String, NetClass>,
    #[serde(default = "default_clearance")]
    pub clearance: f64,
    #[serde(default = "default_track_width")]
    pub preferred_track_width: f64,
    #[serde(default = "default_via_size")]
    pub via_size: f64,
    #[serde(default = "default_via_drill")]
    pub via_drill: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            clearance: default_clearance(),
            preferred_track_width: default_track_width(),
            minimum_track_width: default_min_width(),
            net_classes: IndexMap::new(),
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

fn default_min_width() -> f64 {
    0.15
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetClass {
    pub nets: Vec<String>,
    pub clearance: f64,
    pub minimum_track_width: f64,
    pub preferred_track_width: f64,
    #[serde(default)]
    pub allowed_layers: Vec<String>,
    /// Override allowed_layers for copper pours. Empty explicitly permits every layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_layers: Option<Vec<String>>,
    /// Override the default through-via policy, which requires both outer track layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_through_vias: Option<bool>,
}
impl NetClass {
    pub fn allows_zone(&self, layer: &str) -> bool {
        self.zone_layers.as_ref().map_or_else(
            || self.allows(layer),
            |layers| layers.is_empty() || layers.iter().any(|l| l == layer),
        )
    }
    pub fn allows_vias(&self) -> bool {
        self.allow_through_vias
            .unwrap_or_else(|| self.allows("F.Cu") && self.allows("B.Cu"))
    }

    pub fn allows(&self, layer: &str) -> bool {
        self.allowed_layers.is_empty() || self.allowed_layers.iter().any(|l| l == layer)
    }
}
impl Rules {
    pub fn class(&self, net: &str) -> Option<&NetClass> {
        self.net_classes
            .values()
            .find(|c| c.nets.iter().any(|n| n == net))
    }
    pub fn width(&self, net: &str) -> f64 {
        self.class(net)
            .map_or(self.preferred_track_width, |c| c.preferred_track_width)
    }
}

/// Names embedded in KiCad rule expressions use a restricted identifier alphabet.
pub(crate) fn valid_net_class_name(name: &str) -> bool {
    !name.is_empty()
        && name != "Default"
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A region reserved from copper and component placement on the selected layers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub outline: Vec<Point>,
    /// Empty means all copper layers.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Whether the keepout excludes tracks.
    #[serde(
        default = "keepout_restriction_default",
        skip_serializing_if = "is_true"
    )]
    pub tracks: bool,
    /// Whether the keepout excludes vias.
    #[serde(
        default = "keepout_restriction_default",
        skip_serializing_if = "is_true"
    )]
    pub vias: bool,
    /// Whether the keepout excludes pads.
    #[serde(
        default = "keepout_restriction_default",
        skip_serializing_if = "is_true"
    )]
    pub pads: bool,
    /// Whether the keepout excludes copper pours.
    #[serde(
        default = "keepout_restriction_default",
        skip_serializing_if = "is_true"
    )]
    pub copper_pours: bool,
    /// Whether the keepout excludes footprints.
    #[serde(
        default = "keepout_restriction_default",
        skip_serializing_if = "is_true"
    )]
    pub footprints: bool,
}

fn keepout_restriction_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}
