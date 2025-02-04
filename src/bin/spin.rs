use anyhow::Error;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use is_terminal::IsTerminal;
use lazy_static::lazy_static;
use spin_cli::build_info::*;
use spin_cli::commands::external::predefined_externals;
use spin_cli::commands::{
    build::BuildCommand,
    cloud::{DeployCommand, LoginCommand},
    doctor::DoctorCommand,
    external::execute_external_subcommand,
    new::{AddCommand, NewCommand},
    plugins::PluginCommands,
    registry::RegistryCommands,
    templates::TemplateCommands,
    up::UpCommand,
    watch::WatchCommand,
};
use spin_redis_engine::RedisTrigger;
use spin_trigger::cli::help::HelpArgsOnlyTrigger;
use spin_trigger::cli::TriggerExecutorCommand;
use spin_trigger_http::HttpTrigger;

#[tokio::main]
async fn main() {
    if let Err(err) = _main().await {
        terminal::error!("{err}");
        print_error_chain(err);
        std::process::exit(1)
    }
}

async fn _main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("watchexec=off".parse()?),
        )
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    let plugin_help_entries = plugin_help_entries();

    let mut cmd = SpinApp::command();
    for plugin in &plugin_help_entries {
        let subcmd = clap::Command::new(plugin.display_text())
            .about(plugin.about.as_str())
            .allow_hyphen_values(true)
            .disable_help_flag(true)
            .arg(
                clap::Arg::new("command")
                    .allow_hyphen_values(true)
                    .multiple_values(true),
            );
        cmd = cmd.subcommand(subcmd);
    }

    if !plugin_help_entries.is_empty() {
        cmd = cmd.after_help("* implemented via plugin");
    }

    let matches = cmd.clone().get_matches();

    if let Some((subcmd, _)) = matches.subcommand() {
        if plugin_help_entries.iter().any(|e| e.name == subcmd) {
            let command = std::env::args().skip(1).collect();
            return execute_external_subcommand(command, cmd).await;
        }
    }

    SpinApp::from_arg_matches(&matches)?.run(cmd).await
}

fn print_error_chain(err: anyhow::Error) {
    if let Some(cause) = err.source() {
        let is_multiple = cause.source().is_some();
        eprintln!("\nCaused by:");
        for (i, err) in err.chain().skip(1).enumerate() {
            if is_multiple {
                eprintln!("{i:>4}: {}", err)
            } else {
                eprintln!("      {}", err)
            }
        }
    }
}

lazy_static! {
    pub static ref VERSION: String = build_info();
}

/// Helper for passing VERSION to structopt.
fn version() -> &'static str {
    &VERSION
}

/// The Spin CLI
#[derive(Parser)]
#[clap(
    name = "spin",
    version = version()
)]
enum SpinApp {
    #[clap(subcommand, alias = "template")]
    Templates(TemplateCommands),
    New(NewCommand),
    Add(AddCommand),
    Up(UpCommand),
    // acts as a cross-level subcommand shortcut -> `spin cloud deploy`
    Deploy(DeployCommand),
    // acts as a cross-level subcommand shortcut -> `spin cloud login`
    Login(LoginCommand),
    #[clap(subcommand, alias = "oci")]
    Registry(RegistryCommands),
    Build(BuildCommand),
    #[clap(subcommand, alias = "plugin")]
    Plugins(PluginCommands),
    #[clap(subcommand, hide = true)]
    Trigger(TriggerCommands),
    #[clap(external_subcommand)]
    External(Vec<String>),
    Watch(WatchCommand),
    Doctor(DoctorCommand),
}

#[derive(Subcommand)]
enum TriggerCommands {
    Http(TriggerExecutorCommand<HttpTrigger>),
    Redis(TriggerExecutorCommand<RedisTrigger>),
    #[clap(name = spin_cli::HELP_ARGS_ONLY_TRIGGER_TYPE, hide = true)]
    HelpArgsOnly(TriggerExecutorCommand<HelpArgsOnlyTrigger>),
}

impl SpinApp {
    /// The main entry point to Spin.
    pub async fn run(self, app: clap::Command<'_>) -> Result<(), Error> {
        match self {
            Self::Templates(cmd) => cmd.run().await,
            Self::Up(cmd) => cmd.run().await,
            Self::New(cmd) => cmd.run().await,
            Self::Add(cmd) => cmd.run().await,
            Self::Deploy(cmd) => cmd.run(SpinApp::command()).await,
            Self::Login(cmd) => cmd.run(SpinApp::command()).await,
            Self::Registry(cmd) => cmd.run().await,
            Self::Build(cmd) => cmd.run().await,
            Self::Trigger(TriggerCommands::Http(cmd)) => cmd.run().await,
            Self::Trigger(TriggerCommands::Redis(cmd)) => cmd.run().await,
            Self::Trigger(TriggerCommands::HelpArgsOnly(cmd)) => cmd.run().await,
            Self::Plugins(cmd) => cmd.run().await,
            Self::External(cmd) => execute_external_subcommand(cmd, app).await,
            Self::Watch(cmd) => cmd.run().await,
            Self::Doctor(cmd) => cmd.run().await,
        }
    }
}

/// Returns build information, similar to: 0.1.0 (2be4034 2022-03-31).
fn build_info() -> String {
    format!("{SPIN_VERSION} ({SPIN_COMMIT_SHA} {SPIN_COMMIT_DATE})")
}

struct PluginHelpEntry {
    name: String,
    about: String,
}

impl PluginHelpEntry {
    fn from_plugin(plugin: &spin_plugins::manifest::PluginManifest) -> Option<Self> {
        if hide_plugin_in_help(plugin) {
            None
        } else {
            Some(Self {
                name: plugin.name(),
                about: plugin.description().unwrap_or_default().to_owned(),
            })
        }
    }

    fn display_text(&self) -> String {
        format!("{}*", self.name)
    }
}

fn plugin_help_entries() -> Vec<PluginHelpEntry> {
    let mut entries = installed_plugin_help_entries();
    for (name, about) in predefined_externals() {
        if !entries.iter().any(|e| e.name == name) {
            entries.push(PluginHelpEntry { name, about });
        }
    }
    entries
}

fn installed_plugin_help_entries() -> Vec<PluginHelpEntry> {
    let Ok(manager) = spin_plugins::manager::PluginManager::try_default() else {
        return vec![];
    };
    let Ok(manifests) = manager.store().installed_manifests() else {
        return vec![];
    };

    manifests
        .iter()
        .filter_map(PluginHelpEntry::from_plugin)
        .collect()
}

fn hide_plugin_in_help(plugin: &spin_plugins::manifest::PluginManifest) -> bool {
    plugin.name().starts_with("trigger-")
}
