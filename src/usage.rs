//! What each call used, counted in the `usage` table by day, agent, person, purpose (`chat`, `lookup`, `summary`) and
//! model, and what it cost at the model's listed prices. Cost is kept in billionths of a dollar so it adds up exactly.

use crate::{config::Cost, provider::chat::Usage, store::Store, who::Who};
use anyhow::Result;
use rusqlite::params;

/// What a call cost in US dollars: the prices of the highest tier its input is above, else the base prices. A model
/// with no listed cost costs nothing.
pub fn cost_usd(cost: Option<&Cost>, usage: &Usage) -> f64 {
    let Some(cost) = cost else { return 0.0 };
    let all_input = usage.input + usage.cache_read + usage.cache_write;
    let (input, output, cache_read, cache_write) = cost
        .tiers
        .iter()
        .filter(|t| all_input > t.input_tokens_above)
        .max_by_key(|t| t.input_tokens_above)
        .map_or(
            (cost.input, cost.output, cost.cache_read, cost.cache_write),
            |t| (t.input, t.output, t.cache_read, t.cache_write),
        );
    (usage.input as f64 * input
        + usage.output as f64 * output
        + usage.cache_read as f64 * cache_read
        + usage.cache_write as f64 * cache_write)
        / 1e6
}

impl Store {
    pub fn count_usage(
        &self,
        day: &str,
        who: &Who,
        purpose: &str,
        model: &str,
        usage: &Usage,
        usd: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO usage (day, agent, person, purpose, model, calls, input, output, cache_read, cache_write, nano_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (day, agent, person, purpose, model) DO UPDATE SET
               calls = calls + 1, input = input + ?6, output = output + ?7, cache_read = cache_read + ?8,
               cache_write = cache_write + ?9, nano_usd = nano_usd + ?10",
            params![
                day,
                who.agent,
                who.person,
                purpose,
                model,
                usage.input as i64,
                usage.output as i64,
                usage.cache_read as i64,
                usage.cache_write as i64,
                (usd * 1e9).round() as i64
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CostTier;

    #[test]
    fn cost_uses_the_tier_the_input_is_above() {
        let cost = Cost {
            input: 2.0,
            output: 10.0,
            cache_read: 1.0,
            cache_write: 0.0,
            tiers: vec![CostTier {
                input_tokens_above: 1000,
                input: 4.0,
                output: 20.0,
                cache_read: 2.0,
                cache_write: 0.0,
            }],
        };
        let small = Usage {
            input: 500,
            output: 100,
            cache_read: 0,
            cache_write: 0,
        };
        assert!((cost_usd(Some(&cost), &small) - (500.0 * 2.0 + 100.0 * 10.0) / 1e6).abs() < 1e-12);
        let large = Usage {
            input: 1500,
            output: 100,
            cache_read: 0,
            cache_write: 0,
        };
        assert!(
            (cost_usd(Some(&cost), &large) - (1500.0 * 4.0 + 100.0 * 20.0) / 1e6).abs() < 1e-12
        );
        assert_eq!(cost_usd(None, &large), 0.0);
    }
}
