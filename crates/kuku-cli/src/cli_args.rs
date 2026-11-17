//! Shared clap argument structs.
//!
//! Defined in the library crate so commands (in lib) can import them.
//! The binary crate only parses and dispatches.

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kuku", version = env!("KUKU_VERSION"), about = "file-native agent runtime")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
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
    /// Show or manage configuration
    Config(ConfigArgs),
    /// Initialize config and directory structure
    Init,
    /// Show or export embedded prompt assets
    Prompts(PromptsArgs),
    /// List or inspect available agent contact cards
    Agents(AgentsArgs),
    /// List or inspect available skill definitions
    Skills(SkillsArgs),
    /// Start the HTTP API server
    #[cfg(feature = "server")]
    Server(kuku_server::server_args::ServerArgs),
    /// Start the HTTP server with embedded Web UI
    #[cfg(feature = "server")]
    Web(kuku_server::server_args::ServerArgs),
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
    /// The prompt to execute
    #[arg(trailing_var_arg = true, required = true)]
    pub prompt: Vec<String>,

    /// Explicitly select the default trusted-local policy
    #[arg(short = 'y', long = "yes")]
    pub auto_yes: bool,

    /// Exact model identifier (overrides the tier's model)
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Exact configured connection name
    #[arg(long)]
    pub connection: Option<String>,
    /// Caller command identity, allowing safe input retries
    #[arg(long)]
    pub command_id: Option<String>,

    /// Reusable model tier
    #[arg(long)]
    pub tier: Option<String>,

    /// Stable agent address within the session
    #[arg(long, default_value = "main")]
    pub agent: String,

    /// Working directory (defaults to the current directory)
    #[arg(long)]
    pub workspace: Option<String>,

    /// Persistent data directory (defaults to KUKU_HOME or ~/.kuku)
    #[arg(long)]
    pub data_dir: Option<String>,

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

    /// Path to config.toml (default: ~/.kuku/config.toml)
    #[arg(long = "config")]
    pub config: Option<String>,

    /// Directory containing prompt files to override embedded defaults
    #[arg(long = "prompts-dir")]
    pub prompts_dir: Option<String>,

    /// Disable create_agent, send_message, and list_agents; keep ask_user
    #[arg(long = "no-agents")]
    pub no_agents: bool,

    /// Disable the default skill tool surface (`list_skills`, `search_skills`, `use_skill`) and slash-skill loading
    #[arg(long = "no-skills")]
    pub no_skills: bool,

    /// Internal: pre-parsed skill body from interactive slash command
    #[arg(skip)]
    pub skill_body: Option<String>,

    /// Internal: name for a pre-parsed bootstrap skill
    #[arg(skip)]
    pub bootstrap_skill_name: Option<String>,
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
    /// Working directory
    #[arg(long)]
    pub workspace: Option<String>,
    /// Persistent data directory
    #[arg(long)]
    pub data_dir: Option<String>,
    /// Configuration file
    #[arg(long)]
    pub config: Option<String>,
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
    #[arg(long = "agent", alias = "conversation")]
    pub conversation: Option<String>,
    /// Emit the durable Agent snapshot, including pending requests
    #[arg(long)]
    pub json: bool,
    /// Working directory
    #[arg(long)]
    pub workspace: Option<String>,
    /// Persistent data directory
    #[arg(long)]
    pub data_dir: Option<String>,
}

// ── Events ──

#[derive(Args)]
pub struct EventsArgs {
    /// Session ID
    pub session_id: String,

    /// Stable agent address (defaults to full ledger)
    #[arg(long = "agent", alias = "conversation")]
    pub conversation: Option<String>,

    /// Return events after this durable cursor
    #[arg(long, default_value_t = 0)]
    pub after: u64,
    /// Maximum records (1..=256)
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
    /// Emit stored event JSON lines
    #[arg(long)]
    pub json: bool,
    /// Working directory
    #[arg(long)]
    pub workspace: Option<String>,
    /// Persistent data directory
    #[arg(long)]
    pub data_dir: Option<String>,

    /// Verbose output (-v for metadata, -vv for full context)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,
}

// ── List ──

#[derive(Args)]
pub struct ListArgs {
    /// Show conversations for a specific session
    pub session_id: Option<String>,

    /// List sessions across all workspaces
    #[arg(short = 'a', long = "all")]
    pub all: bool,

    /// Filter to a specific workspace (overrides auto-detect)
    #[arg(short = 'w', long = "workspace")]
    pub workspace: Option<String>,

    /// Verbose listing (mtime, created_at)
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
    /// Persistent data directory
    #[arg(long)]
    pub data_dir: Option<String>,
}

// ── Delete ──

#[derive(Args)]
pub struct DeleteArgs {
    /// Session ID to delete
    pub session_id: String,

    /// Workspace the session belongs to
    #[arg(short = 'w', long = "workspace")]
    pub workspace: Option<String>,
}

// ── Config ──

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub cmd: Option<ConfigSubcommand>,

    /// Path to config.toml (default: ~/.kuku/config.toml)
    #[arg(long = "config", global = true)]
    pub config: Option<String>,
}

#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Show current configuration (redacted)
    Show,
    /// Validate config file
    Validate,
    /// Set a config value (e.g. model.balanced.think high)
    Set {
        /// Dot-notation config key (e.g. model.balanced.think)
        key: String,
        /// Value to set
        value: String,
    },
    /// Manage project permission policy
    Policy(PolicyArgs),
}

#[derive(Args)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub cmd: PolicySubcommand,
}

#[derive(Subcommand)]
pub enum PolicySubcommand {
    /// Allow a risk level in this project
    Allow { risk: String },
    /// Deny a risk level in this project
    Deny { risk: String },
}
