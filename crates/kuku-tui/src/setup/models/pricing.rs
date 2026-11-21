use crossterm::event::{KeyCode, KeyEvent};
use kuku::adapter::openrouter_quote_url;
use kuku::config::{ModelPricing, ProviderConfig, TokenRates};

use crate::setup::state::{edit_field, SetupState, SetupStep};

impl SetupState {
    pub(in crate::setup) fn pricing_label(&self) -> &'static str {
        match self
            .primary_model()
            .and_then(|id| self.model_pricing.get(&id))
        {
            Some(ModelPricing::UsageOnly) => "Usage only",
            Some(ModelPricing::Configured { .. } | ModelPricing::Routed { .. }) => {
                "Configured USD rates"
            }
            Some(ModelPricing::Automatic) | None => "Automatic",
        }
    }

    pub(in crate::setup) fn choose_pricing(&mut self, key: KeyEvent) {
        if key.code != KeyCode::Enter {
            self.models.move_cursor(key.code, 3);
            return;
        }
        let Some(model) = self.primary_model() else {
            self.step = SetupStep::ReviewSettings;
            return;
        };
        if self.models.cursor != 1 {
            let mode = if self.models.cursor == 0 {
                ModelPricing::Automatic
            } else {
                ModelPricing::UsageOnly
            };
            self.model_pricing.insert(model, mode);
            self.step = SetupStep::ReviewSettings;
            return;
        }
        if matches!(
            self.model_pricing.get(&model),
            Some(ModelPricing::Routed { .. })
        ) {
            self.problem = Some("Edit the route rates in your config file.".into());
            return;
        }
        let rates = match self.model_pricing.get(&model) {
            Some(ModelPricing::Configured { rates }) => rates.clone(),
            _ => TokenRates::default(),
        };
        for (field, rate) in self.rate_fields.iter_mut().zip([
            rates.input,
            rates.output,
            rates.cache_read,
            rates.cache_write,
        ]) {
            field.set_text(rate.map_or_else(String::new, |rate| rate.to_string()));
        }
        self.models.cursor = 0;
        self.step = SetupStep::Rates;
    }

    pub(in crate::setup) fn edit_rates(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Down => self.models.move_cursor(key.code, 5),
            KeyCode::Enter if self.models.cursor < 4 => self.models.cursor += 1,
            KeyCode::Enter => {
                let parse = |index: usize| {
                    let text = self.rate_fields[index].text().trim();
                    if text.is_empty() {
                        Ok(None)
                    } else {
                        text.parse::<f64>()
                            .ok()
                            .filter(|rate| rate.is_finite() && *rate >= 0.0)
                            .map(Some)
                            .ok_or(())
                    }
                };
                let (Ok(input), Ok(output), Ok(cache_read), Ok(cache_write)) =
                    (parse(0), parse(1), parse(2), parse(3))
                else {
                    self.problem =
                        Some("Enter nonnegative USD rates, or leave an unknown rate blank.".into());
                    return;
                };
                let Some(model) = self.primary_model() else {
                    return;
                };
                let original = match self.model_pricing.get(&model) {
                    Some(ModelPricing::Configured { rates }) => rates.clone(),
                    _ => TokenRates::default(),
                };
                self.model_pricing.insert(
                    model,
                    ModelPricing::Configured {
                        rates: TokenRates {
                            input,
                            output,
                            cache_read,
                            cache_write,
                            ..original
                        },
                    },
                );
                self.problem = None;
                self.step = SetupStep::ReviewSettings;
            }
            _ => {
                if let Some(field) = self.rate_fields.get_mut(self.models.cursor) {
                    edit_field(field, key);
                }
            }
        }
    }

    pub(in crate::setup) fn update_price_catalogs(&self, connection: &mut ProviderConfig) {
        let mut catalogs = std::mem::take(&mut connection.options.model_price_catalogs);
        catalogs.retain(|model, catalog| {
            openrouter_quote_url(connection, model).as_ref() == Some(&catalog.url)
        });
        for model in self.model_ids().into_iter().flatten() {
            if let Some(catalog) = self.trials.price_catalog(connection, &model) {
                catalogs.insert(model, catalog);
            }
        }
        connection.options.model_price_catalogs = catalogs;
    }
}
