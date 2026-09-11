use crate::library::{ResolvedComponent, ResolvedLibraries, ResolvedPin};
use crate::model::{BoardSide, Point, ResolvedProject};
use crate::validate::parse_endpoint;
use indexmap::IndexMap;
use kiutils_kicad::{PcbFile, SchematicFile};
use kiutils_sexpr::{Atom, CstDocument, Node, Span};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const ZERO: Span = Span { start: 0, end: 0 };

#[derive(Debug, Clone)]
pub struct GeneratedProject {
    pub design_rules: String,
    pub project: String,
    pub schematic: String,
    pub pcb: String,
}

impl GeneratedProject {
    pub fn write_to(&self, directory: &Path, name: &str) -> std::io::Result<Vec<PathBuf>> {
        fs::create_dir_all(directory)?;
        let project_path = directory.join(format!("{name}.kicad_pro"));
        let schematic_path = directory.join(format!("{name}.kicad_sch"));
        let pcb_path = directory.join(format!("{name}.kicad_pcb"));
        fs::write(
            directory.join(format!("{name}.kicad_dru")),
            &self.design_rules,
        )?;
        fs::write(&project_path, &self.project)?;
        fs::write(&schematic_path, &self.schematic)?;
        fs::write(&pcb_path, &self.pcb)?;
        Ok(vec![project_path, schematic_path, pcb_path])
    }
}

pub fn generate(project: &ResolvedProject, libraries: &ResolvedLibraries) -> GeneratedProject {
    let schematic_node = schematic_node(project, libraries);
    let pcb_node = pcb_node(project, libraries);
    let schematic = CstDocument {
        raw: String::new(),
        nodes: vec![schematic_node],
    }
    .to_canonical_string();
    let pcb = CstDocument {
        raw: String::new(),
        nodes: vec![pcb_node],
    }
    .to_canonical_string();
    let project_json = project_json(project);
    GeneratedProject {
        design_rules: design_rules(project),
        project: project_json,
        schematic,
        pcb,
    }
}

pub fn validate_generated(directory: &Path, name: &str) -> Result<(), String> {
    let sch = directory.join(format!("{name}.kicad_sch"));
    let pcb = directory.join(format!("{name}.kicad_pcb"));
    let sch_doc = SchematicFile::read(&sch)
        .map_err(|err| format!("generated schematic is invalid: {err}"))?;
    let pcb_doc = PcbFile::read(&pcb).map_err(|err| format!("generated PCB is invalid: {err}"))?;
    if sch_doc.ast().symbol_count == 0 && sch_doc.ast().lib_symbol_count > 0 {
        return Err("generated schematic contains libraries but no placed symbols".into());
    }
    if pcb_doc.ast().footprint_count == 0 && pcb_doc.ast().net_count > 1 {
        return Err("generated PCB contains nets but no footprints".into());
    }
    Ok(())
}

