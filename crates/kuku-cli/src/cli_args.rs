//! Shared clap argument structs.
//!
//! Defined in the library crate so commands (in lib) can import them.
//! The binary crate only parses and dispatches.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use kuku_client::local::BuildPolicy;
use kuku_client::{Client, Connection, ConnectionOptions, Error};

#[derive(Parser)]
#[command(name = "kuku", version = kuku_server::BUILD_VERSION, about = "file-native agent runtime")]
pub struct Cli {
    /// Select the service and workspace for the requested action.
    #[command(flatten)]
    pub target: ConnectionArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Shared command-line connection choices.
#[derive(Args, Clone, Default)]
pub struct ConnectionArgs {
    /// Workspace on the connected service host (local default: current directory).
    #[arg(short, long, global = true)]
    pub workspace: Option<PathBuf>,
    /// User data directory (default: KUKU_HOME or ~/.kuku).
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    /// Configuration file for the managed service or a local config command.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Explicit remote service URL.
    #[arg(long, global = true)]
    pub remote: Option<String>,
    /// Remote service token (may also be supplied as KUKU_TOKEN).
    #[arg(long, global = true, requires = "remote")]
    pub token: Option<String>,
}

impl ConnectionArgs {
    /// Copy CLI choices into the shared client's connection options.
    pub fn options(&self) -> ConnectionOptions {
        ConnectionOptions {
            workspace: self.workspace.clone(),
            data_dir: self.data_dir.clone(),
            config: self.config.clone(),
            remote: self.remote.clone(),
            token: self.token.clone(),
        }
    }

    /// Connect to the selected service and resolve its workspace.
    pub async fn connect(&self) -> Result<Connection, Error> {
        self.options()
            .connect_for_build(BuildPolicy::Start(kuku_server::BUILD_VERSION))
            .await
    }

    /// Connect to the selected service without scoping operations to a workspace.
    pub async fn client(&self) -> Result<Client, Error> {
        self.options()
            .client_for_build(BuildPolicy::Start(kuku_server::BUILD_VERSION))
            .await
    }

    /// Resolve local management without starting a service.
    pub fn local_service(&self) -> Result<kuku_client::local::LocalService, Error> {
        self.options().local_service()
    }

    /// Return the explicit remote connection, if selected.
    pub fn remote_connection(&self) -> Result<Option<kuku_client::Client>, Error> {
        self.options().remote_connection()
    }

    /// Get the user data path without starting a service.
    pub fn data_dir(&self) -> Result<PathBuf, Error> {
        self.options().data_dir()
    }

