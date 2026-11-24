use std::io::{self, IsTerminal, Write};

use kuku::adapter::{
    discover_models, sign_out_subscription, subscription_account_info, SubscriptionLogin,
    SubscriptionLoginProgress, SubscriptionStatus,
};
use kuku::config::{
    ProviderConfig, ProviderProxy, ProxyAuth, SubscriptionAccount, SubscriptionChannel,
};
use kuku_tui::SubscriptionSettings;

use crate::cli_args::{ConnectionArgs, SubscriptionAuthCommand, SubscriptionProxyArgs};

use super::CliResult;

pub(super) async fn run(command: SubscriptionAuthCommand, target: &ConnectionArgs) -> CliResult {
    let settings = SubscriptionSettings::load(target.options()).await?;
    match command {
        SubscriptionAuthCommand::Login {
            channel,
            name,
            proxy,
        } => {
            login(
                &settings,
                channel.into(),
                name.as_deref(),
                proxy_settings(proxy),
            )
            .await
        }
        SubscriptionAuthCommand::List => {
            for (name, account, _) in settings.connections() {
                let info = subscription_account_info(account)?;
                let email = info.masked_email.as_deref().unwrap_or("—");
                println!(
                    "{}\t{}\t{}\t{}",
                    account.channel().as_str(),
                    account.name()?,
                    email,
                    status(info.status)
                );
                println!("  Connection: {name}");
            }
            Ok(())
        }
        SubscriptionAuthCommand::Logout { channel, name } => {
            let (_, account, connection) = saved_connection(&settings, channel.into(), &name)?;
            if !sign_out_subscription(account, &connection.options.proxy).await? {
                println!("Signed out locally. Remote revocation could not be confirmed.");
                println!("Manage usage: {}", account.channel().usage_url());
            } else {
                println!("Signed out of {name}.");
            }
            Ok(())
        }
        SubscriptionAuthCommand::Proxy {
            channel,
            name,
            proxy,
        } => {
            let (_, account, _) = saved_connection(&settings, channel.into(), &name)?;
            let proxy = proxy_settings(proxy)
                .ok_or("choose --proxy URL, --proxy-from-env, or --no-proxy")?;
            let name = settings.save_connection(account, &proxy)?;
            println!("Saved network settings for {name}. The service uses them after reloading configuration.");
            Ok(())
        }
        SubscriptionAuthCommand::Models { channel, name } => {
            let (_, _, connection) = saved_connection(&settings, channel.into(), &name)?;
            for model in discover_models(connection).await? {
                if let Some(label) = model
                    .name
                    .filter(|label| !label.chars().any(char::is_control))
                {
                    println!("{}\t{label}", model.id);
                } else {
                    println!("{}", model.id);
                }
            }
            Ok(())
        }
    }
}

async fn login(
    settings: &SubscriptionSettings,
    channel: SubscriptionChannel,
    name: Option<&str>,
    proxy: Option<ProviderProxy>,
) -> CliResult {
    if !io::stdin().is_terminal() {
        return Err("subscription sign-in needs a terminal for hidden callback input".into());
    }
    let proxy = proxy
        .or_else(|| {
            name.and_then(|name| settings.connection(channel, name))
                .map(|(_, _, connection)| connection.options.proxy.clone())
        })
        .unwrap_or_default();
    let mut attempt = Box::new(
        SubscriptionLogin::start(settings.private_data_dir()?, channel, name, proxy.clone())
            .await?,
    );
    let login = loop {
        print_authorization_link(attempt.authorization_url())?;
        let callback = tokio::task::spawn_blocking(|| {
            kuku_tui::read_hidden_line("Paste the callback URL: ", 64 * 1024)
        })
        .await??;
        match attempt.complete_or_retry(&callback).await? {
            SubscriptionLoginProgress::Completed(login) => break login,
            SubscriptionLoginProgress::Retry(new_attempt) => {
                println!("OpenAI rejected the code. Use the new authorization link below.");
                attempt = new_attempt;
            }
        }
    };
    let connection_name = settings.save_connection(&login.account, &proxy)?;
    let name = login.account.name()?;
    let label = login.info.masked_email.as_deref().unwrap_or(name);
    println!("{label} ({name}): {}", status(login.info.status));
    println!("Model connection: {connection_name}");
    if login.info.status == SubscriptionStatus::SignedIn {
        println!("Choose the connection in `kuku init` or `/model`.");
    }
    println!("Manage usage: {}", channel.usage_url());
    Ok(())
}

fn print_authorization_link(url: &str) -> io::Result<()> {
    println!("Open the authorization page in your browser.");
    if io::stdout().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb") {
        println!("\x1b]8;;{url}\x1b\\[Open authorization link]\x1b]8;;\x1b\\");
    }
    println!("\n---- Authorization URL ----\n\n{url}\n\n---- End authorization URL ----\n");
    println!("After authorization, copy the full address even if the page cannot connect.");
    io::stdout().flush()
}

fn saved_connection<'a>(
    settings: &'a SubscriptionSettings,
    channel: SubscriptionChannel,
    name: &str,
) -> Result<(&'a str, &'a SubscriptionAccount, &'a ProviderConfig), Box<dyn std::error::Error>> {
    settings
        .connection(channel, name)
        .ok_or_else(|| format!("unknown {} account; see `kuku auth list`", channel.as_str()).into())
}

fn proxy_settings(args: SubscriptionProxyArgs) -> Option<ProviderProxy> {
    if args.no_proxy {
        return Some(ProviderProxy::Direct);
    }
    if args.proxy_from_env {
        return Some(ProviderProxy::Environment);
    }
    let url = args.proxy?;
    let auth = args.proxy_user.map(|username| ProxyAuth {
        username,
        password_env: args.proxy_password_env,
        password_file: args.proxy_password_file,
    });
    Some(ProviderProxy::Explicit { url, auth })
}

fn status(status: SubscriptionStatus) -> &'static str {
    match status {
        SubscriptionStatus::SignedIn => "signed in",
        SubscriptionStatus::PlanDisabled => "plan usage permission needed",
        SubscriptionStatus::SignedOut => "signed out",
    }
}
