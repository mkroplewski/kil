//! Verify electrical intent against KiCad's exported connectivity, not merely readability.
use crate::model::ResolvedProject;
use kiutils_sexpr::{Atom, Node};
use std::collections::{BTreeMap, BTreeSet};
fn atom(node: &Node) -> Option<&str> {
    match node {
        Node::Atom {
            atom: Atom::Quoted(s) | Atom::Symbol(s),
            ..
        } => Some(s),
        _ => None,
    }
}
fn children<'a>(node: &'a Node, head: &'a str) -> impl Iterator<Item = &'a Node> {
    let items = match node {
        Node::List { items, .. } => items.as_slice(),
        _ => &[],
    };
    items.iter().filter(
        move |n| matches!(n,Node::List{items,..} if items.first().and_then(atom)==Some(head)),
    )
}
fn field<'a>(node: &'a Node, head: &str) -> Option<&'a str> {
    let Node::List { items, .. } = node else {
        return None;
    };
    items.iter().find_map(|n| {
        let Node::List { items, .. } = n else {
            return None;
        };
        if items.first().and_then(atom) == Some(head) {
            items.get(1).and_then(atom)
        } else {
            None
        }
    })
}
pub fn verify(project: &ResolvedProject, netlist: &str) -> Result<(), String> {
    let doc = kiutils_sexpr::parse_one(netlist).map_err(|e| e.to_string())?;
    let root = doc.nodes.first().ok_or("empty netlist")?;
    let nets = children(root, "nets")
        .next()
        .ok_or("netlist has no nets section")?;
    let references: BTreeMap<_, _> = project
        .components
        .iter()
        .map(|(id, c)| (c.reference.as_str(), id.as_str()))
        .collect();
    let mut actual: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for net in children(nets, "net") {
        let name = field(net, "name")
            .ok_or("net has no name")?
            .trim_start_matches('/')
            .to_owned();
        let mut endpoints = BTreeSet::new();
        for node in children(net, "node") {
            let reference = field(node, "ref").ok_or("netlist node has no ref")?;
            let pin = field(node, "pin").ok_or("netlist node has no pin")?;
            let id = references
                .get(reference)
                .ok_or_else(|| format!("unexpected component '{reference}' in netlist"))?;
            let endpoint = format!("{id}.{pin}");
            if !seen.insert(endpoint.clone()) {
                return Err(format!("terminal '{endpoint}' appears in multiple nets"));
            }
            endpoints.insert(endpoint);
        }
        if actual.insert(name.clone(), endpoints).is_some() {
            return Err(format!("duplicate exported net '{name}'"));
        }
    }
    for (name, endpoints) in &project.nets {
        let expected: BTreeSet<_> = endpoints.iter().cloned().collect();
        // KiCad escapes slashes inside net labels to distinguish them from sheet paths.
        let exported_name = name.replace('/', "{slash}");
        let exported = actual.remove(&exported_name).unwrap_or_default();
        if expected != exported {
            return Err(format!(
                "net '{name}' differs from circuit intent: expected {expected:?}, exported {exported:?}"
            ));
        }
    }
    // KiCad names isolated pins automatically. They must never acquire additional connections.
    for (name, endpoints) in actual {
        if endpoints.len() > 1 {
            return Err(format!(
                "unexpected electrical connection '{name}': {endpoints:?}"
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn module_private_nets_match_kicad_escaped_names() {
        let (mut project, _) = crate::kicad::tests::fixture();
        project.nets.clear();
        project.nets.insert(
            "channel/internal".into(),
            vec!["R1.1".into(), "R2.1".into()],
        );
        let netlist = r#"(export (nets (net (name "channel{slash}internal") (node (ref "R1") (pin "1")) (node (ref "R2") (pin "1")))))"#;
        assert!(verify(&project, netlist).is_ok());
        assert!(
            verify(
                &project,
                &netlist.replace("channel{slash}internal", "other")
            )
            .is_err()
        );
    }
    #[test]
    fn detects_missing_endpoints_and_unintended_geometric_shorts() {
        let (mut project, _) = crate::kicad::tests::fixture();
        project.nets.clear();
        project
            .nets
            .insert("N".into(), vec!["R1.1".into(), "R2.1".into()]);
        let good = r#"(export (nets (net (name "/N") (node (ref "R1") (pin "1")) (node (ref "R2") (pin "1")))))"#;
        assert!(verify(&project, good).is_ok());
        assert!(
            verify(
                &project,
                &good.replace("(node (ref \"R2\") (pin \"1\"))", "")
            )
            .is_err()
        );
        assert!(
            verify(
                &project,
                &good.replace(
                    "(pin \"1\")))))",
                    "(pin \"1\")) (node (ref \"R2\") (pin \"2\")))))"
                )
            )
            .is_err()
        );
        assert!(verify(&project, "{}").is_err());
    }
}
