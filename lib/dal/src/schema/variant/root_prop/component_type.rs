//! This module contains the ability to switch a [`Component's`](crate::Component) type between
//! a standard [`Component`](crate::Component) and a "frame". This functionality resides in this
//! location because it corresponds to the "/root/si/type" location in the
//! [`RootProp`](crate::RootProp) tree.
use serde::Deserialize;
use serde::Serialize;

use si_pkg::SchemaVariantSpecComponentType;
use strum::{AsRefStr, Display, EnumIter, EnumString};

/// The possible values of "/root/si/type".
#[remain::sorted]
#[derive(
    AsRefStr,
    Clone,
    Copy,
    Debug,
    Deserialize,
    Display,
    EnumIter,
    EnumString,
    Eq,
    PartialEq,
    Serialize,
)]
#[serde(rename_all = "camelCase")]
pub enum ComponentType {
    #[serde(alias = "AggregationFrame")]
    #[strum(serialize = "AggregationFrame", serialize = "aggregationFrame")]
    AggregationFrame,
    #[serde(alias = "Component")]
    #[strum(serialize = "Component", serialize = "component")]
    Component,
    #[serde(alias = "ConfigurationFrame")]
    #[strum(serialize = "ConfigurationFrame", serialize = "configurationFrame")]
    ConfigurationFrame,
}

impl From<SchemaVariantSpecComponentType> for ComponentType {
    fn from(value: SchemaVariantSpecComponentType) -> Self {
        match value {
            SchemaVariantSpecComponentType::Component => Self::Component,
            SchemaVariantSpecComponentType::ConfigurationFrame => Self::ConfigurationFrame,
            SchemaVariantSpecComponentType::AggregationFrame => Self::AggregationFrame,
        }
    }
}

impl From<ComponentType> for SchemaVariantSpecComponentType {
    fn from(value: ComponentType) -> Self {
        match value {
            ComponentType::Component => Self::Component,
            ComponentType::ConfigurationFrame => Self::ConfigurationFrame,
            ComponentType::AggregationFrame => Self::AggregationFrame,
        }
    }
}

impl ComponentType {
    /// Return the label corresponding to [`self`](Self).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Component => "Component",
            Self::ConfigurationFrame => "Configuration Frame",
            Self::AggregationFrame => "Aggregation Frame",
        }
    }
}
