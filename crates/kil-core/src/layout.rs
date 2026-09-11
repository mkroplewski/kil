use crate::{diagnostic::Diagnostic, library::ResolvedLibraries, model::*, source::*};
use indexmap::IndexMap;
use kiutils_sexpr::{Atom, Node};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Debug, Clone)]
pub struct LayoutPlan {
    pub design: PcbDesign,
    pub prefix: String,
    pub nets: IndexMap<String, String>,
    pub transform: Transform,
    pub file: PathBuf,
}
fn id(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.into()
    } else {
        format!("{prefix}/{local}")
    }
}
fn rotate(p: Point, degrees: f64) -> Point {
    let (s, c) = degrees.to_radians().sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}
fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
fn transform(p: Point, t: Transform) -> Point {
    add(t.at, rotate(p, t.rotation))
}

struct Context<'a> {
    project: &'a ResolvedProject,
    libraries: &'a ResolvedLibraries,
    pending: IndexMap<String, (&'a PlacementIntent, &'a LayoutPlan)>,
    placed: IndexMap<String, PcbPlacement>,
    visiting: BTreeSet<String>,
}
impl Context<'_> {
    fn placement(&mut self, key: &str) -> Result<PcbPlacement, String> {
        if let Some(p) = self.placed.get(key) {
            return Ok(p.clone());
        }
        let &(intent, plan) = self
            .pending
            .get(key)
            .ok_or_else(|| format!("missing placement '{key}'"))?;
        if !self.visiting.insert(key.into()) {
            return Err(format!("placement dependency cycle at '{key}'"));
        }
        let point = self.anchor(&intent.at, plan);
        self.visiting.remove(key);
        let result = PcbPlacement {
            at: point?,
            rotation: plan.transform.rotation + intent.rotation,
            side: intent.side,
            locked: intent.mode == PlacementMode::Fixed,
        };
        self.placed.insert(key.into(), result.clone());
        Ok(result)
    }
    fn anchor(&mut self, anchor: &Anchor, plan: &LayoutPlan) -> Result<Point, String> {
        match anchor {
            Anchor::Point(p) => Ok(transform(*p, plan.transform)),
            Anchor::Relative(a) => {
                let p = self.placement(&id(&plan.prefix, &a.part))?;
                Ok(add(p.at, rotate(a.offset, p.rotation)))
            }
            Anchor::Pad(a) => {
                let (local, terminal) = a
                    .pad
                    .rsplit_once('.')
                    .ok_or_else(|| format!("invalid pad '{}', expected part.terminal", a.pad))?;
                let key = id(&plan.prefix, local);
                let placement = self.placement(&key)?;
                let part = self
                    .project
                    .components
                    .get(&key)
                    .ok_or_else(|| format!("unknown part '{key}'"))?;
                let terminal = part
                    .aliases
                    .get(terminal)
                    .map(String::as_str)
                    .unwrap_or(terminal);
                let lib =
                    self.libraries.components.get(&key).ok_or_else(|| {
                        format!("pad anchor requires resolved library for '{key}'")
                    })?;
                let mut positions = pad_positions(&lib.footprint_node, terminal);
                positions.dedup();
                if positions.len() != 1 {
                    return Err(format!(
                        "pad '{}.{terminal}' must have exactly one anchor position",
                        key
                    ));
                }
                let mut local = [positions[0][0], -positions[0][1]];
                if placement.side == BoardSide::Back {
                    local[0] = -local[0];
                }
                Ok(add(
                    placement.at,
                    rotate(add(local, a.offset), placement.rotation),
                ))
            }
            Anchor::Edge(a) => {
                let outline = &self.project.pcb.outline;
                if a.edge >= outline.len() || !(0.0..=1.0).contains(&a.fraction) {
                    return Err("invalid board edge index or fraction".into());
                }
                let p = outline[a.edge];
                let q = outline[(a.edge + 1) % outline.len()];
                // Board edge anchors are in the root board frame, including in a module.
                Ok(add(
                    [
                        p[0] + (q[0] - p[0]) * a.fraction,
                        p[1] + (q[1] - p[1]) * a.fraction,
                    ],
                    a.offset,
                ))
            }
        }
    }
}
fn atom(n: &Node) -> Option<&str> {
    match n {
        Node::Atom {
            atom: Atom::Symbol(v) | Atom::Quoted(v),
            ..
        } => Some(v),
        _ => None,
    }
}
fn pad_positions(node: &Node, number: &str) -> Vec<Point> {
    let Node::List { items, .. } = node else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|n| {
            let Node::List { items, .. } = n else {
                return None;
            };
            if items.first().and_then(atom) != Some("pad")
                || items.get(1).and_then(atom) != Some(number)
            {
                return None;
            }
            items.iter().find_map(|n| {
                let Node::List { items, .. } = n else {
                    return None;
                };
                (items.first().and_then(atom) == Some("at"))
                    .then(|| {
                        Some([
                            items.get(1).and_then(atom)?.parse().ok()?,
                            items.get(2).and_then(atom)?.parse().ok()?,
                        ])
                    })
                    .flatten()
            })
        })
        .collect()
}