    /// Get the selected workspace path without starting a service.
    pub fn workspace(&self) -> Result<PathBuf, Error> {
        self.options().workspace()
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Execute an agent task (non-interactive)
    Run(RunArgs),
    /// Reply to one durable question or authorization request
    Reply(ReplyArgs),
    /// Show final output from a session
    Show(ShowArgs),
    /// Show events from a session
    Events(EventsArgs),
    /// List sessions (current workspace by default, -a for all)
    List(ListArgs),
    /// Delete a session
    Delete(DeleteArgs),
    /// Recover an unconfirmed command
    Pending(PendingArgs),
    /// Show or manage configuration
    Config(ConfigArgs),
    /// Sign in to or manage model subscription accounts
    Auth(SubscriptionAuthArgs),
    /// Show or export embedded prompt assets
    Prompts(PromptsArgs),
    /// List or inspect available agent contact cards
    Agents(AgentsArgs),
    /// List or inspect available skill definitions
    Skills(SkillsArgs),
    /// Start or manage the HTTP service
    Server(ServerCommandArgs),
    /// Internal entry point used to launch the managed service.
    #[command(name = "__managed-server", hide = true)]
    ManagedServer(ServerCommandArgs),
}

/// Subscription account operations.
#[derive(Args)]
pub struct SubscriptionAuthArgs {
    #[command(subcommand)]
    pub command: SubscriptionAuthCommand,
}

/// A supported subscription authorization channel.
#[derive(Clone, Copy, ValueEnum)]
pub enum SubscriptionChannelArg {
    #[value(name = "chatgpt")]
    ChatGpt,
}

impl From<SubscriptionChannelArg> for kuku::config::SubscriptionChannel {
    fn from(value: SubscriptionChannelArg) -> Self {
        match value {
            SubscriptionChannelArg::ChatGpt => Self::ChatGpt,
        }
    }
}

/// Network settings for a subscription account.
#[derive(Args)]
pub struct SubscriptionProxyArgs {
    /// HTTP or SOCKS5H proxy URL.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Use the proxy settings from the environment.
    #[arg(long, conflicts_with = "proxy")]
    pub proxy_from_env: bool,
    /// Connect directly, ignoring proxy settings from the environment.
    #[arg(long, conflicts_with_all = ["proxy", "proxy_from_env"])]
    pub no_proxy: bool,
    /// Username for the proxy.
    #[arg(long, requires = "proxy")]
    pub proxy_user: Option<String>,
    /// Name of the environment variable containing the proxy password.
    #[arg(long, requires = "proxy_user", conflicts_with = "proxy_password_file")]
    pub proxy_password_env: Option<String>,
    /// Absolute path to the private proxy password file.
    #[arg(long, requires = "proxy_user")]
    pub proxy_password_file: Option<PathBuf>,
}

/// Manage saved subscriptions on the service host.
#[derive(Subcommand)]
pub enum SubscriptionAuthCommand {
    /// Authorize a channel in your browser and paste its callback in the terminal.
    Login {
        channel: SubscriptionChannelArg,
        /// Name a new account or reauthorize the existing account with this name.
        name: Option<String>,
        #[command(flatten)]
        proxy: SubscriptionProxyArgs,
    },
    /// Show saved account names and sign-in state.
    List,
    /// Sign out of one saved account.
    Logout {
        channel: SubscriptionChannelArg,
        name: String,
    },
    /// Change the network settings for a saved account.
    Proxy {
        channel: SubscriptionChannelArg,
        name: String,
        #[command(flatten)]
        proxy: SubscriptionProxyArgs,
    },
    /// List the models available for the saved account.
    Models {
        channel: SubscriptionChannelArg,
        name: String,
    },
}

/// Arguments for starting or managing the service.
#[derive(Args)]
pub struct ServerCommandArgs {
    /// Listen address for an explicit foreground server.
    #[arg(long, default_value = "127.0.0.1:17777")]
    pub listen: String,
    /// Foreground service password (may also be supplied as KUKU_SERVER_PASSWORD).
    #[arg(long)]
    pub password: Option<String>,
    /// Maximum concurrent foreground executions.
    #[arg(long, default_value = "16")]
    pub max_concurrent_runs: usize,
    /// Optional service management operation.
    #[command(subcommand)]
    pub command: Option<ServerCommand>,
}

impl ServerCommandArgs {
    /// Build service-host arguments with the selected data and config paths.
    pub fn into_server_args(self, target: &ConnectionArgs) -> kuku_server::server_args::ServerArgs {
        kuku_server::server_args::ServerArgs {
            listen: self.listen,
            config: target.config.clone(),
            data_dir: target.data_dir.clone(),
            password: self.password,
            max_concurrent_runs: self.max_concurrent_runs,
            managed: false,
        }
    }
}

/// Operations on a managed or remote service.
#[derive(Subcommand)]
pub enum ServerCommand {
    /// Show the service version, configuration, and running work.
    Status,
    /// Restart the managed service when running work has settled.
    Restart,
}

/// Choose how to handle a pending command.
#[derive(Args)]
pub struct PendingArgs {
    /// Operation; defaults to `list`.
    #[command(subcommand)]
    pub command: Option<PendingCommand>,
}

/// Actions for recovering a pending command.
#[derive(Subcommand)]
pub enum PendingCommand {
    /// Query receipts and list commands that still need a decision.
    List,
    /// Send the exact original intent again if its durable receipt is missing.
    Resend(PendingTargetArgs),
    /// Discard the local pending intent if its durable receipt is missing.
    Discard(PendingTargetArgs),
}

/// Identify one pending command in the selected workspace.
#[derive(Args)]
pub struct PendingTargetArgs {
    /// Original command identity shown by `kuku pending list`.
    pub command_id: String,
    /// Select a Session when the command ID is reused in more than one.
    #[arg(long)]
    pub session: Option<String>,
}
// ── Prompts ──

#[derive(Args)]
pub struct PromptsArgs {
    #[command(subcommand)]
    pub cmd: Option<PromptsSubcommand>,
}

#[derive(Subcommand)]
pub enum PromptsSubcommand {
    /// Show embedded prompt content
    Show {
        /// Prompt name: system, project-context, tool-guidance, runtime-context, or omit for all
        name: Option<String>,
    },
    /// Export embedded prompts to a directory
    Export {
        /// Target directory path
        dir: String,
    },
}

// ── Run ──

#[derive(Args)]
pub struct RunArgs {
    /// The prompt to execute. Place options before the positional prompt.
    #[arg(
        trailing_var_arg = true,
        required_unless_present = "prompt_file",
        conflicts_with = "prompt_file"
    )]
    pub prompt: Vec<String>,

