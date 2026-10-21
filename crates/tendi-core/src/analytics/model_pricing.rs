use std::{
    collections::HashMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeSeed, MapAccess, Visitor},
};
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
    catalog_path: Option<PathBuf>,
    provider_prices: HashMap<(String, String), Option<ModelPrices>>,
    unambiguous_prices: HashMap<String, Option<ModelPrices>>,
    refreshed_at: Option<Instant>,
    last_attempt_at: Option<Instant>,
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
    provider_model_price(&mut catalog, provider, model).map(ModelPrices::pricing)
}

pub(crate) fn pricing_revision_for_provider_model(provider: &str, model: &str) -> Option<String> {
    let provider = provider.trim().to_ascii_lowercase();
    let model = model.trim().to_ascii_lowercase();
    let mut catalog = PRICING_CATALOG.lock().ok()?;
    refresh_if_due(&mut catalog);
    catalog.catalog_path.as_ref()?;
    Some(
        provider_model_price(&mut catalog, provider, model)
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
    unambiguous_model_price(&mut catalog, model).map(ModelPrices::pricing)
}

pub(crate) fn pricing_revision_for_unambiguous_model(model: &str) -> Option<String> {
    let model = model.trim().to_ascii_lowercase();
    let mut catalog = PRICING_CATALOG.lock().ok()?;
    refresh_if_due(&mut catalog);
    catalog.catalog_path.as_ref()?;
    Some(
        unambiguous_model_price(&mut catalog, model)
            .map(ModelPrices::revision)
            .unwrap_or_else(|| "unpriced".to_string()),
    )
}

fn provider_model_price(
    catalog: &mut PricingCatalog,
    provider: String,
    model: String,
) -> Option<ModelPrices> {
    let key = (provider.clone(), model.clone());
    if let Some(price) = catalog.provider_prices.get(&key) {
        return *price;
    }
    let Some(catalog_path) = catalog.catalog_path.clone() else {
        return None;
    };
    let price = if catalog_contains_model_id(&catalog_path, &model).unwrap_or(true) {
        lookup_catalog(&catalog_path, CatalogLookup::Exact { provider, model })
            .ok()
            .and_then(|matches| matches.into_iter().next().map(|(_, _, price)| price))
    } else {
        None
    };
    catalog.provider_prices.insert(key, price);
    price
}

fn unambiguous_model_price(catalog: &mut PricingCatalog, model: String) -> Option<ModelPrices> {
    if let Some(price) = catalog.unambiguous_prices.get(&model) {
        return *price;
    }
    let catalog_path = catalog.catalog_path.clone()?;
    let matches = lookup_catalog(
        &catalog_path,
        CatalogLookup::Unambiguous {
            label: model.clone(),
        },
    )
    .ok()?;
    let longest = matches.iter().map(|(_, model_id, _)| model_id.len()).max();
    let price = longest.and_then(|longest| {
        let mut matching_prices = matches
            .iter()
            .filter(|(_, model_id, _)| model_id.len() == longest)
            .map(|(_, _, price)| *price);
        let first = matching_prices.next()?;
        matching_prices.all(|price| price == first).then_some(first)
    });
    catalog.unambiguous_prices.insert(model, price);
    price
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
    load_cached_catalog(catalog, now);
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
        Ok(catalog_path) => {
            let catalog_bytes = fs::metadata(&catalog_path)
                .map(|metadata| metadata.len())
                .unwrap_or_default();
            crate::logging::global().info(
                "model pricing catalog loaded",
                serde_json::json!({ "catalogBytes": catalog_bytes }),
            );
            catalog.catalog_path = Some(catalog_path);
            catalog.provider_prices.clear();
            catalog.unambiguous_prices.clear();
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

fn load_cached_catalog(catalog: &mut PricingCatalog, now: Instant) {
    if catalog.catalog_path.is_some() {
        return;
    }
    let Ok(path) = pricing_cache_path() else {
        return;
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return;
    };
    if !metadata.is_file() {
        return;
    }

    catalog.catalog_path = Some(path);
    if let Some(age) = metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        && age < REFRESH_INTERVAL
    {
        catalog.refreshed_at = now.checked_sub(age);
    }
}

fn pricing_cache_path() -> Result<PathBuf> {
    let cache_dir = dirs::cache_dir()
        .context("could not resolve the application cache directory")?
        .join("tendi");
    Ok(cache_dir.join("model-pricing.json"))
}

fn fetch_catalog() -> Result<PathBuf> {
    let cache_path = pricing_cache_path()?;
    let cache_dir = cache_path
        .parent()
        .context("model pricing cache path has no parent directory")?;
    fs::create_dir_all(cache_dir).context("failed to create model pricing cache directory")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary_path =
        cache_dir.join(format!(".model-pricing-{}-{nonce}.tmp", std::process::id()));
    let response = ureq::get(CATALOG_URL)
        .timeout(Duration::from_secs(10))
        .set("Accept", "application/json")
        .call()
        .context("failed to request the models.dev catalog")?;
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .context("failed to create temporary model pricing cache")?;
        std::io::copy(&mut response.into_reader(), &mut file)
            .context("failed to write model pricing cache")?;
        file.flush()
            .context("failed to flush temporary model pricing cache")?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary_path, &cache_path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error).context("failed to publish model pricing cache");
    }
    Ok(cache_path)
}

#[derive(Clone)]
enum CatalogLookup {
    Exact { provider: String, model: String },
    Unambiguous { label: String },
}

impl CatalogLookup {
    fn matches_provider(&self, provider: &str) -> bool {
        match self {
            Self::Exact {
                provider: expected, ..
            } => provider.eq_ignore_ascii_case(expected),
            Self::Unambiguous { .. } => true,
        }
    }

    fn matches_model(&self, model: &str) -> bool {
        match self {
            Self::Exact {
                model: expected, ..
            } => model.eq_ignore_ascii_case(expected),
            Self::Unambiguous { label } => {
                contains_model_id(&label.to_ascii_lowercase(), &model.to_ascii_lowercase())
            }
        }
    }
}

type CatalogMatch = (String, String, ModelPrices);

fn lookup_catalog(path: &Path, lookup: CatalogLookup) -> Result<Vec<CatalogMatch>> {
    let file = File::open(path).context("failed to open model pricing cache")?;
    let mut matches = Vec::new();
    let mut deserializer = serde_json::Deserializer::from_reader(BufReader::new(file));
    CatalogSeed {
        lookup,
        matches: &mut matches,
    }
    .deserialize(&mut deserializer)
    .context("models.dev returned an unsupported catalog shape")?;
    deserializer
        .end()
        .context("models.dev returned invalid trailing catalog data")?;
    Ok(matches)
}

fn catalog_contains_model_id(path: &Path, model: &str) -> Result<bool> {
    let needle = model.as_bytes();
    if needle.is_empty() {
        return Ok(false);
    }

    let mut reader =
        BufReader::new(File::open(path).context("failed to open model pricing cache")?);
    let mut chunk = [0_u8; 8192];
    let mut overlap = Vec::with_capacity(needle.len().saturating_sub(1));
    loop {
        let read = reader
            .read(&mut chunk)
            .context("failed to read model pricing cache")?;
        if read == 0 {
            return Ok(false);
        }
        overlap.extend_from_slice(&chunk[..read]);
        if overlap
            .windows(needle.len())
            .any(|candidate| candidate.eq_ignore_ascii_case(needle))
        {
            return Ok(true);
        }
        let keep = needle.len().saturating_sub(1);
        if overlap.len() > keep {
            overlap.drain(..overlap.len() - keep);
        }
    }
}

struct CatalogSeed<'a> {
    lookup: CatalogLookup,
    matches: &'a mut Vec<CatalogMatch>,
}

