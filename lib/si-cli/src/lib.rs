use color_eyre::eyre::ErrReport;
use color_eyre::Result;
use std::env::VarError;
use thiserror::Error;

pub mod cmd;
pub mod engine;
mod key_management;
pub mod state;

pub const CONTAINER_NAMES: &[&str] = &[
    "jaeger", "postgres", "nats", "otelcol", "council", "veritech", "pinga", "sdf", "web",
];

#[remain::sorted]
#[derive(Error, Debug)]
pub enum SiCliError {
    #[error("unable to connect to the container engine")]
    ContainerEngine,
    #[error("ctrl+c")]
    CtrlC,
    #[error("docker api: {0}")]
    Docker(#[from] docker_api::Error),
    #[error("container search failed: {0}")]
    DockerContainerSearch(String),
    #[error("err report: {0}")]
    ErrReport(#[from] ErrReport),
    #[error("failed to launch web url {0}")]
    FailToLaunch(String),
    #[error("incorrect installation type {0}")]
    IncorrectInstallMode(String),
    #[error("aborting installation")]
    Installation,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("join: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Unable to find local data dir. Expected format `$HOME/.local/share` or `$HOME/Library/Application Support`")]
    MissingDataDir(),
    #[error("podman api: {0}")]
    Podman(#[from] podman_api::Error),
    #[error("reqwest: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("symmetric crypto: {0}")]
    SymmetricCrypto(#[from] si_crypto::SymmetricCryptoError),
    #[error("toml deserialize error: {0}")]
    TomlDeserialize(#[from] toml::de::Error),
    #[error("unable to download update, status = {0}")]
    UnableToDownloadUpdate(u16),
    #[error("unable to fetch containers update, status = {0}")]
    UnableToFetchContainersUpdate(u16),
    #[error("unable to fetch si update, status = {0}")]
    UnableToFetchSiUpdate(u16),
    #[error("unsupported operating system: {0}")]
    UnsupportedOperatingSystem(String),
    #[error("env var: {0}")]
    Var(#[from] VarError),
    #[error("web portal is currently offline - please check that the system is running")]
    WebPortal(),
}

pub type CliResult<T> = Result<T, SiCliError>;
