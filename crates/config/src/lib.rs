use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use nix_search_core::ArtifactKind;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to load configuration: {0}")]
    Figment(Box<figment::Error>),

    #[error("invalid configuration: {0}")]
    Validation(String),
}

impl From<figment::Error> for ConfigError {
    fn from(error: figment::Error) -> Self {
        Self::Figment(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub data: DataConfig,
    pub server: ServerConfig,
    pub projects: BTreeMap<String, ProjectConfig>,
}

impl AppConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut figment = Figment::from(Serialized::defaults(Self::default()));

        if let Some(path) = path {
            figment = figment.merge(Toml::file(path));
        }

        figment = figment.merge(Env::prefixed("NIX_SEARCH_").split("__"));

        let config: Self = figment.extract()?;
        config.validate()?;

        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.data.validate()?;
        self.server.validate()?;

        for (project_key, project) in &self.projects {
            project.validate(project_key)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DataConfig {
    pub artifact_url: String,
    pub index_dir: PathBuf,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            artifact_url: "file://./data/artifacts".to_owned(),
            index_dir: PathBuf::from("./data/indexes"),
        }
    }
}

impl DataConfig {
    fn validate(&self) -> Result<()> {
        validate_non_empty("data.artifact_url", &self.artifact_url)?;

        if self.index_dir.as_os_str().is_empty() {
            return Err(ConfigError::Validation(
                "data.index_dir must not be empty".to_owned(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub listen: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:3000".to_owned(),
        }
    }
}

impl ServerConfig {
    fn validate(&self) -> Result<()> {
        validate_non_empty("server.listen", &self.listen)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub name: Option<String>,
    pub datasets: Vec<DatasetConfig>,
}

impl ProjectConfig {
    fn validate(&self, project_key: &str) -> Result<()> {
        validate_id("project key", project_key)?;

        for dataset in &self.datasets {
            dataset.validate(project_key)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: DatasetKind,
    #[serde(default)]
    pub refs: Vec<RefConfig>,
}

impl DatasetConfig {
    fn validate(&self, project_key: &str) -> Result<()> {
        validate_id("dataset id", &self.id)?;

        for ref_config in &self.refs {
            ref_config.validate(project_key, &self.id)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatasetKind {
    #[default]
    Options,
    Packages,
    Apps,
    Services,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefConfig {
    pub id: String,
    pub producer: ProducerConfig,
}

impl RefConfig {
    fn validate(&self, project_id: &str, dataset_id: &str) -> Result<()> {
        validate_id("ref id", &self.id)?;
        self.producer.validate(project_id, dataset_id, &self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProducerConfig {
    ExistingFile {
        path: PathBuf,
        artifact: ArtifactKind,
    },

    ChannelPackagesJson {
        channel: String,
        #[serde(default)]
        url: Option<String>,
    },

    NixBuildOptionsJson {
        #[serde(rename = "ref")]
        source_ref: String,
        attribute: String,
        import_path: String,
        output_path: String,
    },

    EvalModules {
        #[serde(rename = "ref")]
        source_ref: String,
        #[serde(default)]
        modules_attr: Option<String>,
        #[serde(default)]
        options_prefix: Option<String>,
        #[serde(default)]
        url_prefix: Option<String>,
    },

    Download {
        url: String,
        artifact: ArtifactKind,
    },

    CustomCommand {
        command: Vec<String>,
        artifact: ArtifactKind,
    },

    FlakeOutput {
        #[serde(rename = "ref")]
        source_ref: String,
    },
}

impl ProducerConfig {
    fn validate(&self, project_id: &str, dataset_id: &str, ref_id: &str) -> Result<()> {
        match self {
            Self::ExistingFile { path, .. } => {
                if path.as_os_str().is_empty() {
                    return producer_error(
                        project_id,
                        dataset_id,
                        ref_id,
                        "path must not be empty",
                    );
                }
            }

            Self::ChannelPackagesJson { channel, url } => {
                validate_producer_non_empty(project_id, dataset_id, ref_id, "channel", channel)?;

                if let Some(url) = url {
                    validate_producer_non_empty(project_id, dataset_id, ref_id, "url", url)?;
                }
            }

            Self::NixBuildOptionsJson {
                source_ref,
                attribute,
                import_path,
                output_path,
            } => {
                validate_producer_non_empty(project_id, dataset_id, ref_id, "ref", source_ref)?;
                validate_producer_non_empty(
                    project_id,
                    dataset_id,
                    ref_id,
                    "attribute",
                    attribute,
                )?;
                validate_producer_non_empty(
                    project_id,
                    dataset_id,
                    ref_id,
                    "import_path",
                    import_path,
                )?;
                validate_producer_non_empty(
                    project_id,
                    dataset_id,
                    ref_id,
                    "output_path",
                    output_path,
                )?;
            }

            Self::EvalModules {
                source_ref,
                modules_attr,
                options_prefix,
                url_prefix,
            } => {
                validate_producer_non_empty(project_id, dataset_id, ref_id, "ref", source_ref)?;

                if let Some(modules_attr) = modules_attr {
                    validate_producer_non_empty(
                        project_id,
                        dataset_id,
                        ref_id,
                        "modules_attr",
                        modules_attr,
                    )?;
                }

                if let Some(options_prefix) = options_prefix {
                    validate_producer_non_empty(
                        project_id,
                        dataset_id,
                        ref_id,
                        "options_prefix",
                        options_prefix,
                    )?;
                }

                if let Some(url_prefix) = url_prefix {
                    validate_producer_non_empty(
                        project_id,
                        dataset_id,
                        ref_id,
                        "url_prefix",
                        url_prefix,
                    )?;
                }
            }

            Self::Download { url, .. } => {
                validate_producer_non_empty(project_id, dataset_id, ref_id, "url", url)?;
            }

            Self::CustomCommand { command, .. } => {
                if command.is_empty() {
                    return producer_error(
                        project_id,
                        dataset_id,
                        ref_id,
                        "command must not be empty",
                    );
                }

                for item in command {
                    validate_producer_non_empty(
                        project_id,
                        dataset_id,
                        ref_id,
                        "command item",
                        item,
                    )?;
                }
            }

            Self::FlakeOutput { source_ref } => {
                validate_producer_non_empty(project_id, dataset_id, ref_id, "ref", source_ref)?;
            }
        }

        Ok(())
    }

    pub fn kind(&self) -> ProducerKind {
        match self {
            Self::ExistingFile { .. } => ProducerKind::ExistingFile,
            Self::ChannelPackagesJson { .. } => ProducerKind::ChannelPackagesJson,
            Self::NixBuildOptionsJson { .. } => ProducerKind::NixBuildOptionsJson,
            Self::EvalModules { .. } => ProducerKind::EvalModules,
            Self::Download { .. } => ProducerKind::Download,
            Self::CustomCommand { .. } => ProducerKind::CustomCommand,
            Self::FlakeOutput { .. } => ProducerKind::FlakeOutput,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProducerKind {
    ExistingFile,
    ChannelPackagesJson,
    NixBuildOptionsJson,
    EvalModules,
    Download,
    CustomCommand,
    FlakeOutput,
}

fn validate_non_empty(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ConfigError::Validation(format!("{name} must not be empty")));
    }

    Ok(())
}

fn validate_id(name: &str, value: &str) -> Result<()> {
    validate_non_empty(name, value)?;

    if value.contains('/') {
        return Err(ConfigError::Validation(format!(
            "{name} must not contain '/': {value:?}"
        )));
    }

    Ok(())
}

fn validate_producer_non_empty(
    project_id: &str,
    dataset_id: &str,
    ref_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    if value.trim().is_empty() {
        return producer_error(
            project_id,
            dataset_id,
            ref_id,
            &format!("{field} must not be empty"),
        );
    }

    Ok(())
}

fn producer_error<T>(project_id: &str, dataset_id: &str, ref_id: &str, message: &str) -> Result<T> {
    Err(ConfigError::Validation(format!(
        "projects.{project_id}.datasets.{dataset_id}.refs.{ref_id}: {message}"
    )))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use nix_search_core::ArtifactKind;

    use super::{AppConfig, DatasetKind, ProducerConfig, ProducerKind};

    #[test]
    fn default_config_is_valid() {
        let config = AppConfig::load(None).unwrap();

        assert_eq!(config.data.artifact_url, "file://./data/artifacts");
        assert_eq!(
            config.data.index_dir,
            std::path::PathBuf::from("./data/indexes")
        );
        assert_eq!(config.server.listen, "127.0.0.1:3000");
        assert!(config.projects.is_empty());
    }

    #[test]
    fn loads_config_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nix-search.toml");

        fs::write(
            &path,
            r#"
               [data]
               artifact_url = "file://./tmp/artifacts"
               index_dir = "./tmp/indexes"

               [server]
               listen = "0.0.0.0:8080"

               [projects.nixpkgs]
               name = "Nixpkgs"

               [[projects.nixpkgs.datasets]]
               id = "nixos-options"
               name = "NixOS Options"
               kind = "options"

               [[projects.nixpkgs.datasets.refs]]
               id = "unstable"

               [projects.nixpkgs.datasets.refs.producer]
               type = "nix-build-options-json"
               ref = "github:NixOS/nixpkgs/nixos-unstable"
               attribute = "options"
               import_path = "nixos/release.nix"
               output_path = "share/doc/nixos/options.json"

               [[projects.nixpkgs.datasets]]
               id = "packages"
               name = "Nix Packages"
               kind = "packages"

               [[projects.nixpkgs.datasets.refs]]
               id = "unstable"

               [projects.nixpkgs.datasets.refs.producer]
               type = "channel-packages-json"
               channel = "nixos-unstable"
               "#,
        )
        .unwrap();

        let config = AppConfig::load(Some(&path)).unwrap();

        assert_eq!(config.data.artifact_url, "file://./tmp/artifacts");
        assert_eq!(config.server.listen, "0.0.0.0:8080");

        let project = config.projects.get("nixpkgs").unwrap();
        assert_eq!(project.name.as_deref(), Some("Nixpkgs"));
        assert_eq!(project.datasets.len(), 2);

        let options = &project.datasets[0];
        assert_eq!(options.id, "nixos-options");
        assert_eq!(options.kind, DatasetKind::Options);
        assert_eq!(
            options.refs[0].producer.kind(),
            ProducerKind::NixBuildOptionsJson
        );

        let packages = &project.datasets[1];
        assert_eq!(packages.id, "packages");
        assert_eq!(packages.kind, DatasetKind::Packages);
        assert_eq!(
            packages.refs[0].producer.kind(),
            ProducerKind::ChannelPackagesJson
        );
    }

    #[test]
    fn loads_existing_file_producer() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nix-search.toml");

        fs::write(
            &path,
            r#"
               [projects.fixtures]
               name = "Fixtures"

               [[projects.fixtures.datasets]]
               id = "options"
               kind = "options"

               [[projects.fixtures.datasets.refs]]
               id = "small"

               [projects.fixtures.datasets.refs.producer]
               type = "existing-file"
               path = "fixtures/options-small.json"
               artifact = "options-json"
               "#,
        )
        .unwrap();

        let config = AppConfig::load(Some(&path)).unwrap();

        let producer = &config.projects["fixtures"].datasets[0].refs[0].producer;

        assert_eq!(producer.kind(), ProducerKind::ExistingFile);

        match producer {
            ProducerConfig::ExistingFile { path, artifact } => {
                assert_eq!(path, &PathBuf::from("fixtures/options-small.json"));
                assert_eq!(*artifact, ArtifactKind::OptionsJson);
            }
            other => panic!("unexpected producer: {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_project_ids() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nix-search.toml");

        fs::write(
            &path,
            r#"
               [projects."bad/project"]
               name = "Bad Project"
               "#,
        )
        .unwrap();

        let error = AppConfig::load(Some(&path)).unwrap_err().to_string();

        assert!(error.contains("must not contain '/'"));
    }

    #[test]
    fn validates_nix_build_options_required_fields_by_deserialization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nix-search.toml");

        fs::write(
            &path,
            r#"
               [projects.nixpkgs]
               name = "Nixpkgs"

               [[projects.nixpkgs.datasets]]
               id = "nixos-options"
               kind = "options"

               [[projects.nixpkgs.datasets.refs]]
               id = "unstable"

               [projects.nixpkgs.datasets.refs.producer]
               type = "nix-build-options-json"
               ref = "github:NixOS/nixpkgs/nixos-unstable"
               "#,
        )
        .unwrap();

        let error = AppConfig::load(Some(&path)).unwrap_err().to_string();

        assert!(error.contains("attribute"));
    }

    #[test]
    fn validates_custom_command_is_not_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nix-search.toml");

        fs::write(
            &path,
            r#"
               [projects.custom]
               name = "Custom"

               [[projects.custom.datasets]]
               id = "options"
               kind = "options"

               [[projects.custom.datasets.refs]]
               id = "main"

               [projects.custom.datasets.refs.producer]
               type = "custom-command"
               command = []
               artifact = "options-json"
               "#,
        )
        .unwrap();

        let error = AppConfig::load(Some(&path)).unwrap_err().to_string();

        assert!(error.contains("command must not be empty"));
    }
}
