use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use kuku::adapter::{subscription_account_info, SubscriptionAccountInfo, SubscriptionStatus};
use kuku::config::{
    Config, ProviderConfig, ProviderFormat, ProviderProxy, SubscriptionAccount, SubscriptionChannel,
};
use kuku_client::ConnectionOptions;
use toml_edit::{value, Item, Value};

use super::config::{entry, parse_service_config, set_connection_proxy, write_config, Settings};
use super::ConnectionChoice;

pub(super) mod enrollment;

/// Local subscription settings using the same configuration snapshot as provider setup.
pub struct SubscriptionSettings {
    settings: Settings,
}

impl SubscriptionSettings {
    /// Load the active local provider settings without launching or replacing the service.
    pub async fn load(options: ConnectionOptions) -> io::Result<Self> {
        Ok(Self {
            settings: Settings::load(options).await?,
        })
    }

    /// Return the active service's data directory after verifying the private storage boundary.
    pub fn private_data_dir(&self) -> io::Result<&Path> {
        self.settings.private_data_dir()
    }

    /// List subscription connections by their configured model connection names.
    pub fn connections(
        &self,
    ) -> impl Iterator<Item = (&str, &SubscriptionAccount, &ProviderConfig)> {
        self.settings.subscription_connections()
    }

    /// Find the model connection associated with a named subscription account.
    pub fn connection(
        &self,
        channel: SubscriptionChannel,
        name: &str,
    ) -> Option<(&str, &SubscriptionAccount, &ProviderConfig)> {
        self.settings.subscription_connection(channel, name)
    }

    /// Save a subscription connection or change its proxy through guarded config publication.
    pub fn save_connection(
        &self,
        account: &SubscriptionAccount,
        proxy: &ProviderProxy,
    ) -> kuku::Result<String> {
        self.settings.save_subscription_connection(account, proxy)
    }
}

impl Settings {
    pub(super) fn private_data_dir(&self) -> io::Result<&Path> {
        if !self.can_store_key() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                self.storage_problem(),
            ));
        }
        Ok(&self.data_dir)
    }

    fn subscription_connections(
        &self,
    ) -> impl Iterator<Item = (&str, &SubscriptionAccount, &ProviderConfig)> {
        self.resolved
            .providers
            .iter()
            .filter_map(|(name, connection)| {
                Some((
                    name.as_str(),
                    connection.options.subscription.as_ref()?,
                    connection,
                ))
            })
    }

    pub(super) fn subscription_connection(
        &self,
        channel: SubscriptionChannel,
        name: &str,
    ) -> Option<(&str, &SubscriptionAccount, &ProviderConfig)> {
        self.subscription_connections().find(|(_, account, _)| {
            account.channel() == channel && account.name().is_ok_and(|saved| saved == name)
        })
    }

    pub(super) fn save_subscription_connection(
        &self,
        account: &SubscriptionAccount,
        proxy: &ProviderProxy,
    ) -> kuku::Result<String> {
        let name = account.name()?;
        if account != &SubscriptionAccount::new(self.private_data_dir()?, account.channel(), name)?
        {
            return Err(kuku::Error::InvalidArgument(
                "subscription account belongs to a different data directory".into(),
            ));
        }
        let existing = self.subscription_connection(account.channel(), name);
        let mut document = self.document.clone();
        let connection_name = if let Some((connection_name, previous, _)) = existing {
            if previous != account {
                return Err(kuku::Error::InvalidArgument(
                    "subscription account belongs to a different data directory".into(),
                ));
            }
            connection_name.to_owned()
        } else {
            let connection_name = format!("{}-{name}", account.channel().as_str());
            if self.raw.provider.contains_key(&connection_name) {
                return Err(kuku::Error::InvalidArgument(format!(
                    "model connection '{connection_name}' already exists"
                )));
            }
            let connection = entry(&mut document, "provider", &connection_name)?;
            let (format, base) = match account.channel() {
                SubscriptionChannel::ChatGpt => {
                    (ProviderFormat::OpenAiResponses, "https://api.openai.com/v1")
                }
            };
            connection.insert("format", value(format.as_str()));
            connection.insert("base_url", value(base));
            let account = toml_edit::ser::to_document(account).map_err(|_| {
                kuku::Error::InvalidArgument(
                    "could not encode subscription account reference".into(),
                )
            })?;
            connection.insert(
                "subscription",
                Item::Value(Value::InlineTable(account.into_table().into_inline_table())),
            );
            connection_name
        };
        let connection = entry(&mut document, "provider", &connection_name)?;
        set_connection_proxy(connection, proxy)?;
        let output = document.to_string();
        parse_service_config(&output, &self.data_dir, &|name| std::env::var(name))?.resolve()?;
        write_config(self, output.as_bytes(), || {})?;
        Ok(connection_name)
    }

    pub(super) fn subscription_for(
        &self,
        choice: &ConnectionChoice,
    ) -> Option<&SubscriptionAccount> {
        let ConnectionChoice::Existing(name) = choice else {
            return None;
        };
        self.resolved
            .providers
            .get(name)?
            .options
            .subscription
            .as_ref()
    }

    pub(super) fn subscription_available(&self, choice: &ConnectionChoice) -> bool {
        let ConnectionChoice::Existing(name) = choice else {
            return false;
        };
        self.subscription_info
            .get(name)
            .is_some_and(|info| info.status == SubscriptionStatus::SignedIn)
    }
}