fn schematic_node(project: &ResolvedProject, libraries: &ResolvedLibraries) -> Node {
    let root_uuid = stable_uuid(project, "schematic/root");
    let mut root = vec![
        sym("kicad_sch"),
        list(vec![sym("version"), sym("20250114")]),
        list(vec![sym("generator"), quoted("kil")]),
        list(vec![
            sym("generator_version"),
            quoted(env!("CARGO_PKG_VERSION")),
        ]),
        list(vec![sym("uuid"), quoted(root_uuid.to_string())]),
        list(vec![sym("paper"), quoted("A4")]),
    ];

    let mut embedded = vec![sym("lib_symbols")];
    let mut seen = BTreeMap::<String, Node>::new();
    for resolved in libraries.components.values() {
        seen.entry(resolved.symbol_id.clone()).or_insert_with(|| {
            let mut node = resolved.symbol_node.clone();
            set_second_quoted(&mut node, &resolved.symbol_id);
            node
        });
    }
    embedded.extend(seen.into_values());
    root.push(list(embedded));

    for (index, (view_id, placement)) in project.schematic.placement.iter().enumerate() {
        let reference = &placement.part;
        let Some(component) = project.components.get(reference) else {
            continue;
        };
        let Some(resolved) = libraries.components.get(reference) else {
            continue;
        };

        let instance_uuid = stable_uuid(project, &format!("schematic/component/{view_id}"));
        let value = if component.value.is_empty() {
            component.reference.as_str()
        } else {
            component.value.as_str()
        };
        let mut items = vec![
            sym("symbol"),
            list(vec![sym("lib_id"), quoted(&component.symbol)]),
            at3(placement.at[0], placement.at[1], placement.rotation),
            list(vec![sym("unit"), sym(placement.unit.to_string())]),
            list(vec![sym("exclude_from_sim"), sym("no")]),
            list(vec![sym("in_bom"), sym("yes")]),
            list(vec![sym("on_board"), sym("yes")]),
            list(vec![sym("dnp"), sym("no")]),
            list(vec![sym("uuid"), quoted(instance_uuid.to_string())]),
            property_node(
                project,
                reference,
                "Reference",
                &component.reference,
                placement.at,
                index,
                false,
            ),
            property_node(
                project,
                reference,
                "Value",
                value,
                [placement.at[0], placement.at[1] + 2.54],
                index,
                false,
            ),
            property_node(
                project,
                reference,
                "Footprint",
                &component.footprint,
                placement.at,
                index,
                true,
            ),
            property_node(
                project,
                reference,
                "Datasheet",
                component
                    .fields
                    .get("Datasheet")
                    .map_or("~", String::as_str),
                placement.at,
                index,
                true,
            ),
        ];
        for (field, value) in &component.fields {
            if field != "Datasheet" {
                items.push(property_node(
                    project,
                    reference,
                    field,
                    value,
                    placement.at,
                    index,
                    true,
                ));
            }
        }
        for pin in unique_pins(&resolved.pins)
            .into_iter()
            .filter(|p| p.unit == 0 || p.unit == placement.unit)
        {
            items.push(list(vec![
                sym("pin"),
                quoted(&pin.number),
                list(vec![
                    sym("uuid"),
                    quoted(
                        stable_uuid(
                            project,
                            &format!("schematic/component/{view_id}/pin/{}", pin.number),
                        )
                        .to_string(),
                    ),
                ]),
            ]));
        }
        items.push(list(vec![
            sym("instances"),
            list(vec![
                sym("project"),
                quoted(&project.project.name),
                list(vec![
                    sym("path"),
                    quoted(format!("/{root_uuid}")),
                    list(vec![sym("reference"), quoted(&component.reference)]),
                    list(vec![sym("unit"), sym(placement.unit.to_string())]),
                ]),
            ]),
        ]));
        root.push(list(items));
    }

    for (wire_index, wire) in project.schematic.wires.iter().enumerate() {
        for (segment_index, segment) in wire.path.windows(2).enumerate() {
            root.push(list(vec![
                sym("wire"),
                list(vec![sym("pts"), xy(segment[0]), xy(segment[1])]),
                stroke(0.0),
                list(vec![
                    sym("uuid"),
                    quoted(
                        stable_uuid(
                            project,
                            &format!("schematic/wire/{wire_index}/{segment_index}"),
                        )
                        .to_string(),
                    ),
                ]),
            ]));
        }
    }

    // Net membership is authoritative. A local label on every member pin gives
    // KiCad the same connectivity even when the drawing omits long wires.
    for (net, endpoints) in &project.nets {
        for endpoint in endpoints {
            if let Some(at) = endpoint_position(project, libraries, endpoint) {
                root.push(label_node(
                    project,
                    net,
                    at,
                    0.0,
                    &format!("endpoint/{endpoint}"),
                ));
            }
        }
    }
    for (index, label) in project.schematic.labels.iter().enumerate() {
        root.push(label_node(
            project,
            &label.net,
            label.at,
            label.rotation,
            &format!("explicit/{index}"),
        ));
    }
    for endpoint in &project.schematic.no_connect {
        if let Some(at) = endpoint_position(project, libraries, endpoint) {
            root.push(list(vec![
                sym("no_connect"),
                at2(at),
                list(vec![
                    sym("uuid"),
                    quoted(
                        stable_uuid(project, &format!("schematic/no-connect/{endpoint}"))
                            .to_string(),
                    ),
                ]),
            ]));
        }
    }

    root.push(list(vec![
        sym("sheet_instances"),
        list(vec![
            sym("path"),
            quoted("/"),
            list(vec![sym("page"), quoted("1")]),
        ]),
    ]));
    list(root)
}

