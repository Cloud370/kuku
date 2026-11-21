use std::collections::HashMap;

use kuku::adapter::{
    ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, ProbeRow, ReasoningCheck,
    ReasoningCheckResult, RequestCost,
};
use kuku::config::ProviderProxy;
use kuku::event::ModelStopReason;
use ratatui::{layout::Rect, Frame};

use crate::text::shorten_middle;
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::models::protocol_label;
use super::ui::line;

fn check_label(probe: &Probe) -> String {
    match probe {
        Probe::Catalog(target) => format!("GET {}", display_url(&target.url)),
        Probe::ApiType { .. } => "API type".into(),
        Probe::Quotes { .. } => "Route pricing".into(),
        Probe::Thinking { check, .. } => match check {
            ReasoningCheck::Discover => "Find thinking values".into(),
            ReasoningCheck::Level(level) => format!("Thinking {level}"),
        },
        Probe::Trial { check, .. } => match check {
            ModelTrial::Text {
                effort: Some(effort),
                ..
            } => format!("Text · {effort}"),
            ModelTrial::Text { .. } => "Text reply".into(),
            ModelTrial::ImageInput { .. } => "Image input".into(),
        },
    }
}

fn model_for(probe: &Probe) -> Option<&str> {
    match probe {
        Probe::ApiType { model, .. }
        | Probe::Quotes { model, .. }
        | Probe::Thinking { model, .. }
        | Probe::Trial { model, .. } => Some(model),
        Probe::Catalog(_) => None,
    }
}

fn label_for(probe: &Probe) -> String {
    let check = check_label(probe);
    model_for(probe).map_or_else(|| check.clone(), |model| format!("{check} · {model}"))
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
    render_heading(frame, area, row, heading, proxy, batch);
    *row += 1;
    for request in batch.rows().iter().skip(visible.start).take(visible.len()) {
        let (label, mut detail, color) = probe_detail(&request.probe, &request.progress);
        if let ProbeProgress::Finished(elapsed, _) = request.progress {
            detail = format!("{:.1}s · {detail}", elapsed.as_secs_f64());
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
    }
}

fn render_heading(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    heading: &str,
    proxy: &ProviderProxy,
    batch: &ProbeBatch,
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
}

pub(super) fn render_activity(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    batch: &ProbeBatch,
    room: u16,
) {
    if room == 0 {
        return;
    }
    let mut active = batch
        .rows()
        .iter()
        .filter(|request| {
            matches!(
                request.progress,
                ProbeProgress::Waiting(_) | ProbeProgress::RetryWaiting { sent: true, .. }
            )
        })
        .peekable();
    if active.peek().is_none() {
        if let Some(request) = batch.rows().iter().find(|request| {
            matches!(
                request.progress,
                ProbeProgress::RetryWaiting { sent: false, .. }
            )
        }) {
            activity_line(frame, area, row, request);
        }
        return;
    }
    for request in active
        .by_ref()
        .take(usize::from(room.saturating_sub(1).max(1)))
    {
        activity_line(frame, area, row, request);
    }
    if room > 1 {
        let other = active.count();
        let queued = batch
            .rows()
            .iter()
            .filter(|request| {
                matches!(
                    request.progress,
                    ProbeProgress::Queued | ProbeProgress::RetryWaiting { sent: false, .. }
                )
            })
            .count();
        let note = match (other, queued) {
            (0, 0) => "Save stops tests".into(),
            (0, queued) => format!("{queued} queued · Save stops tests"),
            (other, 0) => format!("{other} more running · Save stops tests"),
            (other, queued) => format!("{other} more running · {queued} queued"),
        };
        line(frame, area, row, &format!("  {note}"), MUTED);
    }
}

fn activity_line(frame: &mut Frame<'_>, area: Rect, row: &mut u16, request: &ProbeRow) {
    let (text, color) = match request.progress {
        ProbeProgress::RetryWaiting { until, sent } => {
            let action = if sent { "Retry" } else { "Start" };
            (
                format!(
                    "… {action} in {}s · {}",
                    seconds_until(until),
                    label_for(&request.probe)
                ),
                WARNING,
            )
        }
        _ => (format!("… {}", label_for(&request.probe)), ACCENT),
    };
    line(
        frame,
        area,
        row,
        &shorten_middle(&text, usize::from(area.width)),
        color,
    );
}

pub(super) fn render_test_table(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    proxy: &ProviderProxy,
    batch: &ProbeBatch,
    scroll: usize,
) {
    render_heading(frame, area, row, "Model tests", proxy, batch);
    *row += 1;
    let rows = group_by_model(batch);
    let visible = visible_tests(&rows, scroll, area.height.saturating_sub(*row + 3));
    let check_width = 23;
    let time_width = if area.width >= 78 { 8 } else { 0 };
    let check_area = Rect::new(area.x, area.y, check_width, area.height);
    let detail_area = Rect::new(
        area.x + check_width,
        area.y,
        area.width.saturating_sub(check_width + time_width),
        area.height,
    );
    let mut previous = None;
    for request in &rows[visible] {
        let model = model_for(&request.probe).unwrap_or("Connection");
        if previous != Some(model) {
            line(
                frame,
                area,
                row,
                &shorten_middle(model, usize::from(area.width)),
                ACCENT,
            );
            previous = Some(model);
        }
        let (mark, detail, color) = probe_detail(&request.probe, &request.progress);
        let check = shorten_middle(&check_label(&request.probe), usize::from(check_width - 3));
        let mut check_row = *row;
        line(
            frame,
            check_area,
            &mut check_row,
            &format!("  {check}"),
            MUTED,
        );
        if time_width > 0 {
            if let ProbeProgress::Finished(elapsed, _) = request.progress {
                let time_area =
                    Rect::new(area.right() - time_width, area.y, time_width, area.height);
                let mut time_row = *row;
                let time = format!("{:.1}s", elapsed.as_secs_f64());
                line(
                    frame,
                    time_area,
                    &mut time_row,
                    &format!("{time:>8}"),
                    MUTED,
                );
            }
        }
        let detail = shorten_middle(&format!("{mark} {detail}"), usize::from(detail_area.width));
        line(frame, detail_area, row, &detail, color);
        if let Some((expected, observed)) = image_mismatch(request) {
            for note in [
                format!("    Expected {}", expected.join(", ")),
                format!("    Got {}", observed.join(", ")),
            ] {
                line(frame, area, row, &note, MUTED);
            }
        }
    }
}

fn group_by_model(batch: &ProbeBatch) -> Vec<&ProbeRow> {
    let mut models = HashMap::new();
    let mut rows = batch.rows().iter().collect::<Vec<_>>();
    for request in &rows {
        let index = models.len();
        models.entry(model_for(&request.probe)).or_insert(index);
    }
    rows.sort_by_key(|request| {
        let check = match &request.probe {
            Probe::ApiType { .. } => 0,
            Probe::Trial {
                check: ModelTrial::Text { .. },
                ..
            } => 1,
            Probe::Trial {
                check: ModelTrial::ImageInput { .. },
                ..
            } => 2,
            Probe::Thinking { .. } => 3,
            _ => 4,
        };
        (models[&model_for(&request.probe)], check)
    });
    rows
}

fn image_mismatch(request: &ProbeRow) -> Option<(&[String; 4], &[String])> {
    let ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) = &request.progress else {
        return None;
    };
    let expected = trial
        .expected_colors
        .as_ref()
        .filter(|_| trial.image_match == Some(false) && !trial.observed_colors.is_empty())?;
    Some((expected, &trial.observed_colors))
}

