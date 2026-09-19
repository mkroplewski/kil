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
    pub sheets: IndexMap<String, String>,
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
        let sheets_dir = directory.join(format!("{name}.sheets"));
        if sheets_dir.exists() {
            fs::remove_dir_all(&sheets_dir)?;
        }
        fs::create_dir(&sheets_dir)?;
        let mut paths = vec![project_path, schematic_path, pcb_path];
        for (filename, text) in &self.sheets {
            let path = sheets_dir.join(filename);
            fs::write(&path, text)?;
            paths.push(path);
        }
        Ok(paths)
    }
}

/// Generate KiCad artifacts, rejecting names that cannot safely appear in rule expressions.
pub fn generate(
    project: &ResolvedProject,
    libraries: &ResolvedLibraries,
) -> std::io::Result<GeneratedProject> {
    project
        .pcb
        .stackup
        .validate()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    for name in project.rules.net_classes.keys() {
        if !crate::model::valid_net_class_name(name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid net-class name '{name}'"),
            ));
        }
    }
    if !project.power_sources.is_empty() && libraries.power_flag.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing power:PWR_FLAG library asset",
        ));
    }
    let root_schematic = schematic_node(project, libraries, "");
    let sheets = project
        .schematic
        .sheets
        .iter()
        .map(|sheet| {
            (
                sheet_filename(project, sheet),
                CstDocument {
                    raw: String::new(),
                    nodes: vec![schematic_node(project, libraries, sheet)],
                }
                .to_canonical_string(),
            )
        })
        .collect();
    let pcb_node = pcb_node(project, libraries);
    let schematic = CstDocument {
        raw: String::new(),
        nodes: vec![root_schematic],
    }
    .to_canonical_string();
    let pcb = CstDocument {
        raw: String::new(),
        nodes: vec![pcb_node],
    }
    .to_canonical_string();
    let project_json = project_json(project);
    Ok(GeneratedProject {
        sheets,
        design_rules: design_rules(project),
        project: project_json,
        schematic,
        pcb,
    })
}

pub fn validate_generated(directory: &Path, name: &str) -> Result<(), String> {
    let sch = directory.join(format!("{name}.kicad_sch"));
    let pcb = directory.join(format!("{name}.kicad_pcb"));
    let sch_doc = SchematicFile::read(&sch)
        .map_err(|err| format!("generated schematic is invalid: {err}"))?;
    let pcb_doc = PcbFile::read(&pcb).map_err(|err| format!("generated PCB is invalid: {err}"))?;
    if sch_doc.ast().symbol_count == 0
        && sch_doc.ast().sheet_count == 0
        && sch_doc.ast().lib_symbol_count > 0
    {
        return Err("generated schematic contains libraries but no placed symbols".into());
    }
    let sheets = directory.join(format!("{name}.sheets"));
    if sheets.exists() {
        for entry in fs::read_dir(&sheets).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            SchematicFile::read(&path)
                .map_err(|e| format!("invalid child schematic '{}': {e}", path.display()))?;
        }
    }
    if pcb_doc.ast().footprint_count == 0 && pcb_doc.ast().net_count > 1 {
        return Err("generated PCB contains nets but no footprints".into());
    }
    Ok(())
}

