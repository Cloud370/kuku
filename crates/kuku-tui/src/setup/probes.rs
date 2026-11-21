use kuku::adapter::{ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, RequestCost};
use kuku::config::ProviderProxy;
use ratatui::{layout::Rect, Frame};

use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::ui::line;

fn label_for(probe: &Probe) -> String {
    match probe {
        Probe::Catalog(target) => format!("GET {}", display_url(&target.url)),
        Probe::Quotes { model, .. } => format!("{model} · route pricing"),
        Probe::Trial { model, check, .. } => match check {
            ModelTrial::Text { effort, .. } => format!(
                "{model} · thinking {}",
                effort.as_deref().unwrap_or("provider default")
            ),
            ModelTrial::ImageInput { .. } => format!("{model} · image input"),
        },
    }
}

pub(super) fn display_url(value: &str) -> String {
    url::Url::parse(value).map_or_else(
        |_| "invalid endpoint".into(),
        |mut url| {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.into()
        },
    )
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    heading: &str,
    proxy: &ProviderProxy,
    batch: &ProbeBatch,
    visible: std::ops::Range<usize>,
) {
    let completed = batch
        .rows()
        .iter()
        .filter(|row| {
            matches!(
                row.progress,
                ProbeProgress::Finished(..) | ProbeProgress::Canceled { .. }
            )
        })
        .count();
    line(
        frame,
        area,
        row,
        &format!("{heading} · {completed}/{} finished", batch.rows().len()),
        ACCENT,
    );
    let network = match proxy {
        ProviderProxy::Environment => "Follow proxy environment".to_owned(),
        ProviderProxy::Direct => "Direct connection".to_owned(),
        ProviderProxy::Explicit { url, .. } => display_url(url),
    };
    line(frame, area, row, &format!("Network · {network}"), MUTED);
    for request in batch.rows().iter().skip(visible.start).take(visible.len()) {
        let (label, mut detail, color) = probe_detail(&request.probe, &request.progress);
        if let Some(models) = &request.cached_models {
            if !matches!(request.progress, ProbeProgress::Finished(_, Ok(_))) {
                detail = format!("{} cached models · {detail}", models.len());
            }
        }
        line(
            frame,
            area,
            row,
            &format!("{label} {}", label_for(&request.probe)),
            color,
        );
        line(frame, area, row, &format!("  {detail}"), MUTED);
    }
}

pub(super) fn amounts(batch: &ProbeBatch) -> (f64, f64, usize) {
    let mut totals = (0.0, 0.0, 0);
    for row in batch.rows() {
        if !matches!(row.probe, kuku::adapter::Probe::Trial { .. }) {
            continue;
        }
        match &row.progress {
            ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) => match trial.cost {
                RequestCost::Reported { .. } => totals.0 += trial.cost.usd().unwrap_or_default(),
                RequestCost::Estimated { .. } => totals.1 += trial.cost.usd().unwrap_or_default(),
                RequestCost::Unknown => totals.2 += 1,
            },
            ProbeProgress::Finished(_, Err(_)) | ProbeProgress::Canceled { sent: true } => {
                totals.2 += 1
            }
            _ => {}
        }
    }
    totals
}

fn probe_detail(
    probe: &Probe,
    progress: &ProbeProgress,
) -> (&'static str, String, ratatui::style::Color) {
    match progress {
        ProbeProgress::Queued => ("·", "Queued".into(), MUTED),
        ProbeProgress::Waiting(started) => (
            "…",
            format!(
                "Waiting for response · {:.1}s",
                started.elapsed().as_secs_f64()
            ),
            ACCENT,
        ),
        ProbeProgress::Canceled { sent } => (
            "–",
            if *sent {
                "Stopped · request may have been sent"
            } else {
                "Canceled before sending"
            }
            .into(),
            MUTED,
        ),
        ProbeProgress::Finished(elapsed, result) => {
            let (icon, detail, color) = match result {
                Ok(ProbeResult::Catalog(models)) if models.is_empty() => {
                    ("!", "The provider returned no models".into(), WARNING)
                }
                Ok(ProbeResult::Catalog(models)) => {
                    ("✓", format!("Found {} models", models.len()), SUCCESS)
                }
                Ok(ProbeResult::Quotes(quotes)) => {
                    let example = match probe {
                        Probe::Quotes {
                            connection,
                            service_tier,
                            ..
                        } => kuku::adapter::route_quote_for_display(
                            quotes,
                            connection.options.routing.as_ref(),
                            service_tier.as_deref(),
                        ),
                        _ => None,
                    };
                    match example {
                        Some(quote) => {
                            let pricing = quote.rates.input.zip(quote.rates.output).map_or_else(
                                || "rates unknown".into(),
                                |(input, output)| format!("${input}/${output} per 1M in/out"),
                            );
                            (
                                "✓",
                                format!("{} · {pricing} · {} routes", quote.tag, quotes.len()),
                                SUCCESS,
                            )
                        }
                        None => (
                            "✓",
                            format!("{} routes · author route unconfirmed", quotes.len()),
                            SUCCESS,
                        ),
                    }
                }
                Ok(ProbeResult::Trial(trial)) => {
                    let detail = match trial.image_match {
                        Some(true) => "Color-grid read passed".into(),
                        Some(false) if trial.observed_colors.is_empty() => {
                            "Color-grid reply contained no usable colors".into()
                        }
                        Some(false) => format!(
                            "Color-grid read did not match · got {}",
                            trial.observed_colors.join(", ")
                        ),
                        None => match &trial.reported_effort {
                            Some(effort) => format!("Upstream reported effort {effort}"),
                            None if trial.observed_reasoning => {
                                "Reasoning observed · effort unknown".into()
                            }
                            None if trial.text_returned => {
                                "Text reply received · effort unknown".into()
                            }
                            None => "Response contained no text reply".into(),
                        },
                    };
                    let cost = match &trial.cost {
                        RequestCost::Reported { .. } => {
                            format!("reported ${:.7}", trial.cost.usd().unwrap_or_default())
                        }
                        RequestCost::Estimated { .. } => {
                            format!("estimated ${:.7}", trial.cost.usd().unwrap_or_default())
                        }
                        RequestCost::Unknown => "cost unknown".into(),
                    };
                    if trial.image_match == Some(false)
                        || (trial.image_match.is_none()
                            && !trial.text_returned
                            && !trial.observed_reasoning)
                    {
                        ("!", format!("{detail} · {cost}"), WARNING)
                    } else {
                        ("✓", format!("{detail} · {cost}"), SUCCESS)
                    }
                }
                Err(error) => ("!", crate::text::single_line(&error.to_string()), WARNING),
            };
            (
                icon,
                format!("{:.1}s · {detail}", elapsed.as_secs_f64()),
                color,
            )
        }
    }
}
