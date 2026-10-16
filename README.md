# tm

[中文](README.zh.md)

A personal tmux plugin: one fzf popup to switch between sessions and windows,
create, rename and close them, with a green or yellow dot on each window
where Claude Code or Codex is working or waiting for you. Outside tmux, `tm`
picks a project and starts tmux in it. A single Rust binary; every key runs
one subcommand that takes a few milliseconds.

![tm demo: the window list with agent dots, jumping to a waiting Claude, opening a project](docs/tm.gif)

`prefix + w` lists the windows, with a preview of each and a dot where an
agent works (green) or waits (yellow); typing `learn` and `Enter` jumps to the
waiting Claude; `prefix + f` opens `~/workspace/tmux-manager` as a new session.

## Install

Needs tmux 3.3+ and fzf 0.74.3+.

With [TPM](https://github.com/tmux-plugins/tpm), add the plugin and press
`prefix + I`:

```tmux
set -g @plugin 'zhu-jiyuan/tmux-manager'
```

The plugin fetches the release binary for your platform next to its script,
checks it against the digest GitHub publishes for the asset, and builds it with
cargo when there is no release for your platform. `prefix + U` updates the
binary along with the plugin.

By hand, either `cargo build --release` or download the tarball for your
platform from Releases and unpack it. Then add one line to your tmux config and
reload:

```tmux
run-shell /path/to/tm.tmux
```

For the agent dots run `tm install-hooks` once. It merges its hooks into
`~/.claude/settings.json` and `~/.codex/hooks.json`, backing them up first.

## Keys

`prefix + s` opens the session list, `prefix + w` the window list,
`prefix + f` the project list and `prefix + a` the agent list. Just type to
filter.

| Key | Action |
|---|---|
| `Ctrl-j` `Ctrl-k` | move; `Ctrl-f` `Ctrl-b` page |
| `Enter` | switch; with no match, create a session named after the query |
| `Tab` | cycle the session, window, project and agent lists |
| `Ctrl-o` `Ctrl-r` `Ctrl-x` | new / rename / close, all on the input line |
| `Ctrl-s` | star, pins the session to the top |
| `Ctrl-l` | preview the next window |
| `Ctrl-v` `Ctrl-t` | toggle the preview / full-screen preview |
| `Ctrl-/` | key legend |
| `Esc` | close |

Options, set before the `run-shell` line: `@tm-session-key`,
`@tm-window-key`, `@tm-project-key`, `@tm-agent-key`, `@tm-projects`,
`@tm-switch-width`, `@tm-switch-height`, `@tm-bin`.

## From a shell

`tm` opens the same list in the terminal, on the projects, or on the
sessions when `@tm-projects` is unset; `--sessions`, `--windows`,
`--projects` and `--agents` choose. Inside tmux, `Enter` switches to the pick. Outside, it
attaches tmux to the pick, creating the session, and starting the tmux
server, when there is none yet.

## Projects

Point `@tm-projects` at the directories that hold your projects and
`prefix + f` lists them, open or not. `Enter` switches to a project's
session, creating it in that directory when there is none yet.

```tmux
set -g @tm-projects "~/work ~/oss:2"
```

Each root is scanned one level deep, or `:N` levels; hidden directories are
skipped. A session belongs to the directory it was started in, so a project
opened some other way still shows as open. With no match, typing a path and
pressing `Enter` opens that directory. Without this option `prefix + f`
keeps its tmux meaning. The option lives in your tmux config, so outside
tmux, with no server running, `tm` starts one to read it.

## Agents

`prefix + a` lists every pane where Claude Code or Codex works or waits,
the waiting ones first. Each row shows the harness, where the pane is
(`session:window`, plus `.pane` in a split window), the window's name, its
directory, the dot, and why the agent waits when it can tell: `permission`,
`question`, `done` or `error`. `Enter` jumps to the pane itself, the
preview shows that pane, and `Ctrl-x` closes it, which ends the agent. The
reasons come mostly from the hooks, so run `tm install-hooks` first;
without them only a question or limit on screen gives one.