    /// Read the complete UTF-8 prompt from a file, or stdin with `-`.
    #[arg(long, value_name = "PATH|-", value_hint = clap::ValueHint::FilePath)]
    pub prompt_file: Option<PathBuf>,

    /// Exact configured model identifier
    #[arg(long, requires = "connection")]
    pub model: Option<String>,

    /// Exact configured connection name
    #[arg(long, requires = "model")]
    pub connection: Option<String>,

    /// Select a discovered Agent preset for a new Session
    #[arg(long, conflicts_with_all = ["session", "cont"])]
    pub preset: Option<String>,

    /// Caller command identity, allowing safe input retries
    #[arg(long)]
    pub command_id: Option<String>,

    /// Stable agent address within the session
    #[arg(long, default_value = "main")]
    pub agent: String,

    /// Provider-supported reasoning effort
    #[arg(long)]
    pub reasoning_effort: Option<String>,

    /// Provider-supported service tier
    #[arg(long)]
    pub service_tier: Option<String>,

    /// Bound output tokens per model request
    #[arg(long)]
    pub max_output_tokens: Option<u32>,

    /// Continue an existing session
    #[arg(short = 's', long = "session", conflicts_with = "cont")]
    pub session: Option<String>,

    /// Continue the most recent session
    #[arg(short = 'c', long = "continue")]
    pub cont: bool,

    /// Output format: single JSON result at end
    #[arg(long = "json", conflicts_with = "stream_json")]
    pub json: bool,

    /// Output format: realtime JSON lines
    #[arg(long = "stream-json", conflicts_with = "json")]
    pub stream_json: bool,

    /// Show thinking content from the model
    #[arg(long = "show-thinking")]
    pub show_thinking: bool,

    /// Raw output mode: plain text without decorations
    #[arg(long = "raw", conflicts_with_all = ["json", "stream_json"])]
    pub raw: bool,

    /// Show detailed run metrics (usage, tools, response)
    #[arg(long = "verbose", conflicts_with_all = ["json", "stream_json"])]
    pub verbose: bool,

    /// Directory containing prompt files to override embedded defaults
    #[arg(long = "prompts-dir")]
    pub prompts_dir: Option<PathBuf>,

    /// Disable create_agent, send_message, and list_agents; keep ask_user
    #[arg(long = "no-agents")]
    pub no_agents: bool,

    /// Disable the default skill tool surface (`list_skills`, `search_skills`, `use_skill`) and slash-skill loading
    #[arg(long = "no-skills")]
    pub no_skills: bool,
}

/// Select one durable request and an explicit host reply.
#[derive(Args)]
pub struct ReplyArgs {
    /// Session containing the pending request
    pub session_id: String,
    /// Exact durable request identity
    pub request_id: String,
    /// Stable agent address
    #[arg(long, default_value = "main")]
    pub agent: String,
    /// Authorize the bound action once
    #[arg(long, required_unless_present_any = ["deny", "answer"], conflicts_with_all = ["deny", "answer"])]
    pub allow: bool,
    /// Deny the bound action
    #[arg(long, required_unless_present_any = ["allow", "answer"], conflicts_with_all = ["allow", "answer"])]
    pub deny: bool,
    /// Answer the exact pending question
    #[arg(long, required_unless_present_any = ["allow", "deny"], conflicts_with_all = ["allow", "deny"])]
    pub answer: Option<String>,
    /// Caller command identity, allowing safe retries
    #[arg(long)]
    pub command_id: Option<String>,
}