pub(super) fn account_info(config: &Config) -> BTreeMap<String, SubscriptionAccountInfo> {
    config
        .providers
        .iter()
        .filter_map(|(name, provider)| {
            let account = provider.options.subscription.as_ref()?;
            subscription_account_info(account)
                .ok()
                .map(|info| (name.clone(), info))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use kuku::adapter::{
        DiscoveredModel, ModelTrial, Probe, SubscriptionAccountInfo, SubscriptionStatus,
    };
    use kuku::config::{
        generate_default, load_config, ModelCapabilityDeclaration, ProviderProxy,
        SubscriptionAccount, SubscriptionChannel,
    };
    use kuku_client::ConnectionOptions;
    use ratatui::{
        backend::TestBackend,
        layout::{Position, Rect},
        Terminal,
    };

    use super::SubscriptionSettings;
    use crate::setup::config::{save, Settings};
    use crate::setup::state::{ReviewAction, SetupIntent, SetupState, SetupStep};
    use crate::setup::{ui, ConnectionChoice, SetupMode};

    #[tokio::test]
    async fn subscription_preset_displays_full_authorization_and_hides_an_invalid_callback() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("workspace")).unwrap();
        let mut settings = Settings::load(ConnectionOptions {
            workspace: Some(root.path().join("workspace")),
            data_dir: Some(root.path().join("data")),
            ..ConnectionOptions::default()
        })
        .await
        .unwrap();
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut state = SetupState::new(&settings);
        let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut state, SetupMode::Init))
            .unwrap();
        assert!(!terminal
            .backend()
            .to_string()
            .contains("Tests may cost money"));

        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut state, SetupMode::Init))
            .unwrap();
        let picker = terminal.backend().to_string();
        assert!(picker.contains("Preset · ChatGPT subscription"));
        assert!(!picker.contains("Add a ChatGPT account with"));

        state.handle(key(KeyCode::Up), &settings, SetupMode::Init);
        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        assert_eq!(SetupStep::Network, state.step);
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut state, SetupMode::Init))
            .unwrap();
        let network = terminal.backend().to_string();
        assert!(network.contains("› Use proxy from environment"));
        assert!(network.contains("No proxy"));
        assert!(network.contains("Enter proxy URL"));

        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        assert_eq!(SetupStep::SubscriptionSignIn, state.step);
        assert_eq!(ProviderProxy::Environment, state.proxy);
        state.handle(key(KeyCode::Esc), &settings, SetupMode::Init);
        state.handle(key(KeyCode::Down), &settings, SetupMode::Init);
        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        assert_eq!(SetupStep::SubscriptionSignIn, state.step);
        assert_eq!(ProviderProxy::Direct, state.proxy);
        state.handle(key(KeyCode::Esc), &settings, SetupMode::Init);
        state.handle(key(KeyCode::Up), &settings, SetupMode::Init);
        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        assert_eq!(SetupStep::ProxyAddress, state.step);
        state.handle(
            Event::Paste(format!("http://{}", proxy.local_addr().unwrap())),
            &settings,
            SetupMode::Init,
        );
        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        assert_eq!(SetupStep::SubscriptionSignIn, state.step);
        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        poll_registration(&mut state, &mut settings).await;

        let url = ui::authorization_link(&state, Rect::new(0, 0, 60, 20)).unwrap();
        let url = url.rows().map(|(_, text)| text).collect::<String>();
        let url = url::Url::parse(&url).unwrap();
        assert_eq!(Some("auth.openai.com"), url.host_str());
        let parameters = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!("kuku", parameters["agent_name_hint"]);
        let bad_callback = format!(
            "{}?code=fixture-callback-secret&state=wrong",
            parameters["redirect_uri"]
        );
        let url_rows = u16::try_from(url.as_str().len().div_ceil(60)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut state, SetupMode::Init))
            .unwrap();
        let prompt = "Paste callback URL > ";
        assert!(terminal.backend().to_string().contains(prompt.trim_end()));
        assert_eq!(
            Position::new(prompt.len() as u16, 5 + url_rows),
            terminal.get_cursor_position().unwrap()
        );
        state.handle(Event::Paste(bad_callback), &settings, SetupMode::Init);
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut state, SetupMode::Init))
            .unwrap();
        let screen = terminal.backend().to_string();
        assert!(screen.contains("Callback received"));
        assert!(screen.contains("[Open authorization link]  [Ctrl+Y Copy URL]"));
        assert!(screen.contains("──── Authorization URL ────"));
        assert!(screen.contains("──── End authorization URL ────"));
        let buffer = terminal.backend().buffer();
        for row in [2, 3 + url_rows] {
            assert!((0..60).all(|column| buffer[(column, row)].symbol() == " "));
        }
        assert!(!screen.contains("fixture-callback-secret"));
        state.handle(key(KeyCode::Enter), &settings, SetupMode::Init);
        poll_registration(&mut state, &mut settings).await;
        assert!(state
            .problem
            .as_deref()
            .unwrap()
            .contains("does not match this sign-in"));
        assert!(!settings.config_path.exists());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), proxy.accept())
                .await
                .is_err()
        );
    }

    async fn poll_registration(state: &mut SetupState, settings: &mut Settings) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !state.poll_subscription(settings) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn registration_and_proxy_changes_preserve_existing_model_settings() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("project");
        let data_dir = root.path().join("data");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&data_dir).unwrap();
        let path = data_dir.join("config.toml");
        fs::write(&path, generate_default()).unwrap();
        let options = ConnectionOptions {
            workspace: Some(workspace),
            data_dir: Some(data_dir),
            ..ConnectionOptions::default()
        };
        let settings = SubscriptionSettings::load(options.clone()).await.unwrap();
        let account = SubscriptionAccount::new(
            settings.private_data_dir().unwrap(),
            SubscriptionChannel::ChatGpt,
            "work",
        )
        .unwrap();
        let original = load_config(&path).unwrap();
        let first = ProviderProxy::Explicit {
            url: "http://127.0.0.1:9100".into(),
            auth: None,
        };
        let second = ProviderProxy::Explicit {
            url: "socks5h://127.0.0.1:9101".into(),
            auth: None,
        };
        assert_eq!(
            "chatgpt-work",
            settings.save_connection(&account, &first).unwrap()
        );
        let live = SubscriptionSettings::load(options.clone()).await.unwrap();
        assert_eq!(
            "chatgpt-work",
            live.save_connection(&account, &second).unwrap()
        );
        let current = load_config(&path).unwrap();
        assert_eq!(original.model, current.model);
        assert_eq!(original.default_model, current.default_model);
        for (name, provider) in original.provider {
            assert_eq!(provider, current.provider[&name]);
        }
        let subscription = &current.provider["chatgpt-work"];
        assert_eq!(Some(&account), subscription.options.subscription.as_ref());
        assert_eq!(second, subscription.options.proxy);
        assert!(subscription.api_key.is_empty());
        for mode in [ProviderProxy::Direct, ProviderProxy::Environment] {
            let settings = SubscriptionSettings::load(options.clone()).await.unwrap();
            settings.save_connection(&account, &mode).unwrap();
            let current = load_config(&path).unwrap();
            assert_eq!(mode, current.provider["chatgpt-work"].options.proxy);
        }

        let mut settings = Settings::load(options.clone()).await.unwrap();
        settings.subscription_info.insert(
            "chatgpt-work".into(),
            SubscriptionAccountInfo {
                masked_email: Some("a***@e***".into()),
                status: SubscriptionStatus::SignedIn,
            },
        );
        let mut add = SetupState::for_intent(&settings, SetupIntent::Add);
        let connection = ConnectionChoice::Existing("chatgpt-work".into());
        add.selected = add
            .choices
            .iter()
            .position(|choice| *choice == connection)
            .unwrap();
        add.load_selection(&settings);
        add.authorized = true;
        add.models.add_manual("account-model".into());
        add.open_service(&settings);
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut add, SetupMode::Session))
            .unwrap();
        let screen = terminal.backend().to_string();
        assert!(screen.contains("ChatGPT · work · a***@e***"));
        assert!(screen.contains("Fast · Availability unconfirmed"));
        assert!(screen.contains("Ultra Fast · Availability unconfirmed"));
        select_service(&mut add, &settings, "fast");
        assert!(add.trials.rows().is_empty());
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut add, SetupMode::Session))
            .unwrap();
        let screen = terminal.backend().to_string();
        assert!(screen.contains("account-model"));
        assert!(!screen.contains("Thinking"));
        let saved = save(&settings, &add).unwrap();
        let route = &saved.tiers["account-model"];
        assert_eq!(Some("fast"), route.service_tier.as_deref());
        assert_eq!("chatgpt-work", route.provider);

        add.models.discovered_from = Some(add.connection(&settings).unwrap());
        add.models.found = vec![DiscoveredModel {
            id: "account-model".into(),
            name: None,
            capabilities: ModelCapabilityDeclaration {
                service_tiers: Some(vec!["priority".into()]),
                ..ModelCapabilityDeclaration::default()
            },
        }];
        select_service(&mut add, &settings, "priority");
        assert!(add.trials.rows().is_empty());
        add.open_service(&settings);
        terminal
            .draw(|frame| ui::render(frame, &settings, &mut add, SetupMode::Session))
            .unwrap();
        let screen = terminal.backend().to_string();
        assert!(screen.contains("Fast"));
        assert!(!screen.contains("Availability unconfirmed"));
        assert!(!screen.contains("Ultra Fast"));

        add.step = SetupStep::ReviewSettings;
        add.review_selection = add
            .review_choices(SetupMode::Session)
            .iter()
            .position(|item| *item == ReviewAction::TestModel)
            .unwrap();
        add.handle(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &settings,
            SetupMode::Session,
        );
        assert_eq!(SetupStep::TrialResults, add.step);
        let [trial] = add.trials.rows() else {
            panic!("expected one explicit trial")
        };
        assert!(
            matches!(&trial.probe, Probe::Trial { model, check: ModelTrial::Text { service_tier: Some(tier), .. }, .. } if model == "account-model" && tier == "priority")
        );
        add.trials.stop();

        let settings = Settings::load(options).await.unwrap();
        let mut edit = SetupState::for_intent(&settings, SetupIntent::Edit("account-model".into()));
        edit.models.add_manual("updated-model".into());
        select_service(&mut edit, &settings, "ultrafast");
        let saved = save(&settings, &edit).unwrap();
        let route = &saved.tiers["account-model"];
        assert_eq!("updated-model", route.model);
        assert_eq!(Some("ultrafast"), route.service_tier.as_deref());
        assert_eq!(
            Some(&account),
            saved.providers["chatgpt-work"]
                .options
                .subscription
                .as_ref()
        );
    }

    fn select_service(state: &mut SetupState, settings: &Settings, tier: &str) {
        state.open_service(settings);
        state.models.cursor = state
            .service_choices(settings)
            .iter()
            .position(|option| option.as_deref() == Some(tier))
            .unwrap();
        state.choose_service(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), settings);
    }
}
