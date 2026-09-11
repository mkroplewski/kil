//! Content lock for the exact resolved symbols and footprints used by a design.
use crate::{diagnostic::Diagnostic, library::ResolvedLibraries};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryLock {
    pub format_version: u32,
    pub assets: BTreeMap<String, String>,
}
pub fn snapshot(libraries: &ResolvedLibraries) -> LibraryLock {
    let mut assets = BTreeMap::new();
    for component in libraries.components.values() {
        for (key, node) in [
            (
                format!("symbol:{}", component.symbol_id),
                &component.symbol_node,
            ),
            (
                format!("footprint:{}", component.footprint_id),
                &component.footprint_node,
            ),
        ] {
            let text = kiutils_sexpr::CstDocument {
                raw: String::new(),
                nodes: vec![node.clone()],
            }
            .to_canonical_string();
            assets.insert(key, format!("{:x}", Sha256::digest(text.as_bytes())));
        }
    }
    LibraryLock {
        format_version: 2,
        assets,
    }
}
pub fn path(input: &Path) -> PathBuf {
    input.with_extension("lock.json")
}
pub fn fingerprint(lock: &LibraryLock) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(lock).expect("serializable lock"))
    )
}
pub fn verify(input: &Path, libraries: &ResolvedLibraries) -> Result<String, Box<Diagnostic>> {
    let path = path(input);
    let expected = snapshot(libraries);
    let actual = fs::read_to_string(&path).map_err(|e| {
        Diagnostic::error("LOCK001", format!("library lock unavailable: {e}"), &path)
            .with_help(format!("run 'kil lock {}'", input.display()))
    })?;
    let actual: LibraryLock = serde_json::from_str(&actual)
        .map_err(|e| Diagnostic::error("LOCK002", e.to_string(), &path))?;
    if actual != expected {
        return Err(Diagnostic::error(
            "LOCK003",
            "resolved library contents differ from the lock",
            &path,
        )
        .with_help("review library changes, then run 'kil lock FILE' to accept them")
        .into());
    }
    Ok(fingerprint(&expected))
}
pub fn write(input: &Path, libraries: &ResolvedLibraries) -> Result<PathBuf, String> {
    let path = path(input);
    let text = serde_json::to_string_pretty(&snapshot(libraries)).map_err(|e| e.to_string())?;
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    use std::io::Write;
    writeln!(file, "{text}").map_err(|e| e.to_string())?;
    file.persist(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_resolved_geometry_requires_explicit_acceptance() {
        let (_, mut libraries) = crate::kicad::tests::fixture();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("test.kil.json");
        assert_eq!(verify(&input, &libraries).unwrap_err().code, "LOCK001");
        write(&input, &libraries).unwrap();
        let original = verify(&input, &libraries).unwrap();
        libraries.components.get_mut("R1").unwrap().footprint_id = "Changed:R".into();
        assert_eq!(verify(&input, &libraries).unwrap_err().code, "LOCK003");
        write(&input, &libraries).unwrap();
        assert_ne!(original, verify(&input, &libraries).unwrap());
    }
}