impl<'de> DeserializeSeed<'de> for CatalogSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(CatalogVisitor {
            lookup: self.lookup,
            matches: self.matches,
        })
    }
}

struct CatalogVisitor<'a> {
    lookup: CatalogLookup,
    matches: &'a mut Vec<CatalogMatch>,
}

impl<'de> Visitor<'de> for CatalogVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a models.dev provider catalog")
    }

    fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        while let Some(provider_id) = map.next_key::<String>()? {
            if self.lookup.matches_provider(&provider_id) {
                map.next_value_seed(ProviderSeed {
                    provider_id: provider_id.to_ascii_lowercase(),
                    lookup: self.lookup.clone(),
                    matches: &mut *self.matches,
                })?;
            } else {
                map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

struct ProviderSeed<'a> {
    provider_id: String,
    lookup: CatalogLookup,
    matches: &'a mut Vec<CatalogMatch>,
}

impl<'de> DeserializeSeed<'de> for ProviderSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ProviderVisitor {
            provider_id: self.provider_id,
            lookup: self.lookup,
            matches: self.matches,
        })
    }
}

struct ProviderVisitor<'a> {
    provider_id: String,
    lookup: CatalogLookup,
    matches: &'a mut Vec<CatalogMatch>,
}

impl<'de> Visitor<'de> for ProviderVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a models.dev provider")
    }

    fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        while let Some(field) = map.next_key::<String>()? {
            if field == "models" {
                map.next_value_seed(ModelsSeed {
                    provider_id: self.provider_id.clone(),
                    lookup: self.lookup.clone(),
                    matches: &mut *self.matches,
                })?;
            } else {
                map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

struct ModelsSeed<'a> {
    provider_id: String,
    lookup: CatalogLookup,
    matches: &'a mut Vec<CatalogMatch>,
}

impl<'de> DeserializeSeed<'de> for ModelsSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ModelsVisitor {
            provider_id: self.provider_id,
            lookup: self.lookup,
            matches: self.matches,
        })
    }
}

struct ModelsVisitor<'a> {
    provider_id: String,
    lookup: CatalogLookup,
    matches: &'a mut Vec<CatalogMatch>,
}

impl<'de> Visitor<'de> for ModelsVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a models.dev model map")
    }

    fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        while let Some(model_id) = map.next_key::<String>()? {
            if !self.lookup.matches_model(&model_id) {
                map.next_value::<serde::de::IgnoredAny>()?;
                continue;
            }
            let model = map.next_value::<CatalogModel>()?;
            if let Some(prices) = model.cost.and_then(model_prices) {
                self.matches.push((
                    self.provider_id.clone(),
                    model_id.to_ascii_lowercase(),
                    prices,
                ));
            }
        }
        Ok(())
    }
}

fn model_prices(cost: CatalogModelCost) -> Option<ModelPrices> {
    let (Some(input), Some(output)) = (cost.input, cost.output) else {
        return None;
    };
    let cache_read = cost.cache_read.unwrap_or(input);
    let cache_write = cost.cache_write.unwrap_or(input);
    if [input, output, cache_read, cache_write]
        .into_iter()
        .any(|price| !price.is_finite() || price < 0.0)
    {
        return None;
    }
    Some(ModelPrices {
        input,
        output,
        cache_read,
        cache_write,
    })
}
