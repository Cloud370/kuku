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
    /// List or inspect available subagent definitions
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

    /// Skip permission prompts; decide by posture
    #[arg(short = 'y', long = "yes")]
    pub auto_yes: bool,

    /// Model tier name (strong/balanced/light) or bare model ID
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Continue an existing session
    #[arg(short = 's', long = "session")]
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

    /// Disable the agent tool (subagent delegation)
    #[arg(long = "no-agents")]
    pub no_agents: bool,

    /// Disable the use_skill tool
    #[arg(long = "no-skills")]
    pub no_skills: bool,

    /// Internal: pre-parsed skill body from interactive slash command
    #[arg(skip)]
    pub skill_body: Option<String>,

    /// Internal: name for a pre-parsed bootstrap skill
    #[arg(skip)]
    pub bootstrap_skill_name: Option<String>,
}

// ── Agents ──

#[derive(Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub cmd: Option<AgentsSubcommand>,
}

#[derive(Subcommand)]
pub enum AgentsSubcommand {
    /// List all available subagent definitions
    List,
    /// Show full details for a specific subagent
    Show {
        /// Subagent name
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
}

// ── Events ──

#[derive(Args)]
pub struct EventsArgs {
    /// Session ID
    pub session_id: String,

    /// Verbose output (-v for metadata, -vv for full context)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,
}

// ── List ──

#[derive(Args)]
pub struct ListArgs {
    /// List sessions across all workspaces
    #[arg(short = 'a', long = "all")]
    pub all: bool,

    /// Filter to a specific workspace (overrides auto-detect)
    #[arg(short = 'w', long = "workspace")]
    pub workspace: Option<String>,

    /// Verbose listing (mtime, created_at)
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
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
