use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::ProviderWireApi;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderVendor {
    pub name: String,
    pub base_url: Option<String>,
    pub credential: Option<String>,
    pub headers: Option<String>,
    pub wire_apis: Vec<ProviderWireApi>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderModelBinding {
    pub binding_id: String,
    pub model_slug: String,
    pub provider: String,
    pub model_name: String,
    pub display_name: Option<String>,
    pub invocation_method: ProviderWireApi,
    pub default_reasoning_effort: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderVendorListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderVendorListResult {
    pub provider_vendors: Vec<ProviderVendor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderVendorUpsertParams {
    pub provider_vendor: ProviderVendor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ProviderModelBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model_binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderVendorUpsertResult {
    pub provider_vendor: ProviderVendor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ProviderModelBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderValidateParams {
    pub provider_vendor: ProviderVendor,
    pub model_binding: ProviderModelBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProviderValidateResult {
    pub reply_preview: String,
}

// TODO: Write ProviderVendor list to the current configuration
// TODO: Read ProviderVendor list from current configuration
// TODO: The api key should at auth.json file

#[derive(Debug, Default)]
pub struct ProviderVendorCatalog {
    pub provider_vendors: Vec<ProviderVendor>,
}

impl ProviderVendorCatalog {
    pub fn list(&self) -> Vec<&ProviderVendor> {
        self.provider_vendors.iter().collect()
    }

    pub fn get(&self, name: &str) -> Option<&ProviderVendor> {
        self.provider_vendors
            .iter()
            .find(|&provider_vendor| provider_vendor.name.as_str() == name)
    }

    pub fn new() -> Self {
        Self {
            provider_vendors: Vec::new(),
        }
    }
}
