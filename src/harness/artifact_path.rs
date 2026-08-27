//! Paths lógicos relativos dentro de un [`crate::harness::RustArtifact`].
//!
//! No representan el filesystem del host. Validan allowlist y rechazan traversal.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Path relativo validado bajo el árbol lógico del Artifact (`src/…`, `tests/…`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactPath {
    inner: String,
}

impl ArtifactPath {
    /// Parsea y valida un path relativo lógico.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, String> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err("ArtifactPath vacío".to_string());
        }
        if raw.starts_with('/') || raw.starts_with('\\') {
            return Err(format!("ArtifactPath absoluto rechazado: {raw}"));
        }
        // Windows-style absolute / drive
        if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
            return Err(format!("ArtifactPath absoluto rechazado: {raw}"));
        }
        if raw.contains('\\') {
            return Err(format!(
                "ArtifactPath con separador '\\' rechazado (usar '/'): {raw}"
            ));
        }
        if raw.contains('\0') {
            return Err("ArtifactPath con null byte rechazado".to_string());
        }

        let segments: Vec<&str> = raw.split('/').collect();
        if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
            return Err(format!("ArtifactPath con segmento vacío: {raw}"));
        }
        for segment in &segments {
            if *segment == "." || *segment == ".." {
                return Err(format!("ArtifactPath con traversal rechazado: {raw}"));
            }
            if !segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
            {
                return Err(format!("ArtifactPath con caracteres inválidos: {raw}"));
            }
        }

        let first = segments[0];
        if first != "src" && first != "tests" {
            return Err(format!(
                "ArtifactPath debe empezar con src/ o tests/: {raw}"
            ));
        }
        if segments.len() < 2 {
            return Err(format!("ArtifactPath incompleto (falta archivo): {raw}"));
        }

        Ok(Self {
            inner: raw.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Une este path bajo `root` sin permitir escape (solo componentes normales).
    pub fn resolve_under(&self, root: &Path) -> Result<PathBuf, String> {
        let mut out = root.to_path_buf();
        for segment in self.inner.split('/') {
            // Defensa en profundidad: aunque parse ya rechazó `..`.
            let component = Path::new(segment);
            for part in component.components() {
                match part {
                    Component::Normal(os) => out.push(os),
                    Component::CurDir => {}
                    Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                        return Err(format!(
                            "resolución de ArtifactPath escaparía del root: {}",
                            self.inner
                        ));
                    }
                }
            }
        }

        let root_canon = root;
        if !out.starts_with(root_canon) {
            return Err(format!(
                "path materializado fuera del temp root: {} (root={})",
                out.display(),
                root.display()
            ));
        }
        Ok(out)
    }
}

impl fmt::Display for ArtifactPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inner)
    }
}

impl AsRef<str> for ArtifactPath {
    fn as_ref(&self) -> &str {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_src_main_and_nested() {
        assert!(ArtifactPath::parse("src/main.rs").is_ok());
        assert!(ArtifactPath::parse("src/domain/math.rs").is_ok());
        assert!(ArtifactPath::parse("tests/integration.rs").is_ok());
    }

    #[test]
    fn rejects_absolute_and_traversal() {
        assert!(ArtifactPath::parse("/src/main.rs").is_err());
        assert!(ArtifactPath::parse("src/../etc/passwd").is_err());
        assert!(ArtifactPath::parse("..\\src\\main.rs").is_err());
        assert!(ArtifactPath::parse("").is_err());
        assert!(ArtifactPath::parse("main.rs").is_err());
    }

    #[test]
    fn resolve_under_stays_inside_root() {
        let root = PathBuf::from("/tmp/artifact_root_test");
        let path = ArtifactPath::parse("src/domain/math.rs").unwrap();
        let resolved = path.resolve_under(&root).unwrap();
        assert!(resolved.starts_with(&root));
        assert_eq!(resolved, root.join("src").join("domain").join("math.rs"));
    }
}
