use serde::{Deserialize, Serialize};
use si_events::{merkle_tree_hash::MerkleTreeHash, ulid::Ulid, ContentHash};

use crate::{
    workspace_snapshot::graph::deprecated::v1::DeprecatedDependentValueRootNodeWeightV1,
    workspace_snapshot::node_weight::traits::CorrectTransforms, EdgeWeightKindDiscriminants,
};

use super::NodeHash;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependentValueRootNodeWeight {
    pub id: Ulid,
    pub lineage_id: Ulid,
    value_id: Ulid,
    pub(super) merkle_tree_hash: MerkleTreeHash,
}

impl DependentValueRootNodeWeight {
    pub fn content_hash(&self) -> ContentHash {
        self.node_hash()
    }

    pub fn content_store_hashes(&self) -> Vec<ContentHash> {
        vec![]
    }

    pub fn id(&self) -> Ulid {
        self.id
    }

    pub fn value_id(&self) -> Ulid {
        self.value_id
    }

    pub fn new(id: Ulid, lineage_id: Ulid, value_id: Ulid) -> Self {
        Self {
            id,
            lineage_id,
            value_id,
            merkle_tree_hash: Default::default(),
        }
    }

    pub const fn exclusive_outgoing_edges(&self) -> &[EdgeWeightKindDiscriminants] {
        &[]
    }
}

impl NodeHash for DependentValueRootNodeWeight {
    fn node_hash(&self) -> ContentHash {
        ContentHash::from(&serde_json::json![{
            "value_id": self.value_id,
        }])
    }
}

impl std::fmt::Debug for DependentValueRootNodeWeight {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("DependentValueNodeWeight")
            .field("id", &self.id.to_string())
            .field("lineage_id", &self.lineage_id.to_string())
            .field("value_id", &self.value_id.to_string())
            .field("merkle_tree_hash", &self.merkle_tree_hash)
            .finish()
    }
}

impl From<DeprecatedDependentValueRootNodeWeightV1> for DependentValueRootNodeWeight {
    fn from(value: DeprecatedDependentValueRootNodeWeightV1) -> Self {
        Self {
            id: value.id,
            lineage_id: value.lineage_id,
            value_id: value.value_id,
            merkle_tree_hash: value.merkle_tree_hash,
        }
    }
}

impl CorrectTransforms for DependentValueRootNodeWeight {}