fn pcb_node(project: &ResolvedProject, libraries: &ResolvedLibraries) -> Node {
    let mut root = vec![
        sym("kicad_pcb"),
        list(vec![sym("version"), sym("20250114")]),
        list(vec![sym("generator"), sym("kil")]),
        list(vec![
            sym("generator_version"),
            quoted(env!("CARGO_PKG_VERSION")),
        ]),
        list(vec![
            sym("general"),
            list(vec![sym("thickness"), num(1.6)]),
            list(vec![sym("legacy_teardrops"), sym("no")]),
        ]),
        list(vec![sym("paper"), quoted("A4")]),
        pcb_layers(),
        list(vec![
            sym("setup"),
            list(vec![sym("pad_to_mask_clearance"), num(0.0)]),
            list(vec![
                sym("allow_soldermask_bridges_in_footprints"),
                sym("no"),
            ]),
        ]),
        list(vec![sym("net"), sym("0"), quoted("")]),
    ];

    let net_codes = project
        .nets
        .keys()
        .enumerate()
        .map(|(i, name)| (name.clone(), (i + 1) as i32))
        .collect::<IndexMap<_, _>>();
    for (name, code) in &net_codes {
        root.push(list(vec![
            sym("net"),
            sym(code.to_string()),
            quoted(schematic_net_name(name)),
        ]));
    }
    for (reference, component) in &project.components {
        let Some(resolved) = libraries.components.get(reference) else {
            continue;
        };
        let Some(placement) = project.pcb.placement.get(reference) else {
            continue;
        };
        root.push(placed_footprint(
            project,
            reference,
            component.value.as_str(),
            resolved,
            placement,
            &net_codes,
        ));
    }

    let frame = BoardFrame::new(&project.pcb.outline);
    for (index, (a, b)) in project
        .pcb
        .outline
        .iter()
        .zip(project.pcb.outline.iter().cycle().skip(1))
        .take(project.pcb.outline.len())
        .enumerate()
    {
        root.push(gr_line(
            project,
            frame.map(*a),
            frame.map(*b),
            "Edge.Cuts",
            0.05,
            &format!("outline/{index}"),
        ));
    }
    for (net, routes) in &project.pcb.routes {
        let Some(code) = net_codes.get(net) else {
            continue;
        };
        for (route_index, route) in routes.iter().enumerate() {
            for (segment_index, segment) in route.path.windows(2).enumerate() {
                let mut items = vec![
                    sym("segment"),
                    point_list("start", frame.map(segment[0])),
                    point_list("end", frame.map(segment[1])),
                    list(vec![
                        sym("width"),
                        num(route.width.unwrap_or(project.rules.width(net))),
                    ]),
                    list(vec![sym("layer"), quoted(&route.layer)]),
                    list(vec![sym("net"), sym(code.to_string())]),
                    list(vec![
                        sym("uuid"),
                        quoted(
                            stable_uuid(
                                project,
                                &format!("pcb/route/{net}/{route_index}/{segment_index}"),
                            )
                            .to_string(),
                        ),
                    ]),
                ];
                if route.locked {
                    items.push(list(vec![sym("locked"), sym("yes")]));
                }
                root.push(list(items));
            }
        }
    }
    for (index, via) in project.pcb.vias.iter().enumerate() {
        let Some(code) = net_codes.get(&via.net) else {
            continue;
        };
        let mut items = vec![
            sym("via"),
            point_list("at", frame.map(via.at)),
            list(vec![
                sym("size"),
                num(via.size.unwrap_or(project.rules.via_size)),
            ]),
            list(vec![
                sym("drill"),
                num(via.drill.unwrap_or(project.rules.via_drill)),
            ]),
            list(vec![sym("layers"), quoted("F.Cu"), quoted("B.Cu")]),
            list(vec![sym("net"), sym(code.to_string())]),
            list(vec![
                sym("uuid"),
                quoted(stable_uuid(project, &format!("pcb/via/{index}")).to_string()),
            ]),
        ];
        if via.locked {
            items.push(list(vec![sym("locked"), sym("yes")]));
        }
        root.push(list(items));
    }
    for (index, zone) in project.pcb.zones.iter().enumerate() {
        let Some(code) = net_codes.get(&zone.net) else {
            continue;
        };
        let mut pts = vec![sym("pts")];
        pts.extend(zone.outline.iter().map(|point| xy(frame.map(*point))));
        root.push(list(vec![
            sym("zone"),
            list(vec![sym("net"), sym(code.to_string())]),
            list(vec![sym("net_name"), quoted(schematic_net_name(&zone.net))]),
            list(vec![sym("layer"), quoted(&zone.layer)]),
            list(vec![
                sym("uuid"),
                quoted(stable_uuid(project, &format!("pcb/zone/{index}")).to_string()),
            ]),
            list(vec![sym("hatch"), sym("edge"), num(0.5)]),
            list(vec![
                sym("connect_pads"),
                list(vec![
                    sym("clearance"),
                    num(zone.clearance.unwrap_or(project.rules.clearance)),
                ]),
            ]),
            list(vec![sym("min_thickness"), num(0.25)]),
            list(vec![
                sym("fill"),
                sym("yes"),
                list(vec![sym("thermal_gap"), num(0.3)]),
                list(vec![sym("thermal_bridge_width"), num(0.3)]),
            ]),
            list(vec![sym("polygon"), list(pts)]),
        ]));
    }
    for (index, hole) in project.pcb.holes.iter().enumerate() {
        let at = frame.map(hole.at);
        root.push(list(vec![
            sym("footprint"),
            quoted("MountingHole"),
            list(vec![sym("layer"), quoted("F.Cu")]),
            list(vec![
                sym("uuid"),
                quoted(stable_uuid(project, &format!("pcb/hole/{index}")).to_string()),
            ]),
            point_list("at", at),
            footprint_property_node(
                project,
                &format!("pcb/hole/{index}/property/Reference"),
                "Reference",
                "",
                "F.SilkS",
                false,
            ),
            footprint_property_node(
                project,
                &format!("pcb/hole/{index}/property/Value"),
                "Value",
                "",
                "F.Fab",
                false,
            ),
            footprint_property_node(
                project,
                &format!("pcb/hole/{index}/property/Datasheet"),
                "Datasheet",
                "",
                "F.Fab",
                true,
            ),
            footprint_property_node(
                project,
                &format!("pcb/hole/{index}/property/Description"),
                "Description",
                "",
                "F.Fab",
                true,
            ),
            list(vec![
                sym("attr"),
                sym("board_only"),
                sym("exclude_from_pos_files"),
                sym("exclude_from_bom"),
            ]),
            list(vec![
                sym("pad"),
                quoted(""),
                sym("np_thru_hole"),
                sym("circle"),
                list(vec![sym("at"), num(0.0), num(0.0)]),
                list(vec![sym("size"), num(hole.diameter), num(hole.diameter)]),
                list(vec![sym("drill"), num(hole.diameter)]),
                list(vec![sym("layers"), quoted("*.Cu"), quoted("*.Mask")]),
                list(vec![
                    sym("uuid"),
                    quoted(stable_uuid(project, &format!("pcb/hole/{index}/pad")).to_string()),
                ]),
            ]),
        ]));
    }
    for (index, text) in project.pcb.silk.iter().enumerate() {
        let at = frame.map(text.at);
        root.push(list(vec![
            sym("gr_text"),
            quoted(&text.text),
            at3(at[0], at[1], -text.rotation),
            list(vec![sym("layer"), quoted(&text.layer)]),
            list(vec![
                sym("uuid"),
                quoted(stable_uuid(project, &format!("pcb/silk/{index}")).to_string()),
            ]),
            effects(false),
        ]));
    }
    list(root)
}

