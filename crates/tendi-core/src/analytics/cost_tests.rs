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
