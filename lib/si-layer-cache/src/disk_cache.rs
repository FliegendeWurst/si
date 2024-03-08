use si_std::CanonicalFile;
use sled::Db;
use std::marker::PhantomData;

use crate::error::LayerCacheResult;

pub fn default_sled_path() -> LayerCacheResult<CanonicalFile> {
    Ok(tempfile::tempdir()?.into_path().try_into()?)
}

#[derive(Clone, Debug)]
pub struct DiskCache<K>
where
    K: AsRef<[u8]> + Copy + Send + Sync,
{
    tree: sled::Tree,
    // We have to make it appear that we hold on to a K when we don't actually
    // do so. This allows us to use static dispatch, etc.
    _phantom_of_the_opera: PhantomData<K>,
}

impl<K> DiskCache<K>
where
    K: AsRef<[u8]> + Copy + Send + Sync,
{
    pub fn new(sled_db: Db, tree_name: impl AsRef<[u8]>) -> LayerCacheResult<Self> {
        let tree = sled_db.open_tree(tree_name.as_ref())?;
        Ok(Self {
            tree,
            _phantom_of_the_opera: PhantomData,
        })
    }

    pub fn get(&self, key: &K) -> LayerCacheResult<Option<Vec<u8>>> {
        Ok(self.tree.get(*key)?.map(|bytes| bytes.to_vec()))
    }

    pub fn contains_key(&self, key: &K) -> LayerCacheResult<bool> {
        Ok(self.tree.contains_key(*key)?)
    }

    pub fn insert(&self, key: K, value: &[u8]) -> LayerCacheResult<()> {
        self.tree.insert(key.as_ref(), value)?;
        Ok(())
    }
}
