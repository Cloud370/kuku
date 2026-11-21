use kuku::config::ProviderConfig;

use super::SetupState;

impl SetupState {
    pub(in crate::setup) fn update_capabilities(&self, connection: &mut ProviderConfig) {
        self.update_model_declarations(connection);
        for model in self.model_ids().into_iter().flatten() {
            if let Some(tiers) = self.trials.advertised_service_tiers(connection, &model) {
                connection
                    .options
                    .model_capabilities
                    .entry(model.clone())
                    .or_default()
                    .discovered
                    .get_or_insert_with(Default::default)
                    .service_tiers = Some(tiers);
            }
            let Some(test) = self.trials.thinking_test(connection, &model) else {
                continue;
            };
            let entry = connection
                .options
                .model_capabilities
                .entry(model)
                .or_default();
            if let Some(previous) = &mut entry.thinking_test {
                previous.merge(test);
            } else {
                entry.thinking_test = Some(test);
            }
        }
    }

    pub(super) fn update_model_declarations(&self, connection: &mut ProviderConfig) {
        if self
            .models
            .discovered_from
            .as_ref()
            .is_some_and(|original| {
                original.format == connection.format
                    && original.base_url == connection.base_url
                    && original.api_key == connection.api_key
                    && original.options.endpoint_url == connection.options.endpoint_url
                    && original.options.models_url == connection.options.models_url
                    && original.options.proxy == connection.options.proxy
            })
        {
            for model in &self.models.found {
                if !self.models.selected.contains(&model.id)
                    && !connection
                        .options
                        .model_capabilities
                        .contains_key(&model.id)
                    && self.model.text() != model.id
                {
                    continue;
                }
                let entry = connection
                    .options
                    .model_capabilities
                    .entry(model.id.clone())
                    .or_default();
                let mut declaration = model.capabilities.clone();
                if declaration.service_tiers.is_none() {
                    declaration.service_tiers = entry
                        .discovered
                        .as_ref()
                        .and_then(|previous| previous.service_tiers.clone());
                }
                entry.discovered = Some(declaration);
            }
        }
        for (id, manual) in &self.models.overrides {
            connection
                .options
                .model_capabilities
                .entry(id.clone())
                .or_default()
                .manual = Some(manual.clone());
        }
    }
}