fn placed_footprint(
    project: &ResolvedProject,
    reference: &str,
    value: &str,
    resolved: &ResolvedComponent,
    placement: &crate::model::PcbPlacement,
    net_codes: &IndexMap<String, i32>,
) -> Node {
    let frame = BoardFrame::new(&project.pcb.outline);
    let mut node = resolved.footprint_node.clone();
    set_second_quoted(&mut node, &resolved.footprint_id);
    let Node::List { items, .. } = &mut node else {
        return node;
    };
    items.retain(
        |item| !matches!(item, Node::Atom { atom: Atom::Symbol(value), .. } if value == "locked"),
    );
    remove_children(items, &["locked"]);
    if placement.locked {
        items.insert(2.min(items.len()), list(vec![sym("locked"), sym("yes")]));
    }
    remove_children(
        items,
        &[
            "version",
            "generator",
            "generator_version",
            "embedded_fonts",
        ],
    );
    upsert_child(
        items,
        list(vec![
            sym("layer"),
            quoted(match placement.side {
                BoardSide::Front => "F.Cu",
                BoardSide::Back => "B.Cu",
            }),
        ]),
    );
    upsert_child(
        items,
        list(vec![
            sym("uuid"),
            quoted(stable_uuid(project, &format!("pcb/component/{reference}")).to_string()),
        ]),
    );
    let at = frame.map(placement.at);
    upsert_child(items, at3(at[0], at[1], -placement.rotation));
    set_property(items, "Reference", &project.components[reference].reference);
    set_property(
        items,
        "Value",
        if value.is_empty() {
            project.components[reference].reference.as_str()
        } else {
            value
        },
    );
    ensure_footprint_property(items, "Datasheet", "");
    ensure_footprint_property(items, "Description", "");
    let endpoint_nets = endpoint_net_map(project);
    let mut ordinal = 0usize;
    for child in items.iter_mut().skip(1) {
        add_descendant_uuids(
            project,
            child,
            &format!("pcb/component/{reference}"),
            &mut ordinal,
        );
        let pad_number = (node_head(child) == Some("pad"))
            .then(|| node_second(child).map(str::to_owned))
            .flatten();
        if let Some(number) = pad_number
            && let Some(net_name) = endpoint_nets.get(&format!("{reference}.{number}"))
            && let Some(code) = net_codes.get(*net_name)
        {
            let Node::List {
                items: pad_items, ..
            } = child
            else {
                continue;
            };
            upsert_child(
                pad_items,
                list(vec![
                    sym("net"),
                    sym(code.to_string()),
                    quoted(schematic_net_name(net_name)),
                ]),
            );
        }
        if placement.side == BoardSide::Back && !matches!(node_head(child), Some("at" | "layer")) {
            mirror_footprint_geometry(child);
            swap_front_back_layers(child);
        }
    }
    node
}

fn project_json(project: &ResolvedProject) -> String {
    let rules = &project.rules;
    let mut output = json!({
        "board": {
            "design_settings": {
                "defaults": { "board_outline_line_width": 0.05, "copper_line_width": rules.preferred_track_width },
                "drc_exclusions": [],
                "meta": { "version": 2 },
                "rules": {
                    "allow_blind_buried_vias": false,
                    "allow_microvias": false,
                    "min_clearance": rules.clearance,
                    "min_track_width": rules.minimum_track_width,
                    "min_via_diameter": rules.via_size
                },
                "track_widths": [0.0, rules.preferred_track_width],
                "via_dimensions": [{"diameter": 0.0, "drill": 0.0}, {"diameter": rules.via_size, "drill": rules.via_drill}]
            }
        },
        "boards": [],
        "cvpcb": { "equivalence_files": [] },
        "erc": { "erc_exclusions": [], "meta": { "version": 0 } },
        "libraries": { "pinned_footprint_libs": [], "pinned_symbol_libs": [] },
        "meta": { "filename": format!("{}.kicad_pro", project.project.name), "version": 3 },
        "net_settings": {
            "classes": [{
                "bus_width": 12, "clearance": rules.clearance, "diff_pair_gap": 0.25,
                "diff_pair_via_gap": 0.25, "diff_pair_width": rules.preferred_track_width,
                "line_style": 0, "microvia_diameter": 0.3, "microvia_drill": 0.1,
                "name": "Default", "pcb_color": "rgba(0, 0, 0, 0.000)",
                "priority": 2147483647, "schematic_color": "rgba(0, 0, 0, 0.000)",
                "track_width": rules.preferred_track_width, "via_diameter": rules.via_size,
                "via_drill": rules.via_drill, "wire_width": 6
            }],
            "meta": { "version": 4 }, "net_colors": null,
            "netclass_assignments": null, "netclass_patterns": []
        },
        "pcbnew": {},
        "schematic": { "meta": { "version": 1 } },
        "sheets": [[stable_uuid(project, "schematic/root").to_string(), "Root"]],
        "text_variables": {}
    });
    for (name, class) in &rules.net_classes {
        let mut value = output["net_settings"]["classes"][0].clone();
        value["name"] = json!(name);
        value["clearance"] = json!(class.clearance);
        value["track_width"] = json!(class.preferred_track_width);
        output["net_settings"]["classes"]
            .as_array_mut()
            .unwrap()
            .push(value);
    }
    let assignments: serde_json::Map<String, serde_json::Value> = rules
        .net_classes
        .iter()
        .flat_map(|(name, class)| {
            class
                .nets
                .iter()
                .map(move |net| (schematic_net_name(net), json!([name])))
        })
        .collect();
    output["net_settings"]["netclass_assignments"] = json!(assignments);
    serde_json::to_string_pretty(&output).expect("JSON serialization cannot fail") + "\n"
}

