//! tm: a personal tmux session and window switcher built on fzf, with
//! agent activity dots.
//!
//! One binary, many small subcommands. `tm` alone opens the picker, and
//! every fzf key binding calls back into this same binary. Each call is a
//! fresh process, so startup cost and the number of `tmux` invocations per
//! call are what decide how the picker feels. Only the picker, `activity`
//! and `install-hooks` are for people, and only they show in `--help`; the
//! hidden rest are what the keys and the agents' hooks run, and each key
//! also runs from a shell, as the first key of a popup that ends with it
//! (see `popup`).

mod actions;
mod agent;
mod ansi;
mod fzf;
mod harness;
mod install;
mod paths;
mod popup;
mod preview;
mod projects;
mod rows;
mod state;
mod switch;
mod tmux;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tm",
    version,
    about = "tmux session, window and project switcher on fzf, with agent activity dots",
    long_about = None,
    after_help = "With no command, tm opens the project list (the session list when \
                  @tm-projects is unset). Inside tmux, Enter switches to the pick; \
                  outside, it attaches tmux to it, starting what is not running yet.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(flatten)]
    mode: ModeFlags,
    /// The client to switch: tm.tmux's keys open the picker in its popup
    #[arg(long, hide = true)]
    client: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Which list, at most one. Without one the picker starts on its default,
/// `list` prints the popup's mode, and `mode` moves to the next.
#[derive(Args)]
#[group(multiple = false)]
struct ModeFlags {
    /// Start on the session list
    #[arg(long)]
    sessions: bool,
    /// Start on the window list
    #[arg(long)]
    windows: bool,
    /// Start on the project list
    #[arg(long)]
    projects: bool,
    /// Start on the agent list: every pane where an agent works or waits
    #[arg(long)]
    agents: bool,
}

impl ModeFlags {
    fn mode(&self) -> Option<popup::Mode> {
        if self.sessions {
            Some(popup::Mode::Sessions)
        } else if self.windows {
            Some(popup::Mode::Windows)
        } else if self.projects {
            Some(popup::Mode::Projects)
        } else if self.agents {
            Some(popup::Mode::Agents)
        } else {
            None
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Print per-window agent activity as JSON
    Activity,
    /// Install the hooks of every harness tm knows
    InstallHooks,
    /// Record an agent lifecycle event (the harnesses' hooks run this)
    #[command(hide = true)]
    Hook {
        harness: String,
        event: Option<String>,
    },
    /// Print the rows; fzf reloads from this
    #[command(hide = true)]
    List {
        #[command(flatten)]
        mode: ModeFlags,
    },
    /// Reload fzf when the rows changed (runs in the background every second)
    #[command(hide = true)]
    Refresh,
    /// Move the picker to another list, by default the next (tab)
    #[command(hide = true)]
    Mode {
        #[command(flatten)]
        mode: ModeFlags,
    },
    /// Star or unstar a session (ctrl-s)
    #[command(hide = true)]
    Favorite { name: String },
    /// Render a row's window for the preview pane
    #[command(hide = true)]
    Preview { row: String },
    /// Move a session row's preview to its next window (ctrl-l)
    #[command(hide = true)]
    PreviewNext { row: String },
    /// Take over the terminal for a full-size preview (ctrl-t)
    #[command(hide = true)]
    PreviewFull { row: String },
    /// After the full view: replay the action it left behind
    #[command(hide = true)]
    ViewReturn,
    /// Toggle the key legend in the footer (ctrl-/)
    #[command(hide = true)]
    Legend,
    /// Print the columns to show for the current query (runs on every change)
    #[command(hide = true)]
    WithNth,
    /// Open the inline prompt for a row (ctrl-o, ctrl-r)
    #[command(hide = true)]
    Prompt {
        kind: actions::PromptKind,
        id: String,
    },
    /// Close a session, window or agent's pane (ctrl-x)
    #[command(hide = true)]
    Close { id: String },
    /// Enter: answer the open prompt or accept the row
    #[command(hide = true)]
    Enter,
    /// Esc: cancel the open prompt or close the picker
    #[command(hide = true)]
    Esc,
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let Some(command) = cli.command else {
        let mode = cli.mode.mode();
        return match cli.client {
            Some(client) => {
                switch::popup(&client, mode);
                Ok(())
            }
            None => switch::here(mode),
        };
    };
    match command {
        Command::Activity => tmux::panes()
            .and_then(|panes| agent::states(&panes))
            .and_then(|states| {
                println!("{}", serde_json::to_string(&states)?);
                Ok(())
            }),
        Command::InstallHooks => install::install_hooks(),
        // Hooks only observe. Whatever goes wrong, an agent must never see a failure.
        Command::Hook { harness, event } => {
            let _ = agent::hook(&harness, event.as_deref());
            Ok(())
        }
        Command::List { mode } => switch::list(mode.mode()),
        Command::Refresh => switch::refresh(),
        Command::Mode { mode } => actions::mode(mode.mode()),
        Command::Favorite { name } => actions::favorite(&name),
        Command::Preview { row } => preview::run(&row),
        Command::PreviewNext { row } => preview::next(&row),
        Command::PreviewFull { row } => preview::full(&row),
        Command::ViewReturn => preview::view_return(),
        Command::Legend => actions::legend(),
        Command::WithNth => actions::with_nth(),
        Command::Prompt { kind, id } => actions::prompt(kind, &id),
        Command::Close { id } => actions::close(&id),
        Command::Enter => actions::enter(),
        Command::Esc => actions::escape(),
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("tm: {error:#}");
        std::process::exit(1);
    }
}