fn schematic_node(full: &ResolvedProject, libraries: &ResolvedLibraries, sheet: &str) -> Node {
    let mut page = full.clone();
    page.schematic.placement.retain(|_, p| p.sheet == sheet);
    page.schematic.wires.retain(|p| p.sheet == sheet);
    page.schematic.labels.retain(|p| p.sheet == sheet);
    let project = &page;
    let root_uuid = stable_uuid(project, "schematic/root");
    let page_uuid = if sheet.is_empty() {
        root_uuid
    } else {
        stable_uuid(project, &format!("schematic/file/{sheet}"))
    };
    let instance_path = if sheet.is_empty() {
        format!("/{root_uuid}")
    } else {
        format!("/{root_uuid}/{}", sheet_uuid(project, sheet))
    };
    let mut root = vec![
        sym("kicad_sch"),
        list(vec![sym("version"), sym("20250114")]),
        list(vec![sym("generator"), quoted("kil")]),
        list(vec![
            sym("generator_version"),
            quoted(env!("CARGO_PKG_VERSION")),
        ]),
        list(vec![sym("uuid"), quoted(page_uuid.to_string())]),
        list(vec![
            sym("paper"),
            quoted(if project.schematic.sheets.len() > 8 && sheet.is_empty() {
                "A3"
            } else {
                "A4"
            }),
        ]),
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
    if !project.power_sources.is_empty() {
        let mut flag = libraries
            .power_flag
            .clone()
            .expect("validated power flag library");
        set_second_quoted(&mut flag, "power:PWR_FLAG");
        embedded.push(flag);
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
                    quoted(&instance_path),
                    list(vec![sym("reference"), quoted(&component.reference)]),
                    list(vec![sym("unit"), sym(placement.unit.to_string())]),
                ]),
            ]),
        ]));
        root.push(list(items));
    }

    for (index, endpoint) in project.power_sources.iter().enumerate() {
        if let Some(at) = endpoint_position(project, libraries, endpoint) {
            root.push(power_flag(project, endpoint, at, index, &instance_path));
        }
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
                            &format!("schematic/wire/{sheet}/{wire_index}/{segment_index}"),
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
                    &format!("{sheet}/endpoint/{endpoint}"),
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
            &format!("{sheet}/explicit/{index}"),
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
                        stable_uuid(project, &format!("schematic/no-connect/{sheet}/{endpoint}"))
                            .to_string(),
                    ),
                ]),
            ]));
        }
    }

    if sheet.is_empty() {
        for (index, child) in project.schematic.sheets.iter().enumerate() {
            root.push(sheet_symbol(project, child, index));
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
            list(vec![sym("thickness"), num(project.pcb.stackup.thickness)]),
            list(vec![sym("legacy_teardrops"), sym("no")]),
        ]),
        list(vec![sym("paper"), quoted("A4")]),
        pcb_layers(&project.pcb.stackup),
        list(vec![
            sym("setup"),
            physical_stackup(&project.pcb.stackup),
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
            quoted(schematic_net_name(project, name)),
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
    for (index, area) in project.pcb.keepouts.iter().enumerate() {
        let layers = if area.layers.is_empty() {
            project.pcb.stackup.copper_layers()
        } else {
            area.layers.clone()
        };
        for layer in layers {
            let mut pts = vec![sym("pts")];
            pts.extend(area.outline.iter().map(|p| xy(frame.map(*p))));
            let mut restrictions = vec![sym("keepout")];
            for (item, excluded) in [
                ("tracks", area.tracks),
                ("vias", area.vias),
                ("pads", area.pads),
                ("copperpour", area.copper_pours),
                ("footprints", area.footprints),
            ] {
                restrictions.push(list(vec![
                    sym(item),
                    sym(if excluded { "not_allowed" } else { "allowed" }),
                ]));
            }
            root.push(list(vec![
                sym("zone"),
                list(vec![sym("net"), sym("0")]),
                list(vec![sym("net_name"), quoted("")]),
                list(vec![sym("layer"), quoted(&layer)]),
                list(vec![
                    sym("uuid"),
                    quoted(
                        stable_uuid(project, &format!("pcb/keepout/{index}/{layer}")).to_string(),
                    ),
                ]),
                list(vec![sym("hatch"), sym("edge"), num(0.5)]),
                list(restrictions),
                list(vec![sym("polygon"), list(pts)]),
            ]));
        }
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
            list(vec![
                sym("net_name"),
                quoted(schematic_net_name(project, &zone.net)),
            ]),
            list(vec![sym("layer"), quoted(&zone.layer)]),
            list(vec![
                sym("uuid"),
                quoted(stable_uuid(project, &format!("pcb/zone/{index}")).to_string()),
            ]),
            list(vec![sym("priority"), sym(zone.priority.to_string())]),
            list(vec![sym("hatch"), sym("edge"), num(0.5)]),
            zone_connection(zone, project.rules.clearance),
            list(vec![sym("min_thickness"), num(0.25)]),
            list(vec![
                sym("fill"),
                sym("yes"),
                list(vec![
                    sym("thermal_gap"),
                    num(zone.thermal_gap.unwrap_or(0.3)),
                ]),
                list(vec![
                    sym("thermal_bridge_width"),
                    num(zone.thermal_width.unwrap_or(0.3)),
                ]),
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
                    quoted(schematic_net_name(project, net_name)),
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
    let sheets: Vec<_> = std::iter::once(json!([
        stable_uuid(project, "schematic/root").to_string(),
        "Root"
    ]))
    .chain(
        project
            .schematic
            .sheets
            .iter()
            .map(|sheet| json!([sheet_uuid(project, sheet).to_string(), sheet])),
    )
    .collect();
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
        "sheets": sheets,
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
                .map(move |net| (schematic_net_name(project, net), json!([name])))
        })
        .collect();
    output["net_settings"]["netclass_assignments"] = json!(assignments);
    serde_json::to_string_pretty(&output).expect("JSON serialization cannot fail") + "\n"
}

fn design_rules(project: &ResolvedProject) -> String {
    let mut out = String::from("(version 1)\n");
    for (name, class) in &project.rules.net_classes {
        out.push_str(&format!("(rule \"{name}-width\" (condition \"A.NetClass == '{name}'\") (constraint track_width (min {})))\n",class.minimum_track_width));
        for layer in project.pcb.stackup.copper_layers() {
            let mut disallow = Vec::new();
            if !class.allows(&layer) {
                disallow.push("track");
            }
            if !class.allows_zone(&layer) {
                disallow.push("zone");
            }
            if !disallow.is_empty() {
                out.push_str(&format!("(rule \"{name}-{layer}\" (condition \"A.NetClass == '{name}'\") (layer \"{layer}\") (constraint disallow {}))\n", disallow.join(" ")));
            }
        }
        if !class.allows_vias() {
            out.push_str(&format!("(rule \"{name}-vias\" (condition \"A.NetClass == '{name}'\") (constraint disallow via))\n"));
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

fn schematic_net_name(project: &ResolvedProject, name: &str) -> String {
    if name.starts_with('/') || !project.schematic.sheets.is_empty() {
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
    let mut items = vec![
        sym(if project.schematic.sheets.is_empty() {
            "label"
        } else {
            "global_label"
        }),
        quoted(net),
        at3(at[0], at[1], rotation),
        effects(false),
        list(vec![
            sym("uuid"),
            quoted(stable_uuid(project, &format!("schematic/label/{net}/{key}")).to_string()),
        ]),
    ];
    if !project.schematic.sheets.is_empty() {
        items.push(list(vec![sym("shape"), sym("passive")]));
    }
    list(items)
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

fn pcb_layers(stackup: &crate::stackup::Stackup) -> Node {
    let mut items = vec![
        sym("layers"),
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
    ];
    for (index, layer) in stackup.copper_layers().iter().enumerate().rev() {
        let id = if layer == "F.Cu" {
            0
        } else if layer == "B.Cu" {
            2
        } else {
            2 * index + 2
        };
        items.insert(
            1,
            list(vec![sym(id.to_string()), quoted(layer), sym("signal")]),
        );
    }
    list(items)
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
                atom: Atom::Symbol(x) | Atom::Quoted(x),
                ..
            }) = items.get_mut(1)
                && let Ok(value) = x.parse::<f64>()
            {
                *x = (-value).to_string();
            }
            if let Some(Node::Atom {
                atom: Atom::Symbol(angle) | Atom::Quoted(angle),
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

fn zone_connection(zone: &crate::model::Zone, clearance: f64) -> Node {
    let mut items = vec![sym("connect_pads")];
    if zone.solid {
        items.push(sym("yes"));
    }
    items.push(list(vec![
        sym("clearance"),
        num(zone.clearance.unwrap_or(clearance)),
    ]));
    list(items)
}
fn physical_stackup(stackup: &crate::stackup::Stackup) -> Node {
    let mut items = vec![sym("stackup")];
    let layers = stackup.copper_layers();
    let copper_thicknesses = stackup.resolved_copper_thicknesses();
    let total_copper_thickness = copper_thicknesses.iter().sum::<f64>();
    for (index, layer) in layers.iter().enumerate() {
        items.push(list(vec![
            sym("layer"),
            quoted(layer),
            list(vec![sym("type"), quoted("copper")]),
            list(vec![sym("thickness"), num(copper_thicknesses[index])]),
        ]));
        if index + 1 < layers.len() {
            let dielectric = stackup.dielectrics.get(index);
            let thickness = dielectric.map(|d| d.thickness).unwrap_or(
                (stackup.thickness - total_copper_thickness) / f64::from(stackup.layers - 1),
            );
            items.push(list(vec![
                sym("layer"),
                quoted(format!("dielectric {}", index + 1)),
                list(vec![sym("type"), quoted("core")]),
                list(vec![sym("thickness"), num(thickness)]),
                list(vec![
                    sym("material"),
                    quoted(dielectric.map(|d| d.material.as_str()).unwrap_or("FR4")),
                ]),
                list(vec![
                    sym("epsilon_r"),
                    num(dielectric.map(|d| d.epsilon_r).unwrap_or(4.5)),
                ]),
            ]));
        }
    }
    list(items)
}

fn sheet_uuid(project: &ResolvedProject, sheet: &str) -> Uuid {
    stable_uuid(project, &format!("schematic/sheet/{sheet}"))
}
fn sheet_filename(project: &ResolvedProject, sheet: &str) -> String {
    format!("{}.kicad_sch", sheet_uuid(project, sheet))
}
fn sheet_symbol(project: &ResolvedProject, sheet: &str, index: usize) -> Node {
    let at = [
        20.0 + (index % 4) as f64 * 65.0,
        120.0 + (index / 4) as f64 * 25.0,
    ];
    let path = list(vec![
        sym("path"),
        quoted(format!("/{}", stable_uuid(project, "schematic/root"))),
        list(vec![sym("page"), quoted((index + 2).to_string())]),
    ]);
    let instances = list(vec![
        sym("instances"),
        list(vec![sym("project"), quoted(&project.project.name), path]),
    ]);
    list(vec![
        sym("sheet"),
        at2(at),
        list(vec![sym("size"), num(50.), num(15.)]),
        list(vec![
            sym("uuid"),
            quoted(sheet_uuid(project, sheet).to_string()),
        ]),
        list(vec![
            sym("property"),
            quoted("Sheetname"),
            quoted(sheet),
            at3(at[0], at[1] - 1.27, 0.),
            effects(false),
        ]),
        list(vec![
            sym("property"),
            quoted("Sheetfile"),
            quoted(format!(
                "{}.sheets/{}",
                project.project.name,
                sheet_filename(project, sheet)
            )),
            at3(at[0], at[1] + 16.27, 0.),
            effects(false),
        ]),
        instances,
    ])
}

fn power_flag(
    project: &ResolvedProject,
    endpoint: &str,
    at: Point,
    index: usize,
    path: &str,
) -> Node {
    let reference = format!("#FLG{}", index + 1);
    let uuid = stable_uuid(project, &format!("schematic/power/{endpoint}"));
    let instance = list(vec![
        sym("path"),
        quoted(path),
        list(vec![sym("reference"), quoted(&reference)]),
        list(vec![sym("unit"), sym("1")]),
    ]);
    list(vec![
        sym("symbol"),
        list(vec![sym("lib_id"), quoted("power:PWR_FLAG")]),
        at3(at[0], at[1], 0.),
        list(vec![sym("unit"), sym("1")]),
        list(vec![sym("in_bom"), sym("no")]),
        list(vec![sym("on_board"), sym("no")]),
        list(vec![sym("uuid"), quoted(uuid.to_string())]),
        list(vec![
            sym("property"),
            quoted("Reference"),
            quoted(&reference),
            at3(at[0], at[1], 0.0),
            effects(true),
        ]),
        list(vec![
            sym("property"),
            quoted("Value"),
            quoted("PWR_FLAG"),
            at3(at[0], at[1], 0.0),
            effects(true),
        ]),
        list(vec![
            sym("instances"),
            list(vec![
                sym("project"),
                quoted(&project.project.name),
                instance,
            ]),
        ]),
    ])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library::LibraryResolver;
    use std::time::{Duration, Instant};

    #[test]
    fn generation_rejects_unsafe_net_class_names() {
        let (mut project, libraries) = fixture();
        for name in ["sig\"fast", "sig'fast", "sig\\fast"] {
            project.rules.net_classes.clear();
            project.rules.net_classes.insert(
                name.into(),
                crate::model::NetClass {
                    nets: vec![],
                    clearance: 0.2,
                    minimum_track_width: 0.2,
                    preferred_track_width: 0.25,
                    allowed_layers: vec!["F.Cu".into()],
                    zone_layers: None,
                    allow_through_vias: None,
                },
            );
            assert_eq!(
                generate(&project, &libraries).unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn design_rules_allow_through_vias_between_allowed_outer_layers() {
        let (mut project, _) = fixture();
        project.pcb.stackup.layers = 4;
        project.rules.net_classes.insert(
            "signals".into(),
            crate::model::NetClass {
                nets: vec!["SIGNAL".into()],
                clearance: 0.2,
                minimum_track_width: 0.2,
                preferred_track_width: 0.25,
                allowed_layers: vec!["F.Cu".into(), "B.Cu".into()],
                zone_layers: None,
                allow_through_vias: None,
            },
        );

        let rules = design_rules(&project);
        assert!(rules.contains("(constraint disallow track zone)"));
        assert!(!rules.contains("(constraint disallow track via zone)"));
    }

    #[test]
    fn back_side_geometry_is_mirrored_once_including_quoted_numbers() {
        let mut pad = list(vec![
            sym("pad"),
            quoted("1"),
            list(vec![sym("at"), quoted("2"), num(3.), quoted("90")]),
            list(vec![sym("layers"), quoted("F.Cu"), quoted("F.Mask")]),
        ]);
        mirror_footprint_geometry(&mut pad);
        swap_front_back_layers(&mut pad);
        let expected = list(vec![
            sym("pad"),
            quoted("1"),
            list(vec![sym("at"), quoted("-2"), num(3.), quoted("-90")]),
            list(vec![sym("layers"), quoted("B.Cu"), quoted("B.Mask")]),
        ]);
        assert_eq!(pad, expected);
    }

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
        let a = generate(&project, &libraries).unwrap();
        let b = generate(&project, &libraries).unwrap();
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
        assert!(generate(&renamed, &libraries).unwrap().pcb.contains("R99"));
        renamed.components.get_mut("R1").unwrap().value.clear();
        assert!(
            generate(&renamed, &libraries)
                .unwrap()
                .schematic
                .contains("(property \"Value\" \"R99\"")
        );
    }

    #[test]
    fn pcb_net_names_follow_schematic_label_scope() {
        let (mut project, _) = fixture();

        assert_eq!(schematic_net_name(&project, "SIGNAL"), "/SIGNAL");
        assert_eq!(schematic_net_name(&project, "/PRIVATE"), "/PRIVATE");

        project.schematic.sheets.insert("channel/main".into());
        assert_eq!(schematic_net_name(&project, "SIGNAL"), "SIGNAL");
        assert_eq!(schematic_net_name(&project, "/PRIVATE"), "/PRIVATE");
    }

    #[test]
    fn keepout_restrictions_default_to_excluding_every_object_type() {
        let (mut project, libraries) = fixture();
        let mut keepout: crate::model::Keepout = serde_json::from_value(json!({
            "outline": [[1.0, 1.0], [2.0, 1.0], [2.0, 2.0]]
        }))
        .unwrap();

        assert!(keepout.tracks);
        assert!(keepout.vias);
        assert!(keepout.pads);
        assert!(keepout.copper_pours);
        assert!(keepout.footprints);
        let serialized = serde_json::to_value(&keepout).unwrap();
        for field in ["tracks", "vias", "pads", "copper_pours", "footprints"] {
            assert!(
                serialized.get(field).is_none(),
                "default restriction `{field}` changed serialized project data"
            );
        }

        project.pcb.keepouts = vec![keepout.clone()];
        let pcb = generate(&project, &libraries).unwrap().pcb;
        assert!(pcb.contains("(keepout (tracks not_allowed) (vias not_allowed) (pads not_allowed) (copperpour not_allowed) (footprints not_allowed))"));

        keepout.tracks = false;
        keepout.vias = false;
        keepout.pads = false;
        keepout.footprints = false;
        project.pcb.keepouts = vec![keepout];
        let pcb = generate(&project, &libraries).unwrap().pcb;
        assert!(pcb.contains("(keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed))"));
    }

    #[test]
    fn heterogeneous_copper_thicknesses_are_emitted_per_layer() {
        let (mut project, libraries) = fixture();
        project.pcb.stackup.layers = 4;
        project.pcb.stackup.copper_thicknesses = vec![0.035, 0.0175, 0.0175, 0.07];
        let pcb = generate(&project, &libraries).unwrap().pcb;

        for expected in [
            "(layer \"F.Cu\" (type \"copper\") (thickness 0.035))",
            "(layer \"In1.Cu\" (type \"copper\") (thickness 0.0175))",
            "(layer \"In2.Cu\" (type \"copper\") (thickness 0.0175))",
            "(layer \"B.Cu\" (type \"copper\") (thickness 0.07))",
        ] {
            assert!(
                pcb.contains(expected),
                "missing `{expected}` in generated PCB"
            );
        }
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
            power_flag: None,
            components: project
                .components
                .keys()
                .map(|id| (id.clone(), base_libraries.components["R1"].clone()))
                .collect(),
        };
        let started = Instant::now();
        let generated = generate(&project, &libraries).unwrap();
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
