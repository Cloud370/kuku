use std::collections::BTreeMap;
use std::time::Duration;

use kuku::config::ModelRef;
use kuku_client::local::SESSION_PROTOCOL_VERSION;
use kuku_client::{Connection, ConnectionOptions, Error, JobStatus, ModelCatalog, SessionStatus};

use crate::menu::{Choice, ChoiceKind};

use super::{Search, busy, job_choice, value_choice};

const SEARCH_DELAY: Duration = Duration::from_millis(70);

pub(super) async fn find(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    query: Search,
) -> Result<(Vec<Choice>, String), Error> {
    if matches!(query, Search::Server) {
        return Ok((Vec::new(), service_status(options, connection).await?));
    }
    let connection = connection.ok_or_else(|| Error::Options("Reconnect with Ctrl+R".into()))?;
    let client = &connection.client;
    let workspace = &connection.workspace;
    let choices = match query {
        Search::Skills(query) => {
            tokio::time::sleep(SEARCH_DELAY).await;
            let query = query.trim_start_matches('/').trim_start_matches("skill:");
            client
                .skills(workspace, Some(query))
                .await?
                .into_iter()
                .map(|skill| {
                    Choice::new(
                        format!("/skill:{}", skill.name),
                        format!("Skill · {}", skill.description),
                        ChoiceKind::Skill(skill.name),
                    )
                })
                .collect()
        }
        Search::Paths(query) => {
            tokio::time::sleep(SEARCH_DELAY).await;
            client
                .workspace_files(workspace, &query)
                .await?
                .into_iter()
                .map(|path| Choice::new(&path, "Workspace path", ChoiceKind::Path(path.clone())))
                .collect()
        }
        Search::Sessions(query) => {
            tokio::time::sleep(SEARCH_DELAY).await;
            client
                .sessions(Some(workspace), Some(&query))
                .await?
                .into_iter()
                .map(|session| {
                    let state = if !session.waiting_agents.is_empty() {
                        "Waiting"
                    } else {
                        match session.status {
                            SessionStatus::Active => "Active",
                            SessionStatus::Done => "Done",
                            SessionStatus::Interrupted => "Interrupted",
                        }
                    };
                    let title = if session.title.trim().is_empty() {
                        session.session_id.clone()
                    } else {
                        session.title
                    };
                    Choice::new(
                        &title,
                        format!(
                            "{} · {state} · {} · {} Turns",
                            session.session_id.chars().take(8).collect::<String>(),
                            session.mtime,
                            session.turn_count
                        ),
                        ChoiceKind::Session {
                            id: session.session_id,
                            title: title.clone(),
                            exists: true,
                        },
                    )
                })
                .collect()
        }
        Search::Models(route) => {
            return Ok(model_choices(client.models().await?, route.as_ref()));
        }
        Search::Values(target) => {
            let choices: Vec<_> = client
                .named_values(&target)
                .await?
                .into_iter()
                .map(value_choice)
                .collect();
            let header = if choices.is_empty() {
                "No saved values · /save to name pasted text".into()
            } else {
                String::new()
            };
            return Ok((choices, header));
        }
        Search::Jobs(target) => {
            let mut jobs = client.jobs(&target).await?;
            // Keep active work near the top even when numerous newer Jobs
            // have already settled. Retain the service order within a group.
            jobs.sort_by_key(|job| job.status != JobStatus::Running);
            let choices: Vec<_> = jobs.into_iter().map(job_choice).collect();
            let header = if choices.is_empty() {
                "No background jobs for this Agent"
            } else {
                "Select a job to see available actions"
            };
            return Ok((choices, header.into()));
        }
        Search::Server => unreachable!("handled above"),
    };
    Ok((choices, String::new()))
}

fn model_choices(catalog: ModelCatalog, current: Option<&ModelRef>) -> (Vec<Choice>, String) {
    let mut choices = Vec::new();
    let default = &catalog.default_model;
    let labels = subscription_labels(&catalog.connections);
    for model in catalog.models {
        let account = labels.get(model.connection.as_str());
        let mut detail = account.unwrap_or(&model.connection).clone();
        let is_model = |id: &ModelRef| id.connection == model.connection && id.model == model.model;
        let selected = current.is_some_and(is_model);
        if selected {
            detail.push_str(" · Current");
        }
        if is_model(default) {
            detail.push_str(" · New Sessions default");
        }
        if let Some(purpose) = model
            .purpose
            .as_ref()
            .filter(|purpose| account.is_none() && !purpose.is_empty() && *purpose != &model.model)
        {
            detail.push_str(&format!(" · {purpose}"));
        }
        if let Some(error) = &model.error {
            let message = if account.is_some() {
                error.message.replace(&model.connection, "subscription")
            } else {
                error.message.clone()
            };
            detail.push_str(&format!(" · {message}"));
        }
        choices.push(Choice::new(
            model.model.clone(),
            detail,
            ChoiceKind::ModelRoute {
                model: Box::new(model),
                current: selected,
            },
        ));
    }
    let connection = labels
        .get(default.connection.as_str())
        .unwrap_or(&default.connection);
    (
        choices,
        format!("New Sessions default · {} · {connection}", default.model),
    )
}

