use serde::Deserialize;
#[derive(Deserialize, Clone, Debug)]
pub struct A2DConfig {
    #[serde(alias = "apiKeySecretRef")]
    pub api_key_secret_ref: String,
    #[serde(alias = "assetId")]
    pub asset_id: String,
    #[serde(
        alias = "baseUrl",
        default,
        deserialize_with = "pdk::serde::deserialize_service_opt"
    )]
    pub base_url: Option<pdk::hl::Service>,
    #[serde(alias = "pdpTimeoutMs")]
    pub pdp_timeout_ms: Option<i64>,
    #[serde(alias = "pinPathPrefix")]
    pub pin_path_prefix: Option<String>,
    #[serde(alias = "refreshIntervalSec")]
    pub refresh_interval_sec: Option<i64>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct DecisionConfig {
    #[serde(alias = "hybridSampleRate")]
    pub hybrid_sample_rate: Option<f64>,
    #[serde(alias = "source")]
    pub source: Option<String>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct EnforceConfig {
    #[serde(alias = "allowAddedTools")]
    pub allow_added_tools: Option<bool>,
    #[serde(alias = "allowRemovedTools")]
    pub allow_removed_tools: Option<bool>,
    #[serde(alias = "exactMatch")]
    pub exact_match: Option<bool>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct EvidenceConfig {
    #[serde(alias = "logLocally")]
    pub log_locally: Option<bool>,
    #[serde(alias = "reportToA2d")]
    pub report_to_a_2_d: Option<bool>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct FailOpenConfig {
    #[serde(alias = "onPdpUnavailable")]
    pub on_pdp_unavailable: Option<bool>,
    #[serde(alias = "onSpecUnavailable")]
    pub on_spec_unavailable: Option<bool>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(alias = "a2d")]
    pub a_2_d: A2DConfig,
    #[serde(alias = "decision")]
    pub decision: Option<DecisionConfig>,
    #[serde(alias = "enforce")]
    pub enforce: Option<EnforceConfig>,
    #[serde(alias = "evidence")]
    pub evidence: Option<EvidenceConfig>,
    #[serde(alias = "failOpen")]
    pub fail_open: Option<FailOpenConfig>,
    #[serde(alias = "mode")]
    pub mode: Option<String>,
}
#[pdk::hl::entrypoint_flex]
fn init(abi: &dyn pdk::flex_abi::api::FlexAbi) -> Result<(), anyhow::Error> {
    let config: Config = serde_json::from_slice(abi.get_configuration())
        .map_err(|err| {
            anyhow::anyhow!(
                "Failed to parse configuration '{}'. Cause: {}",
                String::from_utf8_lossy(abi.get_configuration()), err
            )
        })?;
    let current = config.a_2_d;
    if current.base_url.is_some() {
        let service = current.base_url.unwrap();
        abi.service_create(service)?;
    }
    abi.setup()?;
    Ok(())
}
