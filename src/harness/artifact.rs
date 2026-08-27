//! Snapshot controlado del artefacto Rust sobre el que trabajará el Agent.
//!
//! [`RustArtifact`] es el objeto de dominio RESULT / WORKING PRODUCT.
//! Contrato v2: árbol lógico multi-file ([`ArtifactPath`] → source) con
//! archivo **primary** para compatibilidad single-file (`source()` / `replace_source`).
//! Permanece separado de [`crate::state::CodeState`].

use std::collections::BTreeMap;

use crate::harness::artifact_path::ArtifactPath;
use crate::harness::specification::SpecificationId;

/// Versión del contrato de Artifact.
pub type ArtifactContractVersion = u32;

/// Versión activa: multi-file con primary compatible.
pub const ARTIFACT_CONTRACT_VERSION: ArtifactContractVersion = 2;

/// Identidad estable del Artifact (no deriva del contenido del source).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lenguaje del Artifact (extensible; hoy solo Rust).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactLanguage {
    Rust,
}

impl ArtifactLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
        }
    }
}

/// Vista de un archivo del Artifact (path + source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFile {
    pub path: ArtifactPath,
    pub source: String,
}

/// Artefacto Rust versionado: identidad estable + árbol de archivos revisable.
///
/// Fuente canónica: [`Self::files`]. El **primary** es el archivo que exponen
/// `source()` / `replace_source()` / `AgentContext::working_code()` (compat single-file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustArtifact {
    id: ArtifactId,
    /// Etiqueta de compatibilidad (p. ej. `"main.rs"`); no es el path materializado.
    name: String,
    primary: ArtifactPath,
    files: BTreeMap<ArtifactPath, String>,
    language: ArtifactLanguage,
    contract_version: ArtifactContractVersion,
    revision: u64,
    /// Enlace opcional Specification → Artifact (trazabilidad en memoria).
    specification_id: Option<SpecificationId>,
}

