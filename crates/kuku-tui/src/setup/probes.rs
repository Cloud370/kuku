use kuku::adapter::{
    ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, ReasoningCheck,
    ReasoningCheckResult, RequestCost,
};
use kuku::config::ProviderProxy;
use kuku::event::ModelStopReason;
use ratatui::{layout::Rect, Frame};

use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::models::protocol_label;
use super::ui::line;

pub(super) fn visible_trials(
    batch: &ProbeBatch,
    scroll: usize,
    lines: u16,
) -> std::ops::Range<usize> {
    let rows = batch.rows();
    let height = |row: &kuku::adapter::ProbeRow| match (&row.probe, &row.progress) {
        (Probe::Thinking { .. }, _) => 1,
        (_, ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))))
            if trial.expected_colors.is_some()
                && trial.image_match == Some(false)
                && !trial.observed_colors.is_empty() =>
        {
            4
        }
        _ => 2,
    };
    let mut start = scroll.min(rows.len().saturating_sub(1));
    let mut end = start;
    let mut used = 0;
    while let Some(row) = rows.get(end).filter(|row| used + height(row) <= lines) {
        end += 1;
        used += height(row);
    }
    while end == rows.len() && start > 0 && used + height(&rows[start - 1]) <= lines {
        start -= 1;
        used += height(&rows[start]);
    }
    start..end
}

fn label_for(probe: &Probe) -> String {
    match probe {
        Probe::Catalog(target) => format!("GET {}", display_url(&target.url)),
        Probe::ApiType { model, .. } => format!("{model} · API type"),
        Probe::Quotes { model, .. } => format!("{model} · route pricing"),
        Probe::Thinking { model, check, .. } => {
            let level = match check {
                ReasoningCheck::Discover => "Find thinking values",
                ReasoningCheck::Level(level) => level,
            };
            format!("{level} · {model}")
        }
        Probe::Trial { model, check, .. } => match check {
            ModelTrial::Text { effort, .. } => format!(
                "{model} · text · {}",
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
    line(
        frame,
        area,
        row,
        &format!("{heading} · {}", progress(batch)),
        ACCENT,
    );
    if let ProviderProxy::Explicit { url, .. } = proxy {
        line(
            frame,
            area,
            row,
            &format!("Proxy · {}", display_url(url)),
            MUTED,
        );
    }
    *row += 1;
    for request in batch.rows().iter().skip(visible.start).take(visible.len()) {
        let (label, mut detail, color) = probe_detail(&request.probe, &request.progress);
        if let Probe::Thinking { .. } = &request.probe {
            let extra = match &request.progress {
                ProbeProgress::Finished(
                    _,
                    Ok(ProbeResult::Thinking(ReasoningCheckResult::Accepted(trial))),
                ) => {
                    if let Some(reported) = &trial.reported_effort {
                        format!(" · Reported {}", crate::text::single_line(reported))
                    } else if trial.observed_reasoning {
                        " · Thinking seen".into()
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };
            if matches!(request.progress, ProbeProgress::Waiting(_)) {
                detail = "Testing".into();
            }
            line(
                frame,
                area,
                row,
                &format!("{label} {detail} · {}{extra}", label_for(&request.probe)),
                color,
            );
            continue;
        }
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
        if let ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) = &request.progress {
            if let Some(expected) = trial
                .expected_colors
                .as_ref()
                .filter(|_| trial.image_match == Some(false) && !trial.observed_colors.is_empty())
            {
                let got = trial.observed_colors.join(", ");
                line(
                    frame,
                    area,
                    row,
                    &format!("  Expected {}", expected.join(", ")),
                    MUTED,
                );
                line(frame, area, row, &format!("  Got {got}"), MUTED);
            }
        }
    }
}

pub(super) fn progress(batch: &ProbeBatch) -> String {
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
    format!("{completed}/{} finished", batch.rows().len())
}

pub(super) fn review_status(batch: &ProbeBatch) -> (String, bool) {
    let progress = progress(batch);
    if batch.pending() {
        if let Some(until) = batch
            .rows()
            .iter()
            .filter_map(|row| match row.progress {
                ProbeProgress::RetryWaiting { until, sent: true } => Some(until),
                _ => None,
            })
            .min()
        {
            return (
                format!("{progress} · Retry in {}s", seconds_until(until)),
                false,
            );
        }
        return (format!("{progress} · Save stops tests"), false);
    }
    let unresolved = batch
        .rows()
        .iter()
        .filter(|row| match &row.progress {
            ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) => {
                trial.image_match == Some(false) || !trial.text_returned
            }
            ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) => {
                !evidence.trial.text_returned
            }
            ProbeProgress::Finished(_, Err(_)) => true,
            _ => false,
        })
        .count();
    if unresolved == 0 {
        (progress, false)
    } else {
        (format!("{progress} · {unresolved} to check"), true)
    }
}

pub(super) fn cost_summary(batch: &ProbeBatch) -> Option<String> {
    let (mut reported, mut estimated, mut unknown) = (None, None, 0);
    for row in batch.rows() {
        if !matches!(
            row.probe,
            Probe::Trial { .. } | Probe::ApiType { .. } | Probe::Thinking { .. }
        ) {
            continue;
        }
        let cost = match &row.progress {
            ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) => &trial.cost,
            ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) => &evidence.trial.cost,
            ProbeProgress::Finished(
                _,
                Ok(ProbeResult::Thinking(
                    ReasoningCheckResult::Accepted(trial)
                    | ReasoningCheckResult::Candidates {
                        trial: Some(trial), ..
                    },
                )),
            ) => &trial.cost,
            ProbeProgress::Finished(_, Err(_)) | ProbeProgress::Canceled { sent: true } => {
                unknown += 1;
                continue;
            }
            _ => continue,
        };
        match cost {
            RequestCost::Reported { .. } => {
                *reported.get_or_insert(0.0) += cost.usd().unwrap_or_default()
            }
            RequestCost::Estimated { .. } => {
                *estimated.get_or_insert(0.0) += cost.usd().unwrap_or_default()
            }
            RequestCost::Unknown => unknown += 1,
        }
    }
    let mut parts = Vec::new();
    if let Some(amount) = reported {
        parts.push(format!("${amount:.7} reported"));
    }
    if let Some(amount) = estimated {
        parts.push(format!("${amount:.7} estimated"));
    }
    if unknown > 0 {
        parts.push(if parts.is_empty() {
            "Unknown".into()
        } else {
            format!("{unknown} unknown")
        });
    }
    (!parts.is_empty()).then(|| format!("Cost · {}", parts.join(" · ")))
}