fn visible_tests(rows: &[&ProbeRow], scroll: usize, lines: u16) -> std::ops::Range<usize> {
    let height = |request: &ProbeRow| 1 + 2 * u16::from(image_mismatch(request).is_some());
    let mut start = scroll.min(rows.len().saturating_sub(1));
    let mut end = start;
    let mut used = 0;
    while let Some(request) = rows.get(end) {
        let heading = end == start || model_for(&rows[end - 1].probe) != model_for(&request.probe);
        let required = height(request) + u16::from(heading);
        if used + required > lines {
            break;
        }
        end += 1;
        used += required;
    }
    while end == rows.len() && start > 0 {
        let previous = rows[start - 1];
        let heading = model_for(&rows[start].probe) != model_for(&previous.probe);
        let required = height(previous) + u16::from(heading);
        if used + required > lines {
            break;
        }
        start -= 1;
        used += required;
    }
    start..end
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
        return (progress, false);
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
            format!("Testing · {:.1}s", started.elapsed().as_secs_f64()),
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
        ProbeProgress::Finished(_, result) => match result {
            Ok(ProbeResult::Catalog(models)) if models.is_empty() => {
                ("!", "The provider returned no models".into(), WARNING)
            }
            Ok(ProbeResult::Catalog(models)) => {
                ("✓", format!("Found {} models", models.len()), SUCCESS)
            }
            Ok(ProbeResult::ApiType(evidence)) => {
                let api = protocol_label(evidence.format);
                if evidence.trial.text_returned {
                    ("✓", api.into(), SUCCESS)
                } else {
                    let status = if evidence.trial.observed_reasoning {
                        "thinking only"
                    } else {
                        "no text"
                    };
                    ("!", format!("{api} · {status}"), WARNING)
                }
            }
            Ok(ProbeResult::Thinking(result)) => match result {
                ReasoningCheckResult::Candidates {
                    values, declared, ..
                } => {
                    let count = values.len();
                    let detail = if *declared {
                        format!("API listed {count} values")
                    } else {
                        format!("{count} common values")
                    };
                    ("✓", detail, SUCCESS)
                }
                ReasoningCheckResult::Accepted(trial) => {
                    let detail = if let Some(reported) = &trial.reported_effort {
                        format!("Usable · Reported {}", crate::text::single_line(reported))
                    } else if trial.observed_reasoning {
                        "Usable · Thinking seen".into()
                    } else {
                        "Usable".into()
                    };
                    ("✓", detail, SUCCESS)
                }
                ReasoningCheckResult::Rejected(_) => ("–", "Rejected".into(), WARNING),
            },
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
                        format!("{} routes · route unknown", quotes.len()),
                        SUCCESS,
                    ),
                }
            }
            Ok(ProbeResult::Trial(trial)) => {
                let detail = match trial.image_match {
                    Some(true) => "Verified".into(),
                    Some(false) => {
                        let reason = if trial.stop_reason == Some(ModelStopReason::Length) {
                            "output limit"
                        } else if !trial.text_returned && trial.observed_reasoning {
                            "thinking only"
                        } else if !trial.text_returned {
                            "no answer"
                        } else if trial.observed_colors.is_empty() {
                            "no color names"
                        } else {
                            "colors differ"
                        };
                        format!("Not verified · {reason}")
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
                        None => "Received".into(),
                    },
                };
                if trial.image_match == Some(false) || !trial.text_returned {
                    ("!", detail, WARNING)
                } else {
                    ("✓", detail, SUCCESS)
                }
            }
            Err(error) if matches!(probe, Probe::Thinking { .. }) => (
                "!",
                format!(
                    "Unknown · {}",
                    crate::text::shorten_middle(&crate::text::single_line(&error.to_string()), 18)
                ),
                WARNING,
            ),
            Err(error) => ("!", crate::text::single_line(&error.to_string()), WARNING),
        },
    }
}