impl RustArtifact {
    /// Crea un artefacto single-file con `revision == 0`.
    ///
    /// El contenido vive en `src/main.rs` (primary). `name` se conserva como etiqueta.
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        let name = name.into();
        Self::with_id(ArtifactId::new(format!("artifact:{name}")), name, source)
    }

    /// Crea un artefacto single-file con identidad explícita (`src/main.rs` = primary).
    pub fn with_id(id: ArtifactId, name: impl Into<String>, source: impl Into<String>) -> Self {
        let primary = ArtifactPath::parse("src/main.rs").expect("path canónico src/main.rs");
        let mut files = BTreeMap::new();
        files.insert(primary.clone(), source.into());
        Self {
            id,
            name: name.into(),
            primary,
            files,
            language: ArtifactLanguage::Rust,
            contract_version: ARTIFACT_CONTRACT_VERSION,
            revision: 0,
            specification_id: None,
        }
    }

    /// Construye un Artifact multi-file. `primary` debe existir en `files`.
    pub fn try_from_files(
        id: ArtifactId,
        name: impl Into<String>,
        primary: ArtifactPath,
        files: impl IntoIterator<Item = (ArtifactPath, String)>,
    ) -> Result<Self, String> {
        let files: BTreeMap<ArtifactPath, String> = files.into_iter().collect();
        if files.is_empty() {
            return Err("RustArtifact requiere al menos un archivo".to_string());
        }
        if !files.contains_key(&primary) {
            return Err(format!("primary `{}` no está en files", primary.as_str()));
        }
        Ok(Self {
            id,
            name: name.into(),
            primary,
            files,
            language: ArtifactLanguage::Rust,
            contract_version: ARTIFACT_CONTRACT_VERSION,
            revision: 0,
            specification_id: None,
        })
    }

    pub fn id(&self) -> &ArtifactId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Contenido del archivo **primary** (compat single-file).
    pub fn source(&self) -> &str {
        self.files
            .get(&self.primary)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn primary_path(&self) -> &ArtifactPath {
        &self.primary
    }

    pub fn primary_source(&self) -> &str {
        self.source()
    }

    pub fn file(&self, path: &ArtifactPath) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    pub fn files(&self) -> impl Iterator<Item = (&ArtifactPath, &str)> {
        self.files
            .iter()
            .map(|(path, source)| (path, source.as_str()))
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn language(&self) -> ArtifactLanguage {
        self.language
    }

    pub fn contract_version(&self) -> ArtifactContractVersion {
        self.contract_version
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn specification_id(&self) -> Option<&SpecificationId> {
        self.specification_id.as_ref()
    }

    pub fn with_specification_id(mut self, specification_id: SpecificationId) -> Self {
        self.specification_id = Some(specification_id);
        self
    }

    /// Reemplaza el source del **primary** e incrementa `revision` solo si cambia.
    /// Conserva el resto de archivos. No modifica [`ArtifactId`].
    pub fn replace_source(&mut self, new_source: impl Into<String>) {
        let next = new_source.into();
        let current = self
            .files
            .get(&self.primary)
            .map(String::as_str)
            .unwrap_or("");
        if next != current {
            self.files.insert(self.primary.clone(), next);
            self.revision += 1;
        }
    }

    /// Inserta o actualiza un archivo. Incrementa `revision` solo si el contenido cambia
    /// o el path es nuevo.
    pub fn upsert_file(
        &mut self,
        path: ArtifactPath,
        source: impl Into<String>,
    ) -> Result<(), String> {
        let next = source.into();
        match self.files.get(&path) {
            Some(prev) if prev == &next => Ok(()),
            _ => {
                self.files.insert(path, next);
                self.revision += 1;
                Ok(())
            }
        }
    }

    /// Elimina un archivo no-primary. Incrementa `revision` si existía.
    pub fn remove_file(&mut self, path: &ArtifactPath) -> Result<(), String> {
        if path == &self.primary {
            return Err(format!(
                "no se puede eliminar el archivo primary `{}`",
                path.as_str()
            ));
        }
        if self.files.remove(path).is_some() {
            self.revision += 1;
            Ok(())
        } else {
            Err(format!("archivo inexistente: {}", path.as_str()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder;
    use crate::planner;
    use crate::state::CodeState;

    fn build_valid_api_state() -> CodeState {
        let mut state = CodeState {
            request: "Crear una API REST".to_string(),
            plan: None,
            code: None,
            errors: Vec::new(),
            feedback: Vec::new(),
            iteration: 0,
        };
        planner::plan(&mut state);
        state.iteration = 2;
        builder::build(&mut state);
        state
    }

    #[test]
    fn artifact_starts_at_revision_zero() {
        let artifact = RustArtifact::new("main.rs", "fn main() {}");
        assert_eq!(artifact.revision(), 0);
        assert_eq!(artifact.name(), "main.rs");
        assert_eq!(artifact.language(), ArtifactLanguage::Rust);
        assert_eq!(artifact.contract_version(), ARTIFACT_CONTRACT_VERSION);
        assert_eq!(artifact.primary_path().as_str(), "src/main.rs");
        assert_eq!(artifact.file_count(), 1);
    }

    #[test]
    fn artifact_returns_source() {
        let artifact = RustArtifact::new("main.rs", "fn main() {}");
        assert_eq!(artifact.source(), "fn main() {}");
        assert_eq!(artifact.revision(), 0);
    }

    #[test]
    fn artifact_has_stable_identity() {
        // A
        let artifact =
            RustArtifact::with_id(ArtifactId::new("art-stable"), "main.rs", "fn main() {}");
        assert_eq!(artifact.id().as_str(), "art-stable");
    }

    #[test]
    fn changing_source_does_not_change_artifact_id() {
        // B
        let mut artifact =
            RustArtifact::with_id(ArtifactId::new("art-stable"), "main.rs", "fn main() {}");
        let before = artifact.id().clone();
        artifact.replace_source("fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.id(), &before);
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn artifact_replace_source_updates_source_and_revision() {
        let mut artifact = RustArtifact::new("main.rs", "fn main() {}");
        artifact.replace_source("fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.source(), "fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn artifact_multiple_replacements_increment_revision() {
        let mut artifact = RustArtifact::new("main.rs", "fn main() {}");
        assert_eq!(artifact.revision(), 0);

        artifact.replace_source("fn main() { println!(\"v1\"); }");
        assert_eq!(artifact.revision(), 1);

        artifact.replace_source("fn main() { println!(\"v2\"); }");
        assert_eq!(artifact.revision(), 2);

        artifact.replace_source("fn main() { println!(\"v2\"); }");
        assert_eq!(artifact.revision(), 2);
    }

    #[test]
    fn artifact_links_optional_specification_id() {
        let artifact = RustArtifact::new("main.rs", "fn main() {}")
            .with_specification_id(SpecificationId::new("spec-1"));
        assert_eq!(
            artifact.specification_id().map(|id| id.as_str()),
            Some("spec-1")
        );
    }

    #[test]
    fn artifact_snapshot_does_not_mutate_original_state() {
        let state = build_valid_api_state();
        let original_code = state.code.clone().expect("código del Constructor");
        let original_iteration = state.iteration;

        let mut artifact = RustArtifact::new("main.rs", original_code.clone());

        assert_eq!(artifact.revision(), 0);
        assert_eq!(artifact.source(), original_code);

        artifact.replace_source("fn main() { println!(\"harness-owned\"); }");

        assert_eq!(state.code.as_deref(), Some(original_code.as_str()));
        assert_eq!(state.iteration, original_iteration);
        assert_ne!(artifact.source(), original_code);
        assert_eq!(artifact.revision(), 1);
    }

    #[test]
    fn replace_source_preserves_sibling_files() {
        let main = ArtifactPath::parse("src/main.rs").unwrap();
        let lib = ArtifactPath::parse("src/lib.rs").unwrap();
        let mut artifact = RustArtifact::try_from_files(
            ArtifactId::new("art-multi"),
            "main.rs",
            main,
            [
                (
                    ArtifactPath::parse("src/main.rs").unwrap(),
                    "fn main() { helper::run(); }".to_string(),
                ),
                (
                    ArtifactPath::parse("src/lib.rs").unwrap(),
                    "pub fn run() {}".to_string(),
                ),
            ],
        )
        .unwrap();
        artifact.replace_source("fn main() { helper::run(); println!(\"x\"); }");
        assert_eq!(artifact.revision(), 1);
        assert_eq!(
            artifact.file(&lib).unwrap(),
            "pub fn run() {}",
            "lib.rs no debe destruirse al corregir primary"
        );
        assert_eq!(artifact.file_count(), 2);
    }

    #[test]
    fn upsert_and_remove_preserve_structure() {
        let mut artifact = RustArtifact::new("main.rs", "fn main() {}");
        let helper = ArtifactPath::parse("src/helper.rs").unwrap();
        artifact
            .upsert_file(helper.clone(), "pub fn x() {}".to_string())
            .unwrap();
        assert_eq!(artifact.revision(), 1);
        assert_eq!(artifact.file(&helper), Some("pub fn x() {}"));
        artifact.remove_file(&helper).unwrap();
        assert_eq!(artifact.revision(), 2);
        assert!(artifact.file(&helper).is_none());
        let primary = artifact.primary_path().clone();
        assert!(
            artifact
                .remove_file(&primary)
                .unwrap_err()
                .contains("primary")
        );
    }
}