fn design_rules(project: &ResolvedProject) -> String {
    let mut out = String::from("(version 1)\n");
    for (name, class) in &project.rules.net_classes {
        out.push_str(&format!("(rule \"{name}-width\" (condition \"A.NetClass == '{name}'\") (constraint track_width (min {})))\n",class.minimum_track_width));
        for layer in ["F.Cu", "B.Cu"] {
            if !class.allowed_layers.iter().any(|l| l == layer) {
                out.push_str(&format!("(rule \"{name}-{layer}\" (condition \"A.NetClass == '{name}'\") (layer \"{layer}\") (constraint disallow track via zone))\n"));
            }
        }
    }
    out
}

fn endpoint_position(
    project: &ResolvedProject,
    libraries: &ResolvedLibraries,
    endpoint: &str,
) -> Option<Point> {
    let (reference, query) = parse_endpoint(endpoint)?;
    let resolved = libraries.components.get(reference)?;
    let pin = resolved
        .pins
        .iter()
        .find(|pin| pin.number == query || pin.name.as_deref() == Some(query))?;
    let placement = project
        .schematic
        .placement
        .values()
        .find(|p| p.part == reference && (pin.unit == 0 || p.unit == pin.unit))?;
    let local = [pin.at[0], -pin.at[1]];
    let angle = (-placement.rotation).to_radians();
    let rotated = [
        angle.cos() * local[0] - angle.sin() * local[1],
        angle.sin() * local[0] + angle.cos() * local[1],
    ];
    Some([placement.at[0] + rotated[0], placement.at[1] + rotated[1]])
}

fn endpoint_net_map(project: &ResolvedProject) -> BTreeMap<String, &str> {
    let mut map = BTreeMap::new();
    for (net, endpoints) in &project.nets {
        for endpoint in endpoints {
            map.insert(endpoint.clone(), net.as_str());
        }
    }
    map
}

fn schematic_net_name(name: &str) -> String {
    if name.starts_with('/') {
        name.to_string()
    } else {
        format!("/{name}")
    }
}

fn unique_pins(pins: &[ResolvedPin]) -> Vec<&ResolvedPin> {
    let mut by_number = BTreeMap::new();
    for pin in pins {
        by_number.entry(pin.number.as_str()).or_insert(pin);
    }
    by_number.into_values().collect()
}

fn property_node(
    project: &ResolvedProject,
    reference: &str,
    key: &str,
    value: &str,
    at: Point,
    index: usize,
    hide: bool,
) -> Node {
    let items = vec![
        sym("property"),
        quoted(key),
        quoted(value),
        at3(at[0], at[1], 0.0),
        effects(hide),
    ];
    let _ = (project, reference, index);
    list(items)
}

fn label_node(project: &ResolvedProject, net: &str, at: Point, rotation: f64, key: &str) -> Node {
    list(vec![
        sym("label"),
        quoted(net),
        at3(at[0], at[1], rotation),
        effects(false),
        list(vec![
            sym("uuid"),
            quoted(stable_uuid(project, &format!("schematic/label/{net}/{key}")).to_string()),
        ]),
    ])
}

fn effects(hide: bool) -> Node {
    let mut items = vec![
        sym("effects"),
        list(vec![
            sym("font"),
            list(vec![sym("size"), num(1.27), num(1.27)]),
        ]),
    ];
    if hide {
        items.push(list(vec![sym("hide"), sym("yes")]));
    }
    list(items)
}

fn stroke(width: f64) -> Node {
    list(vec![
        sym("stroke"),
        list(vec![sym("width"), num(width)]),
        list(vec![sym("type"), sym("default")]),
    ])
}