pub fn resolve_layout(
    project: &mut ResolvedProject,
    plans: &[LayoutPlan],
    libraries: &ResolvedLibraries,
    strict: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = vec![];
    let mut context = Context {
        project,
        libraries,
        pending: IndexMap::new(),
        placed: IndexMap::new(),
        visiting: BTreeSet::new(),
    };
    for plan in plans {
        for (local, p) in &plan.design.placement {
            let key = id(&plan.prefix, local);
            if context.pending.insert(key.clone(), (p, plan)).is_some() && strict {
                diagnostics.push(Diagnostic::error(
                    "LAYOUT001",
                    format!("duplicate placement '{key}'"),
                    &plan.file,
                ));
            }
        }
    }
    let keys: Vec<_> = context.pending.keys().cloned().collect();
    for key in keys {
        if let Err(e) = context.placement(&key)
            && strict
        {
            diagnostics.push(Diagnostic::error(
                "LAYOUT002",
                e,
                &context.pending[&key].1.file,
            ));
        }
    }
    let mut routes: IndexMap<String, Vec<Route>> = IndexMap::new();
    let mut vias = vec![];
    let mut zones = vec![];
    let mut holes = vec![];
    let mut silk = vec![];
    for plan in plans {
        let net = |n: &str| {
            plan.nets
                .get(n)
                .cloned()
                .unwrap_or_else(|| id(&plan.prefix, n))
        };
        for (n, rs) in &plan.design.routes {
            for route in rs {
                if strict {
                    for anchor in &route.path {
                        if let Anchor::Pad(a) = anchor
                            && let Some((part, pin)) = a.pad.rsplit_once('.')
                        {
                            let part = id(&plan.prefix, part);
                            let pin = project
                                .components
                                .get(&part)
                                .and_then(|p| p.aliases.get(pin))
                                .map(String::as_str)
                                .unwrap_or(pin);
                            let endpoint = format!("{part}.{pin}");
                            if !project
                                .nets
                                .get(&net(n))
                                .is_some_and(|e| e.contains(&endpoint))
                            {
                                diagnostics.push(Diagnostic::error(
                                    "LAYOUT005",
                                    format!(
                                        "route on '{}' anchors to unrelated terminal '{endpoint}'",
                                        net(n)
                                    ),
                                    &plan.file,
                                ));
                            }
                        }
                    }
                }
                let points: Result<Vec<_>, _> =
                    route.path.iter().map(|p| context.anchor(p, plan)).collect();
                match points {
                    Ok(path) => routes.entry(net(n)).or_default().push(Route {
                        layer: route.layer.clone(),
                        width: route.width,
                        path,
                        locked: route.locked,
                    }),
                    Err(e) if strict => {
                        diagnostics.push(Diagnostic::error("LAYOUT003", e, &plan.file))
                    }
                    _ => {}
                }
            }
        }
        for v in &plan.design.vias {
            let mut v = v.clone();
            v.net = net(&v.net);
            v.at = transform(v.at, plan.transform);
            vias.push(v);
        }
        for z in &plan.design.zones {
            let mut z = z.clone();
            z.net = net(&z.net);
            z.outline = z
                .outline
                .iter()
                .map(|p| transform(*p, plan.transform))
                .collect();
            zones.push(z);
        }
        for h in &plan.design.holes {
            let mut h = h.clone();
            h.at = transform(h.at, plan.transform);
            holes.push(h);
        }
        for s in &plan.design.silk {
            let mut s = s.clone();
            s.at = transform(s.at, plan.transform);
            s.rotation += plan.transform.rotation;
            silk.push(s);
        }
        if strict {
            for (name, c) in &plan.design.constraints {
                let result = (|| {
                    if c.max <= 0.0 {
                        return Err("maximum distance must be positive".into());
                    }
                    let a = context.anchor(&c.from, plan)?;
                    let b = context.anchor(&c.to, plan)?;
                    let d = (a[0] - b[0]).hypot(a[1] - b[1]);
                    if d > c.max + 1e-6 {
                        Err(format!("distance {d:.6} mm exceeds {} mm", c.max))
                    } else {
                        Ok(())
                    }
                })();
                if let Err(e) = result {
                    let message = format!("constraint '{}': {e}", id(&plan.prefix, name));
                    diagnostics.push(if c.preferred {
                        Diagnostic::warning("LAYOUT004", message, &plan.file)
                    } else {
                        Diagnostic::error("LAYOUT004", message, &plan.file)
                    });
                }
            }
        }
    }
    let placed = context.placed;
    project.pcb.placement = placed;
    project.pcb.routes = routes;
    project.pcb.vias = vias;
    project.pcb.zones = zones;
    project.pcb.holes = holes;
    project.pcb.silk = silk;
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (ResolvedProject, LayoutPlan, ResolvedLibraries) {
        let (project, libraries) = crate::kicad::tests::fixture();
        let source: Project =
            serde_json::from_str(include_str!("../../../examples/two-resistors.kil.json")).unwrap();
        (
            project,
            LayoutPlan {
                design: source.pcb,
                prefix: String::new(),
                nets: IndexMap::new(),
                transform: Transform::default(),
                file: "fixture.json".into(),
            },
            libraries,
        )
    }
    #[test]
    fn pad_routes_follow_placement_and_changed_library_geometry() {
        let (mut project, mut plan, mut libraries) = setup();
        plan.design.routes.clear();
        plan.design.routes.insert(
            "SIGNAL".into(),
            vec![RouteIntent {
                layer: "F.Cu".into(),
                width: None,
                locked: true,
                path: vec![
                    Anchor::Pad(PadAnchor {
                        pad: "R1.1".into(),
                        offset: [0., 0.],
                    }),
                    Anchor::Pad(PadAnchor {
                        pad: "R2.1".into(),
                        offset: [0., 0.],
                    }),
                ],
            }],
        );
        assert!(resolve_layout(&mut project, &[plan.clone()], &libraries, true).is_empty());
        let x = project.pcb.routes["SIGNAL"][0].path[0][0];
        plan.design.placement.get_mut("R1").unwrap().at = Anchor::Point([7., 5.]);
        assert!(resolve_layout(&mut project, &[plan.clone()], &libraries, true).is_empty());
        assert_eq!(project.pcb.routes["SIGNAL"][0].path[0][0], x + 2.);
        // Change the resolved pad's actual geometry, not its library identifier.
        let Node::List { items, .. } =
            &mut libraries.components.get_mut("R1").unwrap().footprint_node
        else {
            panic!()
        };
        for node in items {
            let Node::List { items, .. } = node else {
                continue;
            };
            if items.first().and_then(atom) == Some("pad")
                && items.get(1).and_then(atom) == Some("1")
            {
                for node in items {
                    let Node::List { items, .. } = node else {
                        continue;
                    };
                    if items.first().and_then(atom) == Some("at") {
                        let Node::Atom { atom, .. } = &mut items[1] else {
                            panic!()
                        };
                        *atom = Atom::Symbol("-1.825".into());
                    }
                }
            }
        }
        assert!(resolve_layout(&mut project, &[plan], &libraries, true).is_empty());
        assert_eq!(project.pcb.routes["SIGNAL"][0].path[0][0], x + 1.);
    }
    #[test]
    fn relative_placement_rotates_offsets_and_rejects_cycles() {
        let (mut project, mut plan, libraries) = setup();
        plan.design.placement["R1"].rotation = 90.;
        plan.design.placement["R2"].at = Anchor::Relative(RelativeAnchor {
            part: "R1".into(),
            offset: [2., 0.],
        });
        assert!(resolve_layout(&mut project, &[plan.clone()], &libraries, true).is_empty());
        assert_eq!(project.pcb.placement["R2"].at, [5., 7.]);
        plan.design.placement["R1"].at = Anchor::Relative(RelativeAnchor {
            part: "R2".into(),
            offset: [0., 0.],
        });
        assert!(
            resolve_layout(&mut project, &[plan], &libraries, true)
                .iter()
                .any(|d| d.code == "LAYOUT002")
        );
    }
    #[test]
    fn duplicate_placement_is_reported_only_in_strict_pass() {
        let (mut project, plan, libraries) = setup();
        let plans = [plan.clone(), plan];
        assert!(
            !resolve_layout(&mut project, &plans, &libraries, false)
                .iter()
                .any(|d| d.code == "LAYOUT001")
        );
        assert_eq!(
            resolve_layout(&mut project, &plans, &libraries, true)
                .iter()
                .filter(|d| d.code == "LAYOUT001")
                .count(),
            plans[0].design.placement.len()
        );
    }

    #[test]
    fn distance_requirements_and_preferences_have_distinct_severity() {
        let (mut project, mut plan, libraries) = setup();
        plan.design.constraints.insert(
            "near".into(),
            DistanceConstraint {
                from: Anchor::Point([0., 0.]),
                to: Anchor::Point([3., 4.]),
                max: 4.,
                preferred: false,
            },
        );
        assert!(
            resolve_layout(&mut project, &[plan.clone()], &libraries, true)
                .iter()
                .any(|d| d.severity == crate::Severity::Error)
        );
        plan.design.constraints["near"].preferred = true;
        assert!(
            resolve_layout(&mut project, &[plan], &libraries, true)
                .iter()
                .any(|d| d.code == "LAYOUT004" && d.severity == crate::Severity::Warning)
        );
    }
}
