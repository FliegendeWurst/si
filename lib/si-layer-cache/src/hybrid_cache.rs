use foyer::{
    DirectFsDeviceOptions, Engine, FifoPicker, HybridCache, HybridCacheBuilder, LargeEngineOptions,
    RateLimitPicker, RecoverMode,
};
use std::cmp::min;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use telemetry::tracing::{error, info};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::db::serialize;
use crate::error::LayerDbResult;
use crate::LayerDbError;

const FOYER_DISK_CACHE_MINUMUM: usize = 1024 * 1024 * 1024; // 1gb
const FOYER_MEMORY_CACHE_MINUMUM: usize = 1024 * 1024 * 1024; // 1gb
const DEFAULT_DISK_CACHE_RATE_LIMIT: usize = 1024 * 1024 * 1024;
const DEFAULT_DISK_BUFFER_SIZE: usize = 1024 * 1024 * 128; // 128mb
const DEFAULT_DISK_BUFFER_FLUSHERS: usize = 2;
const DEFAULT_DISK_INDEXER_SHARDS: usize = 64;
const DEFAULT_DISK_RECLAIMERS: usize = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
enum MaybeDeserialized<V>
where
    V: Serialize + Clone + Send + Sync + 'static,
{
    RawBytes(Vec<u8>),
    DeserializedValue(V),
}

#[derive(Clone, Debug)]
pub struct Cache<V>
where
    V: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    cache: HybridCache<Arc<str>, MaybeDeserialized<V>>,
}

impl<V> Cache<V>
where
    V: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    pub async fn new(config: CacheConfig) -> LayerDbResult<Self> {
        let mem_cap = min(
            (config.memory as f32 * config.memory_percentage) as usize,
            FOYER_MEMORY_CACHE_MINUMUM,
        );
        let disk_cap = min(
            (config.disk_capacity as f32 * config.disk_percentage) as usize,
            FOYER_DISK_CACHE_MINUMUM,
        ) as usize;
        info!(
            "Creating cache {} with memory capcity of {} and disk capacity of {}",
            config.name, mem_cap, disk_cap
        );
        let cache: HybridCache<Arc<str>, MaybeDeserialized<V>> = HybridCacheBuilder::new()
            .memory(mem_cap)
            .with_weighter(|_key: &Arc<str>, value: &MaybeDeserialized<V>| size_of_val(value))
            .storage(Engine::Large)
            .with_admission_picker(Arc::new(RateLimitPicker::new(
                config.disk_admission_rate_limit,
            )))
            .with_device_options(
                DirectFsDeviceOptions::new(config.disk_path).with_capacity(disk_cap),
            )
            .with_large_object_disk_cache_options(
                LargeEngineOptions::new()
                    .with_buffer_pool_size(config.disk_buffer_size)
                    .with_eviction_pickers(vec![Box::<FifoPicker>::default()])
                    .with_flushers(config.disk_buffer_flushers)
                    .with_indexer_shards(config.disk_indexer_shards)
                    .with_reclaimers(config.disk_reclaimers),
            )
            .with_recover_mode(RecoverMode::Quiet)
            .build()
            .await
            .map_err(|e| LayerDbError::Foyer(e.into()))?;

        Ok(Self { cache })
    }

    pub async fn get(&self, key: &str) -> Option<V> {
        match self.cache.obtain(key.into()).await {
            Ok(Some(entry)) => match entry.value() {
                MaybeDeserialized::DeserializedValue(v) => Some(v.clone()),
                MaybeDeserialized::RawBytes(bytes) => {
                    // If we fail to deserialize the raw bytes for some reason, pretend that we never
                    // had the key in the first place, and also remove it from the cache.
                    match serialize::from_bytes_async::<V>(bytes).await {
                        Ok(deserialized) => {
                            self.insert(key.into(), deserialized.clone());
                            Some(deserialized)
                        }
                        Err(e) => {
                            error!(
                        "Failed to deserialize stored bytes from memory cache for key ({:?}): {}",
                        key,
                        e
                    );
                            self.remove(key);
                            None
                        }
                    }
                }
            },

            _ => None,
        }
    }

    pub fn insert(&self, key: Arc<str>, value: V) {
        self.cache
            .insert(key, MaybeDeserialized::DeserializedValue(value));
    }

    pub fn insert_raw_bytes(&self, key: Arc<str>, raw_bytes: Vec<u8>) {
        self.cache
            .insert(key, MaybeDeserialized::RawBytes(raw_bytes));
    }

    pub fn remove(&self, key: &str) {
        self.cache.remove(key);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.cache.contains(key)
    }

    pub async fn close(&self) -> LayerDbResult<()> {
        self.cache
            .close()
            .await
            .map_err(|e| LayerDbError::Foyer(e.into()))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheConfig {
    disk_admission_rate_limit: usize,
    disk_buffer_size: usize,
    disk_buffer_flushers: usize,
    disk_capacity: usize,
    disk_indexer_shards: usize,
    disk_path: PathBuf,
    disk_percentage: f32,
    disk_reclaimers: usize,
    memory: usize,
    memory_percentage: f32,
    name: String,
}

impl CacheConfig {
    // set the percentage of the total disk space in the disk_path
    pub fn with_disk_percentage(mut self, disk_percentage: f32) -> Self {
        self.disk_percentage = disk_percentage;
        self
    }

    // set the percentage of total memory
    pub fn with_memory_percentage(mut self, memory_percentage: f32) -> Self {
        self.memory_percentage = memory_percentage;
        self
    }

    // append an additional path to the existing disk path
    pub fn with_path_join(mut self, path: impl AsRef<Path>) -> Self {
        self.disk_path = self.disk_path.join(path);
        self
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        let sys = sysinfo::System::new_all();
        let path = tempfile::TempDir::with_prefix_in("default-cache-", "/tmp")
            .expect("unable to create tmp dir for layerdb")
            .path()
            .to_path_buf();

        let stats = nix::sys::statvfs::statvfs(
            path.parent()
                .expect("parent must exist if we just created a directory in it"),
        )
        .expect("unable to get the size of the temp directory");
        let total_size = stats.fragment_size() * stats.blocks();

        Self {
            disk_admission_rate_limit: DEFAULT_DISK_CACHE_RATE_LIMIT,
            disk_buffer_size: DEFAULT_DISK_BUFFER_SIZE,
            disk_buffer_flushers: DEFAULT_DISK_BUFFER_FLUSHERS,
            disk_capacity: total_size as usize,
            disk_indexer_shards: DEFAULT_DISK_INDEXER_SHARDS,
            disk_reclaimers: DEFAULT_DISK_RECLAIMERS,
            disk_percentage: 0.10,
            disk_path: path,
            memory: sys.total_memory() as usize,
            memory_percentage: 1.0,
            name: "default".to_string(),
        }
    }
}