fn pcb_layers() -> Node {
    list(vec![
        sym("layers"),
        list(vec![sym("0"), quoted("F.Cu"), sym("signal")]),
        list(vec![sym("2"), quoted("B.Cu"), sym("signal")]),
        list(vec![
            sym("9"),
            quoted("F.Adhes"),
            sym("user"),
            quoted("F.Adhesive"),
        ]),
        list(vec![
            sym("11"),
            quoted("B.Adhes"),
            sym("user"),
            quoted("B.Adhesive"),
        ]),
        list(vec![sym("13"), quoted("F.Paste"), sym("user")]),
        list(vec![sym("15"), quoted("B.Paste"), sym("user")]),
        list(vec![
            sym("5"),
            quoted("F.SilkS"),
            sym("user"),
            quoted("F.Silkscreen"),
        ]),
        list(vec![
            sym("7"),
            quoted("B.SilkS"),
            sym("user"),
            quoted("B.Silkscreen"),
        ]),
        list(vec![sym("1"), quoted("F.Mask"), sym("user")]),
        list(vec![sym("3"), quoted("B.Mask"), sym("user")]),
        list(vec![
            sym("17"),
            quoted("Dwgs.User"),
            sym("user"),
            quoted("User.Drawings"),
        ]),
        list(vec![
            sym("19"),
            quoted("Cmts.User"),
            sym("user"),
            quoted("User.Comments"),
        ]),
        list(vec![sym("25"), quoted("Edge.Cuts"), sym("user")]),
        list(vec![sym("27"), quoted("Margin"), sym("user")]),
        list(vec![
            sym("31"),
            quoted("F.CrtYd"),
            sym("user"),
            quoted("F.Courtyard"),
        ]),
        list(vec![
            sym("29"),
            quoted("B.CrtYd"),
            sym("user"),
            quoted("B.Courtyard"),
        ]),
        list(vec![sym("35"), quoted("F.Fab"), sym("user")]),
        list(vec![sym("33"), quoted("B.Fab"), sym("user")]),
    ])
}

fn gr_line(
    project: &ResolvedProject,
    start: Point,
    end: Point,
    layer: &str,
    width: f64,
    key: &str,
) -> Node {
    list(vec![
        sym("gr_line"),
        point_list("start", start),
        point_list("end", end),
        stroke(width),
        list(vec![sym("layer"), quoted(layer)]),
        list(vec![
            sym("uuid"),
            quoted(stable_uuid(project, &format!("pcb/{key}")).to_string()),
        ]),
    ])
}

#[derive(Debug, Clone, Copy)]
struct BoardFrame {
    min_x: f64,
    max_y: f64,
}
impl BoardFrame {
    fn new(outline: &[Point]) -> Self {
        Self {
            min_x: outline.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
            max_y: outline
                .iter()
                .map(|p| p[1])
                .fold(f64::NEG_INFINITY, f64::max),
        }
    }
    fn map(self, p: Point) -> Point {
        [20.0 + p[0] - self.min_x, 20.0 + self.max_y - p[1]]
    }
}

fn stable_uuid(project: &ResolvedProject, key: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("kil:{}:{key}", project.project.name).as_bytes(),
    )
}

fn add_descendant_uuids(
    project: &ResolvedProject,
    node: &mut Node,
    prefix: &str,
    ordinal: &mut usize,
) {
    let eligible = matches!(
        node_head(node),
        Some(
            "fp_line"
                | "fp_rect"
                | "fp_circle"
                | "fp_arc"
                | "fp_poly"
                | "fp_curve"
                | "fp_text"
                | "fp_text_box"
                | "point"
                | "pad"
                | "zone"
                | "group"
        )
    ) || is_named_property(node);
    if eligible && let Node::List { items, .. } = node {
        let key = format!("{prefix}/item/{}", *ordinal);
        *ordinal += 1;
        upsert_child(
            items,
            list(vec![
                sym("uuid"),
                quoted(stable_uuid(project, &key).to_string()),
            ]),
        );
    }
    if let Node::List { items, .. } = node {
        for child in items.iter_mut().skip(1) {
            add_descendant_uuids(project, child, prefix, ordinal);
        }
    }
}

fn is_named_property(node: &Node) -> bool {
    matches!(
        node,
        Node::List { items, .. }
            if matches!(
                items.as_slice(),
                [
                    Node::Atom { atom: Atom::Symbol(head), .. },
                    Node::Atom { atom: Atom::Quoted(_), .. },
                    ..
                ] if head == "property"
            )
    )
}

fn mirror_footprint_geometry(node: &mut Node) {
    if let Node::List { items, .. } = node {
        let head = items.first().and_then(|n| match n {
            Node::Atom {
                atom: Atom::Symbol(s),
                ..
            } => Some(s.as_str()),
            _ => None,
        });
        if matches!(head, Some("at" | "xy" | "start" | "end" | "mid" | "center")) {
            if let Some(Node::Atom {
                atom: Atom::Symbol(x),
                ..
            }) = items.get_mut(1)
                && let Ok(value) = x.parse::<f64>()
            {
                *x = (-value).to_string();
            }
            if let Some(Node::Atom {
                atom: Atom::Symbol(angle),
                ..
            }) = items.get_mut(3)
                && let Ok(value) = angle.parse::<f64>()
            {
                *angle = (-value).to_string();
            }
        }
        for child in items.iter_mut().skip(1) {
            mirror_footprint_geometry(child);
        }
    }
}

