//! tm: a personal tmux session and window switcher built on fzf, with
//! agent activity dots.
//!
//! One binary, many small subcommands. `switch` opens the popup, and every
//! fzf key binding calls back into this same binary. Each call is a fresh
//! process, so startup cost and the number of `tmux` invocations per call are
//! what decide how the popup feels. `switch`, `open`, `activity`, `hook` and
//! `install-hooks` are entry points; the rest are what the keys run, and each
//! also runs from a shell, as the first key of a popup that ends with it (see
//! `popup`).

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

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tm",
    version,
    about = "tmux session and window switcher on fzf, with agent activity dots",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// `--windows` or `--projects`. Neither means sessions for `switch`, and for
/// `list` the popup's mode, which from a shell is sessions.
#[derive(Args)]
struct ModeFlags {
    /// Windows mode
    #[arg(long, conflicts_with = "projects")]
    windows: bool,
    /// Projects mode
    #[arg(long)]
    projects: bool,
}

impl ModeFlags {
    fn mode(&self) -> Option<popup::Mode> {
        if self.projects {
            Some(popup::Mode::Projects)
        } else if self.windows {
            Some(popup::Mode::Windows)
        } else {
            None
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Open the popup for a tmux client (prefix + s, prefix + w, prefix + f)
    Switch {
        client: String,
        #[command(flatten)]
        mode: ModeFlags,
    },
    /// Move the open popup to another mode: sessions, windows, projects (tab)
    Mode { action: actions::ModeAction },
    /// Find or create the session for a project directory; prints its id
    Open { path: PathBuf },
    /// Print the rows; fzf reloads from this, and it runs from any shell too
    List {
        #[command(flatten)]
        mode: ModeFlags,
    },
    /// Reload fzf when the rows changed (runs in the background every second)
    Refresh,
    /// Star, unstar or toggle a session (ctrl-s)
    Favorite {
        mode: actions::FavoriteMode,
        name: String,
    },
    /// Render a row's window for the preview pane
    Preview { row: String },
    /// Move a session row's preview to its next window (ctrl-l)
    PreviewNext { row: String },
    /// Take over the popup for a full-size preview (ctrl-t)
    PreviewFull { row: String },
    /// After the full view: replay the action it left behind
    ViewReturn,
    /// Toggle the key legend in the footer (ctrl-/)
    Help,
    /// Print the columns to show for the current query (runs on every change)
    WithNth,
    /// Open the inline prompt for a row (ctrl-o, ctrl-r)
    Prompt {
        kind: actions::PromptKind,
        id: String,
    },
    /// Close a session or window (ctrl-x)
    Close { id: String },
    /// Enter: answer the open prompt or accept the row
    Enter,
    /// Esc: cancel the open prompt or close the popup
    Esc,
    /// Record an agent lifecycle event (called by the harnesses' hooks)
    Hook {
        harness: String,
        event: Option<String>,
    },
    /// Print per-window agent activity as JSON
    Activity,
    /// Install the hooks of every harness tm knows
    InstallHooks,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Switch { client, mode } => {
            switch::run(&client, mode.mode().unwrap_or(popup::Mode::Sessions));
            Ok(())
        }
        Command::Mode { action } => actions::mode(action),
        Command::Open { path } => projects::open(&path).map(|id| println!("{id}")),
        Command::List { mode } => switch::list(mode.mode()),
        Command::Refresh => switch::refresh(),
        Command::Favorite { mode, name } => actions::favorite(mode, &name),
        Command::Preview { row } => preview::run(&row),
        Command::PreviewNext { row } => preview::next(&row),
        Command::PreviewFull { row } => preview::full(&row),
        Command::ViewReturn => preview::view_return(),
        Command::Help => actions::help(),
        Command::WithNth => actions::with_nth(),
        Command::Prompt { kind, id } => actions::prompt(kind, &id),
        Command::Close { id } => actions::close(&id),
        Command::Enter => actions::enter(),
        Command::Esc => actions::escape(),
        // Hooks only observe. Whatever goes wrong, an agent must never see a failure.
        Command::Hook { harness, event } => {
            let _ = agent::hook(&harness, event.as_deref());
            Ok(())
        }
        Command::Activity => tmux::panes()
            .and_then(|panes| agent::states(&panes))
            .and_then(|states| {
                println!("{}", serde_json::to_string(&states)?);
                Ok(())
            }),
        Command::InstallHooks => install::install_hooks(),
    };
    if let Err(error) = result {
        eprintln!("tm: {error:#}");
        std::process::exit(1);
    }
}
