use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use super::{SchemaVariantSpec, SpecError};

#[derive(Builder, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[builder(build_fn(error = "SpecError"))]
pub struct SchemaSpec {
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into))]
    pub category: String,
    #[builder(setter(into, strip_option), default)]
    pub category_name: Option<String>,

    #[builder(setter(each(name = "variant", into)), default)]
    pub variants: Vec<SchemaVariantSpec>,
}

impl SchemaSpec {
    #[must_use]
    pub fn builder() -> SchemaSpecBuilder {
        SchemaSpecBuilder::default()
    }

    #[allow(unused_mut)]
    pub fn try_variant<I>(&mut self, item: I) -> Result<&mut Self, I::Error>
    where
        I: TryInto<SchemaVariantSpec>,
    {
        let converted: SchemaVariantSpec = item.try_into()?;
        self.variants.extend(Some(converted));
        Ok(self)
    }
}

impl TryFrom<SchemaSpecBuilder> for SchemaSpec {
    type Error = SpecError;

    fn try_from(value: SchemaSpecBuilder) -> Result<Self, Self::Error> {
        value.build()
    }
}