// ── Agents ──

#[derive(Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub cmd: Option<AgentsSubcommand>,
}

#[derive(Subcommand)]
pub enum AgentsSubcommand {
    /// List all available agent contact cards
    List,
    /// Show full details for a specific agent contact card
    Show {
        /// Agent name
        name: String,
    },
}

// ── Skills ──

#[derive(Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub cmd: Option<SkillsSubcommand>,
}

#[derive(Subcommand)]
pub enum SkillsSubcommand {
    /// List all available skill definitions
    List,
    /// Show full details for a specific skill
    Show {
        /// Skill name
        name: String,
    },
}

// ── Show ──

#[derive(Args)]
pub struct ShowArgs {
    /// Session ID
    pub session_id: String,

    /// Stable agent address (defaults to main)
    #[arg(long = "agent")]
    pub conversation: Option<String>,
    /// Emit the durable Agent snapshot, including pending requests
    #[arg(long)]
    pub json: bool,
}

// ── Events ──

#[derive(Args)]
pub struct EventsArgs {
    /// Session ID in the selected workspace
    #[arg(required_unless_present = "file")]
    pub session_id: Option<String>,

    /// Read a local journal file directly
    #[arg(long, conflicts_with_all = ["session_id", "remote"])]
    pub file: Option<PathBuf>,

    /// Stable agent address (defaults to the full ledger)
    #[arg(long = "agent")]
    pub conversation: Option<String>,

    /// Return events after this exclusive cursor, oldest first
    #[arg(long, conflicts_with = "before")]
    pub after: Option<u64>,
    /// Return events before this exclusive cursor, starting from the newest
    #[arg(long)]
    pub before: Option<u64>,
    /// Stream all records from the start or --after
    #[arg(long, conflicts_with_all = ["before", "limit"])]
    pub all: bool,
    /// Only include the given event kind
    #[arg(long)]
    pub kind: Option<String>,
    /// Search event content, ignoring case
    #[arg(long)]
    pub search: Option<String>,
    /// Maximum records returned (1..=256)
    #[arg(long, default_value_t = 40, value_parser = clap::value_parser!(u16).range(1..=256))]
    pub limit: u16,
    /// Emit stored event JSON lines
    #[arg(long)]
    pub json: bool,

    /// Show additional event metadata
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

// ── List ──

#[derive(Args)]
pub struct ListArgs {
    /// Show Agents in a specific Session
    pub session_id: Option<String>,

    /// List Sessions across all workspaces
    #[arg(short = 'a', long, conflicts_with = "session_id")]
    pub all: bool,

    /// Include the modification time in the Session table
    #[arg(short = 'v', long, conflicts_with_all = ["session_id", "json"])]
    pub verbose: bool,

    /// Print a JSON array of Sessions or Agents
    #[arg(long)]
    pub json: bool,
}

// ── Delete ──

#[derive(Args)]
pub struct DeleteArgs {
    /// Session ID to delete
    pub session_id: String,
}

// ── Config ──

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub cmd: Option<ConfigSubcommand>,
}

#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Show current configuration with credentials redacted.
    Show {
        /// Include the selected path and load status in a JSON object.
        #[arg(long)]
        json: bool,
    },
    /// List configured models, or discover exact model IDs from one connection.
    Models {
        /// Query the selected connection's model-list endpoint.
        #[arg(long, requires = "connection")]
        discover: bool,
        /// Connection to query for model discovery.
        #[arg(long, requires = "discover")]
        connection: Option<String>,
    },
    /// Validate the file and report whether the managed service loaded it.
    Validate,
    /// Update a scalar field or apply a literal TOML patch in one write.
    Set {
        /// TOML dotted path for a scalar field.
        #[arg(
            required_unless_present = "patch",
            requires = "value",
            conflicts_with = "patch"
        )]
        key: Option<String>,
        /// Value to set.
        #[arg(requires = "key", allow_negative_numbers = true)]
        value: Option<String>,
        /// Literal TOML to merge. The root `delete` array lists paths to remove first.
        #[arg(long, value_name = "TOML", conflicts_with_all = ["key", "value"])]
        patch: Option<String>,
    },
}
