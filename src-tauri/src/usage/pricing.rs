use crate::domain::Model;

const TOKENS_PER_MILLION: f64 = 1_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub provider_id: &'static str,
    pub model_id: &'static str,
    pub input_usd_per_million: f64,
    pub cached_input_usd_per_million: Option<f64>,
    pub output_usd_per_million: f64,
}

/// Maintained in one place; rates use USD per million tokens and represent
/// standard API inference (not subscriptions, discounts, or provider billing).
const PRICING: &[ModelPricing] = &[
    ModelPricing {
        provider_id: "openai",
        model_id: "gpt-5.4-mini",
        input_usd_per_million: 0.75,
        cached_input_usd_per_million: Some(0.075),
        output_usd_per_million: 4.50,
    },
    ModelPricing {
        provider_id: "anthropic",
        model_id: "claude-sonnet-4-6",
        input_usd_per_million: 3.0,
        cached_input_usd_per_million: Some(0.30),
        output_usd_per_million: 15.0,
    },
    ModelPricing {
        provider_id: "gemini",
        model_id: "gemini-2.5-flash",
        input_usd_per_million: 0.30,
        cached_input_usd_per_million: Some(0.03),
        output_usd_per_million: 2.50,
    },
];

fn pricing_for(model: &Model) -> Option<(f64, f64, f64)> {
    if let Some(pricing) = PRICING
        .iter()
        .find(|pricing| pricing.provider_id == model.provider && pricing.model_id == model.id)
    {
        return Some((
            pricing.input_usd_per_million,
            pricing
                .cached_input_usd_per_million
                .unwrap_or(pricing.input_usd_per_million),
            pricing.output_usd_per_million,
        ));
    }

    // Some provider model catalogs publish normalized USD/MTok prices. Keep
    // that fallback here so dashboard code has no provider-specific pricing logic.
    Some((model.input_price?, model.input_price?, model.output_price?))
}

pub fn estimate_cost(
    model: &Model,
    input_tokens: u64,
    cached_input_tokens: Option<u64>,
    output_tokens: u64,
) -> Option<f64> {
    let (input_rate, cached_rate, output_rate) = pricing_for(model)?;
    let cached = cached_input_tokens.unwrap_or_default().min(input_tokens);
    let uncached = input_tokens - cached;
    Some(
        ((uncached as f64 * input_rate)
            + (cached as f64 * cached_rate)
            + (output_tokens as f64 * output_rate))
            / TOKENS_PER_MILLION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str, id: &str) -> Model {
        Model {
            id: id.into(),
            provider: provider.into(),
            display_name: id.into(),
            api_protocol: None,
            capabilities: vec![],
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            supports_streaming: true,
            context_window: None,
            input_price: None,
            output_price: None,
            status: Default::default(),
        }
    }

    #[test]
    fn prices_cached_input_separately_for_a_registered_model() {
        let cost = estimate_cost(
            &model("openai", "gpt-5.4-mini"),
            1_000_000,
            Some(400_000),
            100_000,
        )
        .unwrap();
        assert!((cost - 0.93).abs() < 0.000_001);
    }

    #[test]
    fn unknown_model_without_catalog_prices_has_no_estimate() {
        assert_eq!(
            estimate_cost(&model("opencode-zen", "future-model"), 10, None, 5),
            None
        );
    }

    #[test]
    fn unknown_model_can_use_catalog_prices_via_the_registry() {
        let mut model = model("opencode-zen", "catalog-model");
        model.input_price = Some(1.0);
        model.output_price = Some(2.0);
        assert_eq!(estimate_cost(&model, 100_000, None, 50_000), Some(0.2));
    }
}
