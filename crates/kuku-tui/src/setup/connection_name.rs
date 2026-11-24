use kuku::config::ProviderConfig;
use url::Url;

use super::config::Settings;
use super::ConnectionChoice;

pub(super) fn from_url(address: &str) -> Option<String> {
    let url = Url::parse(address.trim()).ok()?;
    let host = url.host_str()?;
    Some(
        url.port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}")),
    )
}

fn connection_host(connection: &ProviderConfig) -> Option<String> {
    from_url(
        connection
            .options
            .endpoint_url
            .as_deref()
            .unwrap_or(&connection.base_url),
    )
}

fn previous_name(name: &str, connection: &ProviderConfig) -> Option<String> {
    let suffix = name.strip_prefix("custom")?;
    if !suffix.is_empty()
        && !suffix
            .strip_prefix('_')
            .is_some_and(|number| number.parse::<usize>().is_ok_and(|number| number >= 2))
    {
        return None;
    }
    let host = connection_host(connection)?;
    Some(format!("{host}{suffix}"))
}

pub(super) fn existing(settings: &Settings, name: &str) -> String {
    let Some(connection) = settings.resolved.provider(name) else {
        return name.into();
    };
    if let Some(label) = previous_name(name, connection) {
        if settings.resolved.providers.contains_key(&label) && label != name {
            return format!("{label} · {name}");
        }
        return label;
    }
    name.into()
}

pub(super) fn for_save(
    settings: &Settings,
    choice: &ConnectionChoice,
    connection: &ProviderConfig,
) -> String {
    let base = match choice {
        ConnectionChoice::Existing(name) => return name.clone(),
        ConnectionChoice::Subscription(_) => unreachable!("sign-in selects the saved connection"),
        ConnectionChoice::Preset(preset) if !settings.existing => {
            return preset.connection().into()
        }
        ConnectionChoice::Preset(preset) => format!("{}_setup", preset.connection()),
        ConnectionChoice::Custom => connection_host(connection).unwrap_or_else(|| "custom".into()),
    };
    let taken = |candidate: &str| {
        settings.resolved.providers.iter().any(|(name, provider)| {
            name == candidate
                || previous_name(name, provider).is_some_and(|label| label == candidate)
        })
    };
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|index| format!("{base}_{index}"))
        .find(|name| !taken(name))
        .expect("a unique connection name")
}

#[cfg(test)]
mod tests {
    use super::from_url;

    #[test]
    fn connection_names_use_only_the_host_and_non_default_port() {
        assert_eq!(
            from_url(
                "https://fixture:fixture@relay.example.invalid:443/v1/chat/completions?example=fixture"
            ),
            Some("relay.example.invalid".into())
        );
        assert_eq!(from_url("http://[::1]:8080/v1"), Some("[::1]:8080".into()));
        assert_eq!(from_url("not a URL"), None);
    }
}