fn swap_front_back_layers(node: &mut Node) {
    match node {
        Node::Atom {
            atom: Atom::Quoted(value),
            ..
        } => {
            let swapped = match value.as_str() {
                "F.Cu" => Some("B.Cu"),
                "B.Cu" => Some("F.Cu"),
                "F.SilkS" => Some("B.SilkS"),
                "B.SilkS" => Some("F.SilkS"),
                "F.Mask" => Some("B.Mask"),
                "B.Mask" => Some("F.Mask"),
                "F.Paste" => Some("B.Paste"),
                "B.Paste" => Some("F.Paste"),
                "F.CrtYd" => Some("B.CrtYd"),
                "B.CrtYd" => Some("F.CrtYd"),
                "F.Fab" => Some("B.Fab"),
                "B.Fab" => Some("F.Fab"),
                _ => None,
            };
            if let Some(next) = swapped {
                *value = next.into();
            }
        }
        Node::List { items, .. } => {
            for child in items {
                mirror_footprint_geometry(child);
                swap_front_back_layers(child);
            }
        }
        _ => {}
    }
}

fn set_property(items: &mut [Node], key: &str, value: &str) {
    for node in items {
        if node_head(node) == Some("property")
            && node_second(node) == Some(key)
            && let Node::List { items, .. } = node
            && items.len() > 2
        {
            items[2] = quoted(value);
        }
    }
}

fn ensure_footprint_property(items: &mut Vec<Node>, key: &str, value: &str) {
    if items
        .iter()
        .any(|node| node_head(node) == Some("property") && node_second(node) == Some(key))
    {
        return;
    }
    items.push(list(vec![
        sym("property"),
        quoted(key),
        quoted(value),
        at3(0.0, 0.0, 0.0),
        list(vec![sym("layer"), quoted("F.Fab")]),
        list(vec![sym("hide"), sym("yes")]),
        effects(false),
    ]));
}

fn footprint_property_node(
    project: &ResolvedProject,
    uuid_path: &str,
    key: &str,
    value: &str,
    layer: &str,
    hidden: bool,
) -> Node {
    let mut items = vec![
        sym("property"),
        quoted(key),
        quoted(value),
        at3(0.0, 0.0, 0.0),
        list(vec![sym("layer"), quoted(layer)]),
    ];
    if hidden {
        items.push(list(vec![sym("hide"), sym("yes")]));
    }
    items.push(list(vec![
        sym("uuid"),
        quoted(stable_uuid(project, uuid_path).to_string()),
    ]));
    items.push(effects(false));
    list(items)
}

fn remove_children(items: &mut Vec<Node>, heads: &[&str]) {
    items.retain(|node| !node_head(node).is_some_and(|head| heads.contains(&head)));
}
fn upsert_child(items: &mut Vec<Node>, node: Node) {
    let Some(head) = node_head(&node) else {
        items.push(node);
        return;
    };
    if let Some(existing) = items
        .iter_mut()
        .skip(1)
        .find(|item| node_head(item) == Some(head))
    {
        *existing = node;
    } else {
        items.push(node);
    }
}
fn set_second_quoted(node: &mut Node, value: &str) {
    if let Node::List { items, .. } = node
        && items.len() > 1
    {
        items[1] = quoted(value);
    }
}
fn node_head(node: &Node) -> Option<&str> {
    match node {
        Node::List { items, .. } => match items.first() {
            Some(Node::Atom {
                atom: Atom::Symbol(v),
                ..
            }) => Some(v),
            _ => None,
        },
        _ => None,
    }
}
fn node_second(node: &Node) -> Option<&str> {
    match node {
        Node::List { items, .. } => match items.get(1) {
            Some(Node::Atom {
                atom: Atom::Quoted(v) | Atom::Symbol(v),
                ..
            }) => Some(v),
            _ => None,
        },
        _ => None,
    }
}

