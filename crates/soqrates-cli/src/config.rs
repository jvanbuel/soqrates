//! Configuration: `$XDG_CONFIG_HOME/soqrates/config.toml` (per user), overlaid
//! by `.soqrates.toml` in the repository root (per project).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use soqrates_core::{Identity, Mode};

/// Everything is optional so that files can be layered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// e.g. `anthropic:claude-sonnet-5-5` (ADR-0001 D3).
    pub model: Option<String>,
    /// `advisory` or `blocking` (ADR-0001 D8, D9).
    pub mode: Option<ModeName>,
    pub max_questions: Option<usize>,
    pub pass_threshold: Option<f64>,
    /// Where the local learner store lives.
    pub state_dir: Option<PathBuf>,
    /// Forge host -> the identity to use there (ADR-0003 D7).
    #[serde(default)]
    pub identities: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ModeName {
    Advisory,
    Blocking,
}

impl From<ModeName> for Mode {
    fn from(m: ModeName) -> Mode {
        match m {
            ModeName::Advisory => Mode::Advisory,
            ModeName::Blocking => Mode::Blocking,
        }
    }
}

impl File {
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `self` with every field set in `over` replaced.
    pub fn overlay(mut self, over: File) -> Self {
        self.model = over.model.or(self.model);
        self.mode = over.mode.or(self.mode);
        self.max_questions = over.max_questions.or(self.max_questions);
        self.pass_threshold = over.pass_threshold.or(self.pass_threshold);
        self.state_dir = over.state_dir.or(self.state_dir);
        self.identities.extend(over.identities);
        self
    }

    /// The configured identities, parsed (ADR-0003 D7).
    pub fn parsed_identities(&self) -> Result<BTreeMap<String, Identity>, String> {
        self.identities
            .iter()
            .map(|(host, id)| {
                Identity::parse(id)
                    .map(|i| (host.clone(), i))
                    .map_err(|e| format!("identity for {host}: {e}"))
            })
            .collect()
    }
}

pub fn user_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("soqrates")
        .join("config.toml")
}

pub fn default_state_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("soqrates")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database URL may carry a password, so no config file, which may be
    /// committed, can hold one (ADR-0005 D5).
    // @adr-0005-d5
    #[test]
    fn config_files_cannot_hold_a_database_url() {
        let err = toml::from_str::<File>(r#"database_url = "postgres://u:pw@h/db""#).unwrap_err();
        assert!(err.to_string().contains("database_url"), "{err}");
    }

    #[test]
    fn project_file_overrides_user_file() {
        let user: File = toml::from_str(
            r#"
            model = "anthropic:claude-sonnet-5-5"
            mode = "advisory"
            [identities]
            "github.com" = "github.com:1001"
            "#,
        )
        .unwrap();
        let project: File = toml::from_str(
            r#"
            mode = "blocking"
            [identities]
            "gitlab.client.example" = "gitlab.client.example:678"
            "#,
        )
        .unwrap();
        let merged = user.overlay(project);
        assert_eq!(merged.mode, Some(ModeName::Blocking));
        assert_eq!(merged.model.as_deref(), Some("anthropic:claude-sonnet-5-5"));
        assert_eq!(merged.parsed_identities().unwrap().len(), 2);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<File>("modle = \"x\"").is_err());
    }
}
