//! Authoring documents. These types are never mutated by elaboration or generation.
use crate::model::*;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default)]
    pub build: BuildProfile,
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema: Option<String>,
    pub format_version: u32,
    pub project: ProjectMeta,
    #[serde(default)]
    pub circuit: Circuit,
    #[serde(default)]
    pub schematic: SchematicView,
    pub pcb: PcbDesign,
    #[serde(default)]
    pub rules: Rules,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Circuit {
    #[serde(default)]
    pub parts: IndexMap<String, Part>,
    #[serde(default)]
    pub nets: IndexMap<String, Vec<String>>,
    #[serde(default)]
    pub unconnected: Vec<String>,
    #[serde(default)]
    pub instances: IndexMap<String, Instance>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub symbol: String,
    pub footprint: String,
    pub reference_prefix: String,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub value: String,
    /// Explicit aliases for physical terminal numbers. Library display names are not addresses.
    #[serde(default)]
    pub terminals: IndexMap<String, String>,
    #[serde(default)]
    pub fields: IndexMap<String, String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicView {
    #[serde(default)]
    pub symbols: IndexMap<String, SchematicPlacement>,
    #[serde(default)]
    pub wires: Vec<SchematicWire>,
    #[serde(default)]
    pub labels: Vec<SchematicLabel>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Module {
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema: Option<String>,
    pub format_version: u32,
    pub module: String,
    /// Public port name to local net name. All other nets are private.
    pub ports: IndexMap<String, String>,
    #[serde(default)]
    pub parameters: IndexMap<String, Parameter>,
    #[serde(default)]
    pub circuit: Circuit,
    #[serde(default)]
    pub schematic: SchematicView,
    #[serde(default)]
    pub pcb: PcbDesign,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    #[serde(rename = "type")]
    pub kind: ParameterType,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParameterType {
    String,
    Number,
    Boolean,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub source: String,
    pub connections: IndexMap<String, String>,
    #[serde(default)]
    pub parameters: IndexMap<String, serde_json::Value>,
    /// None omits the supplied view, allowing a parent to place the instance's parts.
    #[serde(default)]
    pub schematic: Option<Transform>,
    #[serde(default)]
    pub pcb: Option<Transform>,
}

/// PCB authoring geometry. Anchors resolve after library and module resolution.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PcbDesign {
    #[serde(default)]
    pub outline: Vec<Point>,
    #[serde(default)]
    pub placement: IndexMap<String, PlacementIntent>,
    #[serde(default)]
    pub routes: IndexMap<String, Vec<RouteIntent>>,
    #[serde(default)]
    pub vias: Vec<Via>,
    #[serde(default)]
    pub zones: Vec<Zone>,
    #[serde(default)]
    pub holes: Vec<Hole>,
    #[serde(default)]
    pub silk: Vec<SilkText>,
    #[serde(default)]
    pub constraints: IndexMap<String, DistanceConstraint>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Anchor {
    Point(Point),
    Relative(RelativeAnchor),
    Pad(PadAnchor),
    Edge(EdgeAnchor),
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelativeAnchor {
    pub part: String,
    #[serde(default)]
    pub offset: Point,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadAnchor {
    pub pad: String,
    #[serde(default)]
    pub offset: Point,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EdgeAnchor {
    pub edge: usize,
    pub fraction: f64,
    #[serde(default)]
    pub offset: Point,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlacementMode {
    #[default]
    Fixed,
    Preferred,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlacementIntent {
    pub at: Anchor,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default = "front")]
    pub side: BoardSide,
    #[serde(default)]
    pub mode: PlacementMode,
}
fn front() -> BoardSide {
    BoardSide::Front
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteIntent {
    #[serde(default = "front_copper")]
    pub layer: String,
    #[serde(default)]
    pub width: Option<f64>,
    pub path: Vec<Anchor>,
    #[serde(default)]
    pub locked: bool,
}
fn front_copper() -> String {
    "F.Cu".into()
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DistanceConstraint {
    pub from: Anchor,
    pub to: Anchor,
    pub max: f64,
    #[serde(default)]
    pub preferred: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildProfile {
    #[serde(default)]
    pub routing: Option<RoutingPolicy>,
}