pub(super) async fn send_route_labels(
    connection: Option<&Connection>,
    names: &[String; 2],
) -> [Option<String>; 2] {
    let Some(connection) = connection else {
        return [None, None];
    };
    let Ok(catalog) = connection.client.models().await else {
        return [None, None];
    };
    let labels = subscription_labels(&catalog.connections);
    std::array::from_fn(|index| {
        let name = &names[index];
        catalog
            .connections
            .iter()
            .find(|connection| connection.name == *name)
            .map(|connection| {
                labels
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or_else(|| connection.name.clone())
            })
            .or_else(|| (index == 0).then(|| "Removed connection".to_owned()))
    })
}

pub(crate) fn subscription_labels(
    connections: &[kuku_client::ConnectionSummary],
) -> BTreeMap<&str, String> {
    let mut counts = BTreeMap::new();
    for label in connections
        .iter()
        .filter_map(|connection| connection.subscription_label.as_deref())
    {
        *counts.entry(label).or_insert(0) += 1;
    }
    let mut numbers = BTreeMap::new();
    connections
        .iter()
        .filter_map(|connection| {
            let label = connection.subscription_label.as_deref()?;
            let label = if counts[label] > 1 {
                let number = numbers.entry(label).or_insert(0);
                *number += 1;
                format!("{label} · #{number}")
            } else {
                label.into()
            };
            Some((connection.name.as_str(), label))
        })
        .collect()
}

async fn service_status(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<String, Error> {
    let workspace =
        connection.map_or_else(|| options.workspace(), |item| Ok(item.workspace.clone()))?;
    if options.remote.is_some() {
        let client = options
            .remote_connection()?
            .expect("explicit remote client");
        let health = client.health().await?;
        return Ok(format!(
            "Remote {}\nWorkspace {}\nBuild {} · service protocol {} · client protocol {SESSION_PROTOCOL_VERSION}",
            client.endpoint(),
            workspace.display(),
            health.version,
            health.protocol_version
        ));
    }
    let service = options.local_service()?;
    if let Some(record) = service.status().await? {
        let status = record.status();
        return Ok(format!(
            "Local {}\nWorkspace {}\nConfig {}\nBuild {} · service protocol {} · client protocol {SESSION_PROTOCOL_VERSION}\n{}",
            record.endpoint(),
            workspace.display(),
            status.config_path.as_deref().unwrap_or("Unavailable"),
            status.version,
            status.protocol_version,
            busy(&status.busy, status.draining)
        ));
    }
    Ok(format!(
        "Local service stopped\nWorkspace {}\nData {}",
        workspace.display(),
        service.data_dir().display()
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ModelRef, model_choices};
    use crate::menu::ChoiceKind;

    #[test]
    fn subscription_routes_use_model_and_masked_email_for_display() {
        let catalog = serde_json::from_value(json!({
            "default_model":{"connection":"internal-name","model":"gpt-fixture"},
            "connections": [
                {"name":"internal-name","protocol":"openai-responses","subscription_label":"ChatGPT · a***@e***"},
                {"name":"other-internal-name","protocol":"openai-responses","subscription_label":"ChatGPT · a***@e***"}
            ],
            "models": [
                {"connection":"internal-name","model":"gpt-fixture"},
                {"connection":"other-internal-name","model":"gpt-fixture"}
            ]
        })).unwrap();
        let current = ModelRef::new("other-internal-name", "gpt-fixture");
        let (choices, _) = model_choices(catalog, Some(&current));
        assert_eq!(choices[0].label, "gpt-fixture");
        assert_eq!(
            choices[0].detail,
            "ChatGPT · a***@e*** · #1 · New Sessions default"
        );
        assert_eq!(choices[1].detail, "ChatGPT · a***@e*** · #2 · Current");
        assert!(
            matches!(&choices[0].kind, ChoiceKind::ModelRoute {model, ..} if model.connection == "internal-name")
        );
    }
}
