use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use si_data_nats::NatsConfig;
pub use si_settings::{StandardConfig, StandardConfigFile};
use ulid::Ulid;

const DEFAULT_ATTRIBUTE_VALUE_MAX_ATTEMPTS: u16 = 3;

#[remain::sorted]
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    Builder(#[from] ConfigBuilderError),
    #[error(transparent)]
    Settings(#[from] si_settings::SettingsError),
}

pub type Result<T, E = ConfigError> = std::result::Result<T, E>;

#[derive(Debug, Builder)]
pub struct Config {
    #[builder(default = "NatsConfig::default()")]
    nats: NatsConfig,

    #[builder(default = "random_instance_id()")]
    instance_id: String,

    #[builder(default = "DEFAULT_ATTRIBUTE_VALUE_MAX_ATTEMPTS")]
    attribute_value_max_attempts: u16,
}

impl StandardConfig for Config {
    type Builder = ConfigBuilder;
}

impl Config {
    /// Gets a reference to the config's nats.
    #[must_use]
    pub fn nats(&self) -> &NatsConfig {
        &self.nats
    }

    /// Gets a reference to the config's subject prefix.
    pub fn subject_prefix(&self) -> Option<&str> {
        self.nats.subject_prefix.as_deref()
    }

    /// Gets the config's instance ID.
    pub fn instance_id(&self) -> &str {
        self.instance_id.as_ref()
    }

    /// Gets the value for max number of retry attempts when processing an attribute value.
    pub fn attribute_value_max_attempts(&self) -> u16 {
        self.attribute_value_max_attempts
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConfigFile {
    nats: NatsConfig,
}

impl StandardConfigFile for ConfigFile {
    type Error = ConfigError;
}

impl TryFrom<ConfigFile> for Config {
    type Error = ConfigError;

    fn try_from(value: ConfigFile) -> Result<Self> {
        let mut config = Config::builder();
        config.nats(value.nats);
        config.build().map_err(Into::into)
    }
}

fn random_instance_id() -> String {
    Ulid::new().to_string()
}
