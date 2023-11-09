use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use axum::extract::FromRef;
use tokio::sync::mpsc;

#[derive(Clone, FromRef)]
pub struct AppState {
    lang_server_path: LangServerPath,
    decryption_key: DecryptionKey,
    telemetry_level: TelemetryLevel,
}

impl AppState {
    pub fn new(
        lang_server_path: impl Into<PathBuf>,
        decryption_key: cyclone_core::CycloneDecryptionKey,
        telemetry_level: Box<dyn telemetry::TelemetryLevel>,
    ) -> Self {
        Self {
            lang_server_path: LangServerPath(Arc::new(lang_server_path.into())),
            decryption_key: DecryptionKey(Arc::new(decryption_key)),
            telemetry_level: TelemetryLevel(Arc::new(telemetry_level)),
        }
    }
}

#[derive(Clone, Debug, FromRef)]
pub struct LangServerPath(Arc<PathBuf>);

impl LangServerPath {
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Clone, Debug, FromRef)]
pub struct DecryptionKey(Arc<cyclone_core::CycloneDecryptionKey>);

impl Deref for DecryptionKey {
    type Target = cyclone_core::CycloneDecryptionKey;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<DecryptionKey> for Arc<cyclone_core::CycloneDecryptionKey> {
    fn from(value: DecryptionKey) -> Self {
        value.0
    }
}

#[derive(Clone, FromRef)]
pub struct TelemetryLevel(Arc<Box<dyn telemetry::TelemetryLevel>>);

impl Deref for TelemetryLevel {
    type Target = Box<dyn telemetry::TelemetryLevel>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct WatchKeepalive {
    tx: mpsc::Sender<()>,
    timeout: Duration,
}

impl WatchKeepalive {
    pub fn new(tx: mpsc::Sender<()>, timeout: Duration) -> Self {
        Self { tx, timeout }
    }

    pub fn clone_tx(&self) -> mpsc::Sender<()> {
        self.tx.clone()
    }

    /// Gets a reference to the watch keepalive tx's timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}
