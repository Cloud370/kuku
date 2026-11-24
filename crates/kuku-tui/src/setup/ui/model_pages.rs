use ratatui::{layout::Rect, Frame};

use crate::theme::{ACCENT, MUTED, WARNING};

use super::super::models::{protocol_label, same_service_tier, PROTOCOLS};
use super::super::recovery::text_problem;
use super::super::state::{SetupState, SetupStep};
use super::{field, line, option, trial_usage, Settings};

pub(super) fn render_model_settings(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &mut SetupState,
) {
    match state.step {
        SetupStep::Protocol => {
            line(frame, area, row, "API type", ACCENT);
            for (index, format) in PROTOCOLS.into_iter().enumerate() {
                option(
                    frame,
                    area,
                    row,
                    protocol_label(format),
                    index == state.models.cursor,
                );
            }
        }
        SetupStep::Address => {
            line(frame, area, row, "API URL", ACCENT);
            if state.address.text().is_empty() {
                let placeholder = if state.protocol_fixed {
                    state.default_base_url()
                } else {
                    "https://your-provider.example"
                };
                line(frame, area, row, &format!("> {placeholder}"), MUTED);
                frame.set_cursor_position((area.x + 2, area.y + *row - 1));
            } else {
                field(frame, area, row, &mut state.address);
            }
            if !state.protocol_fixed && !state.recognized_request_url() {
                line(frame, area, row, "API type · Auto", MUTED);
            } else if let Ok(url) = state.request_url() {
                line(
                    frame,
                    area,
                    row,
                    &format!("Request · {}", super::super::probes::display_url(&url)),
                    MUTED,
                );
            }
            line(frame, area, row, "Paste your provider URL.", MUTED);
        }
        SetupStep::AddressKind => {
            line(frame, area, row, "Use this URL as", ACCENT);
            let entered = state.address.text().trim();
            option(
                frame,
                area,
                row,
                &format!("Exact URL · {}", super::super::probes::display_url(entered)),
                state.models.cursor == 0,
            );
            if let Ok(url) = kuku::config::resolve_request_url(state.protocol, entered, None) {
                option(
                    frame,
                    area,
                    row,
                    &format!("Base URL · {}", super::super::probes::display_url(&url)),
                    state.models.cursor == 1,
                );
            }
        }
        SetupStep::Network => {
            line(frame, area, row, "Use a proxy?", ACCENT);
            for (index, name) in ["Use proxy from environment", "No proxy", "Enter proxy URL"]
                .iter()
                .enumerate()
            {
                option(frame, area, row, name, index == state.models.cursor);
            }
        }
        SetupStep::ProxyAddress => {
            line(frame, area, row, "Proxy address", ACCENT);
            field(frame, area, row, &mut state.proxy_address);
            line(frame, area, row, "HTTP · http://127.0.0.1:7890", MUTED);
            if state.enrollment.is_none() {
                line(
                    frame,
                    area,
                    row,
                    "SOCKS5 · socks5://127.0.0.1:1080 · local DNS",
                    MUTED,
                );
            }
            line(
                frame,
                area,
                row,
                "SOCKS5H · socks5h://127.0.0.1:1080 · proxy DNS",
                MUTED,
            );
            line(frame, area, row, "Set proxy login in config.toml.", MUTED);
        }
        SetupStep::ModelSource => {
            if state.discovery.rows().is_empty() {
                line(frame, area, row, "Find models", ACCENT);
            } else {
                super::super::probes::render(
                    frame,
                    area,
                    row,
                    "Finding models",
                    &state.proxy,
                    &state.discovery,
                    0..4,
                );
            }
            for (index, name) in state.source_choices().iter().enumerate() {
                option(frame, area, row, name, index == state.models.cursor);
            }
        }
        SetupStep::Discover => {
            line(frame, area, row, "Choose models", ACCENT);
            trial_usage(frame, area, row, state);
            if state.discovery.pending() {
                let progress = super::super::probes::progress(&state.discovery);
                line(
                    frame,
                    area,
                    row,
                    &format!("Model lists · {progress}"),
                    MUTED,
                );
            }
            field(frame, area, row, &mut state.models.query);
            line(
                frame,
                area,
                row,
                &format!(
                    "{} selected · Type to search · Space to select",
                    state.models.selected.len()
                ),
                MUTED,
            );
            let visible = usize::from(area.height.saturating_sub(*row + 1));
            let start = state
                .models
                .cursor
                .saturating_sub(visible.saturating_sub(1));
            for (index, model) in state
                .models
                .filtered()
                .enumerate()
                .skip(start)
                .take(visible)
            {
                let check = if state.models.selected.contains(&model.id) {
                    "✓"
                } else {
                    " "
                };
                option(
                    frame,
                    area,
                    row,
                    &format!("[{check}] {}", model.id),
                    index == state.models.cursor,
                );
            }
        }
        SetupStep::CheckingApi => {
            super::super::probes::render(
                frame,
                area,
                row,
                "Checking API type",
                &state.proxy,
                &state.trials,
                0..state.trials.rows().len(),
            );
            if !state.trials.pending() {
                line(frame, area, row, "Enter to choose API type", ACCENT);
            }
        }
        SetupStep::TrialResults => {
            super::super::probes::render_test_table(
                frame,
                area,
                row,
                &state.proxy,
                &state.trials,
                state.models.cursor,
            );
            *row += 1;
            if let Ok(url) = state.request_url() {
                let url = super::super::probes::display_url(&url);
                let url =
                    crate::text::shorten_middle(&url, usize::from(area.width.saturating_sub(10)));
                line(frame, area, row, &format!("API URL · {url}"), MUTED);
            }
            if let Some(cost) = super::super::probes::cost_summary(&state.trials) {
                line(frame, area, row, &cost, MUTED);
            }
        }
        SetupStep::RecoveryModels | SetupStep::RecoveryActions | SetupStep::RecoveryDefault => {
            render_recovery(frame, area, row, state);
        }
        SetupStep::DefaultModel => {
            line(frame, area, row, "Choose the default model", ACCENT);
            let visible = usize::from(area.height.saturating_sub(*row + 1));
            let start = state
                .models
                .cursor
                .saturating_sub(visible.saturating_sub(1));
            for (index, model) in state
                .models
                .selected_in_catalog_order()
                .enumerate()
                .skip(start)
                .take(visible)
            {
                option(frame, area, row, model, index == state.models.cursor);
            }
        }
        SetupStep::Thinking => {
            line(frame, area, row, "Thinking", ACCENT);
            let capabilities = state.capabilities(settings);
            if let Some(test) = capabilities
                .as_ref()
                .and_then(|caps| caps.thinking_test.as_ref())
            {
                let usable = test
                    .accepted
                    .iter()
                    .filter(|level| !test.rejected.contains(level))
                    .count();
                line(
                    frame,
                    area,
                    row,
                    &format!("{usable} usable · See Model tests"),
                    MUTED,
                );
            }
            if let Some(default) = state
                .capabilities(settings)
                .and_then(|caps| caps.reasoning_default)
            {
                line(
                    frame,
                    area,
                    row,
                    &format!(
                        "Upstream default · {} ({})",
                        default.value,
                        source_label(default.source)
                    ),
                    MUTED,
                );
            }
            for (index, effort) in state.thinking_choices(settings).iter().enumerate() {
                let label = match effort.as_str() {
                    "provider-default" => "Provider default · omit effort",
                    "none" => "none · send to provider",
                    _ => effort,
                };
                option(frame, area, row, label, index == state.models.cursor);
            }
        }
        SetupStep::Service => {
            line(frame, area, row, "Service tier", ACCENT);
            let subscription = state.subscription.is_some();
            if subscription {
                let account = crate::text::shorten_middle(
                    &state.connection_label(settings),
                    area.width.into(),
                );
                line(frame, area, row, &account, MUTED);
            }
            let primary = state.primary_model();
            let listed_tiers = state
                .models
                .found
                .iter()
                .find(|model| Some(model.id.as_str()) == primary.as_deref())
                .and_then(|model| model.capabilities.service_tiers.as_deref())
                .unwrap_or_default();
            for (index, tier) in state.service_choices(settings).iter().enumerate() {
                let tier = tier.as_deref();
                let mut label = tier.map_or_else(
                    || "Provider default".to_owned(),
                    |tier| crate::text::service_tier_label(Some(tier)),
                );
                let advertised = listed_tiers
                    .iter()
                    .any(|listed| same_service_tier(Some(listed), tier));
                if subscription
                    && !advertised
                    && matches!(tier, Some("fast" | "priority" | "ultrafast"))
                {
                    label.push_str(" · Availability unconfirmed");
                }
                option(frame, area, row, &label, index == state.models.cursor);
            }
        }
        SetupStep::Output => {
            line(frame, area, row, "Agent requested output", ACCENT);
            option(frame, area, row, "Text", state.models.cursor == 0);
            option(frame, area, row, "Image", state.models.cursor == 1);
            line(
                frame,
                area,
                row,
                "Built-in runtime supports text only.",
                MUTED,
            );
        }
        SetupStep::Pricing => {
            line(frame, area, row, "Cost and rates", ACCENT);
            if let Some(model) = state.primary_model() {
                line(frame, area, row, &model, MUTED);
            }
            for (index, name) in ["Automatic", "Configured USD rates", "Usage only"]
                .iter()
                .enumerate()
            {
                option(frame, area, row, name, index == state.models.cursor);
            }
            line(
                frame,
                area,
                row,
                "Provider receipts take precedence over estimates.",
                MUTED,
            );
        }
        SetupStep::Rates => {
            line(frame, area, row, "USD per million tokens", ACCENT);
            line(
                frame,
                area,
                row,
                "Blank leaves an unknown rate. Enter moves to the next field.",
                MUTED,
            );
            for (index, name) in ["Input", "Output", "Cache read", "Cache write"]
                .iter()
                .enumerate()
            {
                let value = state.rate_fields[index].text();
                option(
                    frame,
                    area,
                    row,
                    &format!("{name} · {value}"),
                    index == state.models.cursor,
                );
            }
            option(frame, area, row, "Apply rates", state.models.cursor == 4);
            line(
                frame,
                area,
                row,
                "Duration-specific cache rates use the config file.",
                MUTED,
            );
        }
        SetupStep::Advanced => {
            line(
                frame,
                area,
                row,
                "Manual model capability overrides",
                ACCENT,
            );
            let caps = state.capabilities(settings);
            let reasoning = caps
                .as_ref()
                .and_then(|caps| caps.reasoning_source)
                .map(|source| {
                    format!(
                        "{} ({})",
                        caps.as_ref()
                            .expect("known reasoning")
                            .reasoning_efforts
                            .join(", "),
                        source_label(source)
                    )
                });
            let default = caps
                .as_ref()
                .and_then(|caps| caps.reasoning_default.as_ref())
                .map(|value| format!("{} ({})", value.value, source_label(value.source)));
            let media = |value: Option<&kuku::DeclaredCapability<Vec<String>>>| {
                value.map(|value| {
                    format!(
                        "{} ({})",
                        value.value.join(", "),
                        source_label(value.source)
                    )
                })
            };
            let values = [
                reasoning,
                default,
                media(
                    caps.as_ref()
                        .and_then(|caps| caps.input_modalities.as_ref()),
                ),
                media(
                    caps.as_ref()
                        .and_then(|caps| caps.output_modalities.as_ref()),
                ),
                media(caps.as_ref().and_then(|caps| caps.interfaces.as_ref())),
            ];
            for (index, (label, value)) in ADVANCED_LABELS.iter().zip(values).enumerate() {
                let value = value.as_deref().unwrap_or("unknown");
                option(
                    frame,
                    area,
                    row,
                    &format!("{label} · {value}"),
                    index == state.models.cursor,
                );
            }
        }
        SetupStep::Override => {
            line(
                frame,
                area,
                row,
                ADVANCED_LABELS[state.models.cursor],
                ACCENT,
            );
            line(
                frame,
                area,
                row,
                "Manual values · comma-separated · blank uses declarations",
                MUTED,
            );
            field(frame, area, row, &mut state.models.override_value);
        }
        SetupStep::Consent
        | SetupStep::ManualConfig
        | SetupStep::Connection
        | SetupStep::SubscriptionSignIn
        | SetupStep::Model
        | SetupStep::CredentialSource
        | SetupStep::Credential
        | SetupStep::Review
        | SetupStep::ReviewSettings
        | SetupStep::ConfirmStop(_) => unreachable!("handled by render_step"),
    }
}