fn list(items: Vec<Node>) -> Node {
    Node::List { items, span: ZERO }
}
fn sym(value: impl Into<String>) -> Node {
    Node::Atom {
        atom: Atom::Symbol(value.into()),
        span: ZERO,
    }
}
fn quoted(value: impl Into<String>) -> Node {
    Node::Atom {
        atom: Atom::Quoted(value.into()),
        span: ZERO,
    }
}
fn num(value: f64) -> Node {
    let mut text = format!("{value:.6}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.push('0');
    }
    sym(text)
}
fn point_atoms(point: Point) -> Vec<Node> {
    vec![num(point[0]), num(point[1])]
}
fn point_list(head: &str, point: Point) -> Node {
    let mut items = vec![sym(head)];
    items.extend(point_atoms(point));
    list(items)
}
fn xy(point: Point) -> Node {
    list(vec![sym("xy"), num(point[0]), num(point[1])])
}
fn at2(point: Point) -> Node {
    list(vec![sym("at"), num(point[0]), num(point[1])])
}
fn at3(x: f64, y: f64, rotation: f64) -> Node {
    list(vec![sym("at"), num(x), num(y), num(rotation)])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library::LibraryResolver;
    use std::time::{Duration, Instant};

    #[test]
    fn pad_property_markers_do_not_receive_uuids() {
        let project: ResolvedProject =
            serde_json::from_str(include_str!("../testdata/resolved/two-resistors.kil.json"))
                .unwrap();
        let mut marker = list(vec![sym("property"), sym("pad_prop_mechanical")]);
        let mut ordinal = 0;

        add_descendant_uuids(&project, &mut marker, "pcb/component/J1", &mut ordinal);

        let Node::List { items, .. } = marker else {
            unreachable!();
        };
        assert_eq!(items.len(), 2);
        assert_eq!(ordinal, 0);
    }

    pub(crate) fn fixture() -> (ResolvedProject, ResolvedLibraries) {
        let project: ResolvedProject =
            serde_json::from_str(include_str!("../testdata/resolved/two-resistors.kil.json"))
                .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.kicad_sym"),
            include_str!("../testdata/test.kicad_sym"),
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("test.pretty")).unwrap();
        std::fs::write(
            dir.path().join("test.pretty/R.kicad_mod"),
            include_str!("../testdata/R.kicad_mod"),
        )
        .unwrap();
        std::fs::write(dir.path().join("sym-lib-table"), r#"(sym_lib_table (lib (name "Device") (type "KiCad") (uri "${KIPRJMOD}/test.kicad_sym") (options "") (descr "")))"#).unwrap();
        std::fs::write(dir.path().join("fp-lib-table"), r#"(fp_lib_table (lib (name "Resistor_SMD") (type "KiCad") (uri "${KIPRJMOD}/test.pretty") (options "") (descr "")))"#).unwrap();
        std::fs::copy(
            dir.path().join("test.pretty/R.kicad_mod"),
            dir.path().join("test.pretty/R_0603_1608Metric.kicad_mod"),
        )
        .unwrap();
        let (libraries, diagnostics) = LibraryResolver::discover(dir.path(), None).resolve_all(
            &project,
            &dir.path().join("test.json"),
            "",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        (project, libraries)
    }
    #[test]
    fn generation_is_deterministic_and_parses_without_system_kicad() {
        let (project, libraries) = fixture();
        let a = generate(&project, &libraries);
        let b = generate(&project, &libraries);
        assert_eq!(a.schematic, b.schematic);
        assert_eq!(a.pcb, b.pcb);
        let dir = tempfile::tempdir().unwrap();
        a.write_to(dir.path(), &project.project.name).unwrap();
        validate_generated(dir.path(), &project.project.name).unwrap();
        let mut renamed = project.clone();
        renamed.components.get_mut("R1").unwrap().reference = "R99".into();
        assert_eq!(
            stable_uuid(&project, "pcb/component/R1"),
            stable_uuid(&renamed, "pcb/component/R1")
        );
        assert!(generate(&renamed, &libraries).pcb.contains("R99"));
    }

    #[test]
    fn fifty_part_fixture_meets_size_and_compile_budget() {
        let (base, base_libraries) = fixture();
        let template = base.components.get("R1").unwrap().clone();
        let sch_template = base.schematic.placement.get("R1").unwrap().clone();
        let pcb_template = base.pcb.placement.get("R1").unwrap().clone();
        let mut project = base;
        project.project.name = "fifty-parts".into();
        project.components.clear();
        project.nets.clear();
        project.schematic.placement.clear();
        project.schematic.labels.clear();
        project.pcb.placement.clear();
        project.pcb.routes.clear();
        project.pcb.vias.clear();
        project.pcb.zones.clear();
        project.pcb.holes.clear();
        project.pcb.silk.clear();
        project.pcb.outline = vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        let mut pin_ones = Vec::new();
        let mut pin_twos = Vec::new();
        for index in 0..50 {
            let reference = format!("R{}", index + 1);
            let x = 5.0 + (index % 10) as f64 * 9.0;
            let y = 5.0 + (index / 10) as f64 * 12.0;
            project
                .components
                .insert(reference.clone(), template.clone());
            let mut sch = sch_template.clone();
            sch.at = [20.0 + x, 20.0 + y];
            sch.part = reference.clone();
            project.schematic.placement.insert(reference.clone(), sch);
            let mut pcb = pcb_template.clone();
            pcb.at = [x, y];
            project.pcb.placement.insert(reference.clone(), pcb);
            pin_ones.push(format!("{reference}.1"));
            pin_twos.push(format!("{reference}.2"));
        }
        project.nets.insert("A".into(), pin_ones);
        project.nets.insert("B".into(), pin_twos);
        let il = serde_json::to_string(&project).unwrap();
        let libraries = ResolvedLibraries {
            components: project
                .components
                .keys()
                .map(|id| (id.clone(), base_libraries.components["R1"].clone()))
                .collect(),
        };
        let started = Instant::now();
        let generated = generate(&project, &libraries);
        let elapsed = started.elapsed();
        let native_bytes = generated.schematic.len() + generated.pcb.len();
        assert!(
            il.len() * 4 <= native_bytes,
            "IL={} bytes, native={} bytes",
            il.len(),
            native_bytes
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "generation took {elapsed:?}"
        );
    }
}