fn seconds_until(until: std::time::Instant) -> u64 {
    until
        .saturating_duration_since(std::time::Instant::now())
        .as_secs_f64()
        .ceil() as u64
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
        ProbeProgress::RetryWaiting { until, sent } => {
            let verb = if *sent { "Retry" } else { "Start" };
            (
                "…",
                format!("{verb} in {}s", seconds_until(*until)),
                WARNING,
            )
        }
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
                Ok(ProbeResult::ApiType(evidence)) => {
                    let api = protocol_label(evidence.format);
                    let status = if evidence.trial.text_returned {
                        "Text reply received"
                    } else if evidence.trial.observed_reasoning {
                        "Thinking only"
                    } else {
                        "No text reply"
                    };
                    let detail = format!("API found · {api} · {status}");
                    if evidence.trial.text_returned {
                        ("✓", detail, SUCCESS)
                    } else {
                        ("!", detail, WARNING)
                    }
                }
                Ok(ProbeResult::Thinking(result)) => {
                    return match result {
                        ReasoningCheckResult::Candidates {
                            values, declared, ..
                        } => (
                            "✓",
                            format!(
                                "{} {} values",
                                if *declared { "API listed" } else { "Trying" },
                                values.len()
                            ),
                            SUCCESS,
                        ),
                        ReasoningCheckResult::Accepted(_) => ("✓", "Usable".into(), SUCCESS),
                        ReasoningCheckResult::Rejected(_) => ("–", "Rejected".into(), WARNING),
                    }
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
                        Some(true) => "Image verified".into(),
                        Some(false) => {
                            let reason = if trial.stop_reason == Some(ModelStopReason::Length) {
                                "hit output limit"
                            } else if !trial.text_returned && trial.observed_reasoning {
                                "thinking only"
                            } else if !trial.text_returned {
                                "no answer"
                            } else if trial.observed_colors.is_empty() {
                                "no color names"
                            } else {
                                "colors differ"
                            };
                            format!("Image not verified · {reason}")
                        }
                        None if !trial.text_returned
                            && trial.stop_reason == Some(ModelStopReason::Length) =>
                        {
                            "Hit output limit".into()
                        }
                        None if !trial.text_returned && trial.observed_reasoning => {
                            "Thinking only · no answer".into()
                        }
                        None if !trial.text_returned => "No answer".into(),
                        None => match &trial.reported_effort {
                            Some(effort) => format!("Thinking level · {effort}"),
                            None if trial.observed_reasoning => {
                                "Thinking seen · level not reported".into()
                            }
                            None => "Text reply received".into(),
                        },
                    };
                    if trial.image_match == Some(false) || !trial.text_returned {
                        ("!", detail, WARNING)
                    } else {
                        ("✓", detail, SUCCESS)
                    }
                }
                Err(error) if matches!(probe, Probe::Thinking { .. }) => {
                    return (
                        "!",
                        format!(
                            "Unknown · {}",
                            crate::text::shorten_middle(
                                &crate::text::single_line(&error.to_string()),
                                18
                            )
                        ),
                        WARNING,
                    );
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
