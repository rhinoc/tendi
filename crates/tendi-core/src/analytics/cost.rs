use super::AnalyticsTokenUsage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsCost {
    pub input_usd: f64,
    pub cached_input_usd: f64,
    pub cache_write_input_usd: f64,
    pub output_usd: f64,
    pub total_usd: f64,
}

impl AnalyticsCost {
    pub(crate) fn add_assign(&mut self, other: Self) {
        self.input_usd += other.input_usd;
        self.cached_input_usd += other.cached_input_usd;
        self.cache_write_input_usd += other.cache_write_input_usd;
        self.output_usd += other.output_usd;
        self.total_usd += other.total_usd;
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AnalyticsPricing {
    pub input_usd_per_million: f64,
    pub cached_input_usd_per_million: f64,
    pub cache_write_input_usd_per_million: f64,
    pub output_usd_per_million: f64,
}

impl AnalyticsPricing {
    pub(crate) const fn new(
        input_usd_per_million: f64,
        cached_input_usd_per_million: f64,
        cache_write_input_usd_per_million: f64,
        output_usd_per_million: f64,
    ) -> Self {
        Self {
            input_usd_per_million,
            cached_input_usd_per_million,
            cache_write_input_usd_per_million,
            output_usd_per_million,
        }
    }
}

pub(crate) fn calculate(usage: AnalyticsTokenUsage, pricing: AnalyticsPricing) -> AnalyticsCost {
    let uncached_input_tokens = usage.input_tokens.saturating_sub(usage.cached_input_tokens);
    let input_usd = per_million(uncached_input_tokens, pricing.input_usd_per_million);
    let cached_input_usd = per_million(
        usage.cached_input_tokens,
        pricing.cached_input_usd_per_million,
    );
    let cache_write_input_usd = per_million(
        usage.cache_write_input_tokens,
        pricing.cache_write_input_usd_per_million,
    );
    let output_usd = per_million(usage.output_tokens, pricing.output_usd_per_million);
    AnalyticsCost {
        input_usd,
        cached_input_usd,
        cache_write_input_usd,
        output_usd,
        total_usd: input_usd + cached_input_usd + cache_write_input_usd + output_usd,
    }
}

fn per_million(tokens: u64, usd_per_million: f64) -> f64 {
    tokens as f64 / 1_000_000.0 * usd_per_million
}

#[cfg(test)]
mod tests {
    use super::{AnalyticsPricing, calculate};
    use crate::analytics::AnalyticsTokenUsage;

    #[test]
    fn calculates_cached_and_uncached_input_separately() {
        let cost = calculate(
            AnalyticsTokenUsage {
                input_tokens: 1_000_000,
                cached_input_tokens: 250_000,
                cache_write_input_tokens: 100_000,
                output_tokens: 500_000,
                ..AnalyticsTokenUsage::default()
            },
            AnalyticsPricing::new(2.0, 0.5, 3.0, 4.0),
        );

        const EPSILON: f64 = 1e-12;
        assert!((cost.input_usd - 1.5).abs() < EPSILON);
        assert!((cost.cached_input_usd - 0.125).abs() < EPSILON);
        assert!((cost.cache_write_input_usd - 0.3).abs() < EPSILON);
        assert!((cost.output_usd - 2.0).abs() < EPSILON);
        assert!((cost.total_usd - 3.925).abs() < EPSILON);
    }
}