fn render_recovery(frame: &mut Frame<'_>, area: Rect, row: &mut u16, state: &SetupState) {
    let (title, choices) = match state.step {
        SetupStep::RecoveryModels => (
            "Choose a model to fix",
            state
                .failed_models()
                .into_iter()
                .map(|model| {
                    let reason = text_problem(state.trials.text_check(&model));
                    format!("{model} · {reason}")
                })
                .collect::<Vec<_>>(),
        ),
        SetupStep::RecoveryDefault => ("Use another default", state.verified_defaults()),
        SetupStep::RecoveryActions => (
            "Fix model",
            state
                .recovery_actions()
                .iter()
                .map(|action| action.label().to_owned())
                .collect(),
        ),
        _ => unreachable!("recovery page"),
    };
    line(frame, area, row, title, ACCENT);
    if state.step == SetupStep::RecoveryActions {
        if let Some(model) = state.recovery_model.as_deref() {
            line(frame, area, row, model, MUTED);
            line(
                frame,
                area,
                row,
                text_problem(state.trials.text_check(model)),
                WARNING,
            );
        }
    }
    *row += 1;
    let visible = usize::from(area.height.saturating_sub(*row + 1));
    let start = state
        .models
        .cursor
        .saturating_sub(visible.saturating_sub(1));
    for (index, label) in choices.iter().enumerate().skip(start).take(visible) {
        option(frame, area, row, label, index == state.models.cursor);
    }
}

const ADVANCED_LABELS: [&str; 5] = [
    "Native efforts",
    "Default effort",
    "Input modalities",
    "Output modalities",
    "Advertised interfaces",
];

fn source_label(source: kuku::CapabilitySource) -> &'static str {
    match source {
        kuku::CapabilitySource::ModelList => "model list",
        kuku::CapabilitySource::ProtocolModelList => "protocol model list",
        kuku::CapabilitySource::Manual => "manual",
        kuku::CapabilitySource::ProtocolManual => "protocol manual",
        kuku::CapabilitySource::AdapterProfile => "adapter profile",
    }
}
