use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::cost::AnalyticsPricing;

const CATALOG_URL: &str = "https://models.dev/api.json";
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq)]
struct ModelPrices {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl ModelPrices {
    fn pricing(self) -> AnalyticsPricing {
        AnalyticsPricing::new(self.input, self.cache_read, self.cache_write, self.output)
    }

    fn revision(self) -> String {
        let mut digest = Sha256::new();
        for price in [self.input, self.output, self.cache_read, self.cache_write] {
            digest.update(price.to_bits().to_le_bytes());
        }
        format!("{:x}", digest.finalize())
    }
}

#[derive(Default)]
struct PricingCatalog {
    prices: HashMap<(String, String), ModelPrices>,
    refreshed_at: Option<Instant>,
    last_attempt_at: Option<Instant>,
}

#[derive(Deserialize)]
struct CatalogProvider {
    #[serde(default)]
    models: HashMap<String, CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    cost: Option<CatalogModelCost>,
}

#[derive(Deserialize)]
struct CatalogModelCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

static PRICING_CATALOG: LazyLock<Mutex<PricingCatalog>> =
    LazyLock::new(|| Mutex::new(PricingCatalog::default()));

/// Looks up an exact model ID from a provider namespace in the live models.dev catalog.
pub(crate) fn pricing_for_provider_model(provider: &str, model: &str) -> Option<AnalyticsPricing> {
    let provider = provider.trim().to_ascii_lowercase();
    let model = model.trim().to_ascii_lowercase();
    if provider.is_empty() || model.is_empty() {
        return None;
    }

    let mut catalog = PRICING_CATALOG.lock().ok()?;
    refresh_if_due(&mut catalog);
    catalog
        .prices
        .get(&(provider, model))
        .copied()
        .map(ModelPrices::pricing)
}

pub(crate) fn pricing_revision_for_provider_model(provider: &str, model: &str) -> Option<String> {
    let provider = provider.trim().to_ascii_lowercase();
    let model = model.trim().to_ascii_lowercase();
    let mut catalog = PRICING_CATALOG.lock().ok()?;
    refresh_if_due(&mut catalog);
    catalog.refreshed_at?;
    Some(
        catalog
            .prices
            .get(&(provider, model))
            .copied()
            .map(ModelPrices::revision)
            .unwrap_or_else(|| "unpriced".to_string()),
    )
}

/// Resolves provider-prefixed Cursor model labels only when the catalog has one
/// unambiguous model ID and price for the label.
pub(crate) fn pricing_for_unambiguous_model(model: &str) -> Option<AnalyticsPricing> {
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() {
        return None;
    }

    let mut catalog = PRICING_CATALOG.lock().ok()?;
    refresh_if_due(&mut catalog);
    let matches = catalog
        .prices
        .iter()
        .filter(|((_, model_id), _)| contains_model_id(&model, model_id))
        .collect::<Vec<_>>();
    let longest = matches
        .iter()
        .map(|((_, model_id), _)| model_id.len())
        .max()?;
    let mut prices = matches
        .into_iter()
        .filter(|((_, model_id), _)| model_id.len() == longest)
        .map(|(_, prices)| *prices);
    let first = prices.next()?;
    prices
        .all(|prices| prices == first)
        .then_some(first.pricing())
}

pub(crate) fn pricing_revision_for_unambiguous_model(model: &str) -> Option<String> {
    let model = model.trim().to_ascii_lowercase();
    let mut catalog = PRICING_CATALOG.lock().ok()?;
    refresh_if_due(&mut catalog);
    catalog.refreshed_at?;
    let matches = catalog
        .prices
        .iter()
        .filter(|((_, model_id), _)| contains_model_id(&model, model_id))
        .collect::<Vec<_>>();
    let Some(longest) = matches
        .iter()
        .map(|((_, model_id), _)| model_id.len())
        .max()
    else {
        return Some("unpriced".to_string());
    };
    let mut prices = matches
        .into_iter()
        .filter(|((_, model_id), _)| model_id.len() == longest)
        .map(|(_, prices)| *prices);
    let Some(first) = prices.next() else {
        return Some("unpriced".to_string());
    };
    if prices.all(|prices| prices == first) {
        Some(first.revision())
    } else {
        Some("unpriced".to_string())
    }
}

fn contains_model_id(label: &str, model_id: &str) -> bool {
    label.match_indices(model_id).any(|(start, matched)| {
        let end = start + matched.len();
        let left_boundary = label[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_alphanumeric());
        let right_boundary = label[end..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_ascii_alphanumeric());
        left_boundary && right_boundary
    })
}

fn refresh_if_due(catalog: &mut PricingCatalog) {
    let now = Instant::now();
    let due = catalog
        .refreshed_at
        .is_none_or(|refreshed| now.duration_since(refreshed) >= REFRESH_INTERVAL)
        && catalog
            .last_attempt_at
            .is_none_or(|attempt| now.duration_since(attempt) >= RETRY_INTERVAL);
    if !due {
        return;
    }

    catalog.last_attempt_at = Some(now);
    match fetch_catalog() {
        Ok(prices) => {
            crate::logging::global().info(
                "model pricing catalog loaded",
                serde_json::json!({ "modelCount": prices.len() }),
            );
            catalog.prices = prices;
            catalog.refreshed_at = Some(now);
        }
        Err(error) => {
            crate::logging::global().warn(
                "model pricing catalog refresh failed",
                serde_json::json!({ "source": "models.dev", "error": format!("{error:#}") }),
            );
        }
    }
}

fn fetch_catalog() -> Result<HashMap<(String, String), ModelPrices>> {
    let response = ureq::get(CATALOG_URL)
        .timeout(Duration::from_secs(10))
        .set("Accept", "application/json")
        .call()
        .context("failed to request the models.dev catalog")?;
    let body = response
        .into_string()
        .context("failed to read the models.dev catalog")?;
    let providers = serde_json::from_str::<HashMap<String, CatalogProvider>>(&body)
        .context("models.dev returned an unsupported catalog shape")?;
    let mut prices = HashMap::new();
    for (provider_id, provider) in providers {
        for (model_id, model) in provider.models {
            let Some(cost) = model.cost else {
                continue;
            };
            let (Some(input), Some(output)) = (cost.input, cost.output) else {
                continue;
            };
            let cache_read = cost.cache_read.unwrap_or(input);
            let cache_write = cost.cache_write.unwrap_or(input);
            if [input, output, cache_read, cache_write]
                .into_iter()
                .any(|price| !price.is_finite() || price < 0.0)
            {
                continue;
            }
            prices.insert(
                (
                    provider_id.to_ascii_lowercase(),
                    model_id.to_ascii_lowercase(),
                ),
                ModelPrices {
                    input,
                    output,
                    cache_read,
                    cache_write,
                },
            );
        }
    }
    if prices.is_empty() {
        bail!("models.dev catalog contains no model prices");
    }
    Ok(prices)
}
