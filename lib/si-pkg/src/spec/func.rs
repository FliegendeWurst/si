use derive_builder::Builder;
use object_tree::Hash;
use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumIter, EnumString};
use url::Url;

use super::SpecError;

#[derive(
    Deserialize,
    Serialize,
    AsRefStr,
    Display,
    EnumIter,
    EnumString,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[serde(rename_all = "camelCase")]
pub enum FuncArgumentKind {
    Array,
    Boolean,
    Integer,
    Object,
    String,
    Map,
    Any,
}

#[derive(Builder, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[builder(build_fn(error = "SpecError"))]
pub struct FuncArgumentSpec {
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into))]
    pub kind: FuncArgumentKind,
    #[builder(setter(into))]
    pub element_kind: Option<FuncArgumentKind>,
}

impl FuncArgumentSpec {
    pub fn builder() -> FuncArgumentSpecBuilder {
        FuncArgumentSpecBuilder::default()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, AsRefStr, Display, EnumIter, EnumString)]
#[serde(rename_all = "camelCase")]
pub enum FuncSpecBackendKind {
    JsAttribute,
    JsWorkflow,
    JsCommand,
    JsValidation,
    Json,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, AsRefStr, Display, EnumIter, EnumString)]
#[serde(rename_all = "camelCase")]
pub enum FuncSpecBackendResponseType {
    Array,
    Boolean,
    Integer,
    Map,
    Object,
    Qualification,
    CodeGeneration,
    Confirmation,
    String,
    Json,
    Validation,
    Workflow,
    Command,
}

pub type FuncUniqueId = Hash;

#[derive(Builder, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[builder(build_fn(error = "SpecError"))]
pub struct FuncSpec {
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into, strip_option), default)]
    pub display_name: Option<String>,
    #[builder(setter(into, strip_option), default)]
    pub description: Option<String>,
    #[builder(setter(into))]
    pub handler: String,
    #[builder(setter(into))]
    pub code_base64: String,
    #[builder(setter(into))]
    pub backend_kind: FuncSpecBackendKind,
    #[builder(setter(into))]
    pub response_type: FuncSpecBackendResponseType,
    #[builder(setter(into))]
    pub hidden: bool,
    #[builder(field(type = "FuncUniqueId", build = "self.build_func_unique_id()"))]
    pub unique_id: FuncUniqueId,

    #[builder(setter(into, strip_option), default)]
    pub link: Option<Url>,

    #[builder(setter(each(name = "argument"), into), default)]
    pub arguments: Vec<FuncArgumentSpec>,
}

impl FuncSpec {
    #[must_use]
    pub fn builder() -> FuncSpecBuilder {
        FuncSpecBuilder::default()
    }
}

impl FuncSpecBuilder {
    #[allow(unused_mut)]
    pub fn try_link<V>(&mut self, value: V) -> Result<&mut Self, V::Error>
    where
        V: TryInto<Url>,
    {
        let converted: Url = value.try_into()?;
        Ok(self.link(converted))
    }

    fn build_func_unique_id(&self) -> Hash {
        // Not happy about all these clones and unwraps...
        let mut bytes = vec![];
        bytes.extend(self.name.clone().unwrap_or("".to_string()).as_bytes());
        bytes.extend(
            self.display_name
                .clone()
                .unwrap_or(Some("".to_string()))
                .unwrap_or("".to_string())
                .as_bytes(),
        );
        bytes.extend(
            self.description
                .clone()
                .unwrap_or(Some("".to_string()))
                .unwrap_or("".to_string())
                .as_bytes(),
        );
        bytes.extend(self.handler.clone().unwrap_or("".to_string()).as_bytes());
        bytes.extend(
            self.code_base64
                .clone()
                .unwrap_or("".to_string())
                .as_bytes(),
        );
        bytes.extend(
            self.backend_kind
                .unwrap_or(FuncSpecBackendKind::Json)
                .to_string()
                .as_bytes(),
        );
        bytes.extend(
            self.response_type
                .unwrap_or(FuncSpecBackendResponseType::Json)
                .to_string()
                .as_bytes(),
        );
        bytes.extend(&[self.hidden.unwrap_or(false).into()]);

        Hash::new(&bytes)
    }
}
