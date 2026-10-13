//! End-to-end tests against a throwaway tmux server.
//!
//! Every test starts its own server on a private socket and kills it when
//! done, so your real tmux is never touched. Tests are skipped when tmux is
//! not installed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

const TM: &str = env!("CARGO_BIN_EXE_tm");

/// fzf, as far as the picker can tell. It keeps the arguments and rows tm
/// gave it next to itself (`fzf.args`, `fzf.rows`), prints `$TM_TEST_QUERY`
/// for `--print-query`, then the row whose id is `$TM_TEST_PICK`; with no
/// such row it exits 1, fzf's "no match". The real fzf would open the
/// terminal running the tests.
const FAKE_FZF: &str = r#"#!/bin/sh
[ "$1" = --version ] && { echo 0.99.0; exit 0; }
printf '%s\n' "$@" > "$0.args"
cat > "$0.rows"
printf '%s\n' "$TM_TEST_QUERY"
awk -F '\t' -v pick="$TM_TEST_PICK" '$1 == pick { print; found = 1 } END { exit !found }' "$0.rows"
"#;

/// Put the fake fzf in `dir/bin`; a `PATH` that finds it first.
fn fake_fzf(dir: &Path) -> String {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fzf = bin.join("fzf");
    fs::write(&fzf, FAKE_FZF).unwrap();
    fs::set_permissions(&fzf, fs::Permissions::from_mode(0o755)).unwrap();
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// The picker as the fake fzf leaves it: `TM_TEST_PICK` names the row to
/// pick, else Enter with `TM_TEST_QUERY` typed matched nothing.
fn picker(command: &mut Command, dir: &Path, pick: &str, query: &str) -> Output {
    command
        .env("PATH", fake_fzf(dir))
        .env("TM_TEST_PICK", pick)
        .env("TM_TEST_QUERY", query)
        .output()
        .unwrap()
}

/// What fzf and a popup put in a callback's environment. A shell run has
/// none of them, whatever the environment `cargo test` runs in.
const POPUP_VARS: [&str; 6] = [
    "TM_SNAPSHOT",
    "TMUX_PANE",
    "FZF_QUERY",
    "FZF_IDLE_TIME_MS",
    "FZF_PREVIEW_COLUMNS",
    "FZF_PREVIEW_LINES",
];

/// The names in `dir`, sorted; none when it does not exist.
fn files(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Poll `ready` for up to three seconds; whether it came true.
fn eventually(ready: impl Fn() -> bool) -> bool {
    for _ in 0..30 {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

struct Server {
    socket: String,
    tmux_env: String,
    state: PathBuf,
}

impl Server {
    fn start(tag: &str) -> Option<Server> {
        if Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("tmux is not installed; skipping");
            return None;
        }
        let socket = format!("tm-test-{}-{tag}", std::process::id());
        let state = std::env::temp_dir().join(format!("{socket}-state"));
        fs::create_dir_all(&state).unwrap();
        let mut server = Server {
            socket,
            tmux_env: String::new(),
            state,
        };
        server.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "alpha",
            "-x",
            "120",
            "-y",
            "40",
        ]);
        let path = server.tmux(&["display-message", "-p", "#{socket_path}"]);
        assert!(!path.is_empty(), "throwaway tmux server did not start");
        server.tmux_env = format!("{path},0,0");
        Some(server)
    }

    /// tmux on this server. `-u`: without `TMUX` set and a UTF-8 locale it
    /// prints tabs as `_`.
    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-u", "-L"])
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    }

    /// Run tm the way fzf would inside a popup of this server, with `env`
    /// on top of fzf's usual variables, and return what it printed. A
    /// failing tm fails the test with its stderr.
    fn tm_env(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let snapshot = self.state.join("snap.txt");
        let mut popup = vec![
            ("TM_SNAPSHOT", snapshot.to_str().unwrap()),
            ("FZF_PREVIEW_COLUMNS", "80"),
            ("FZF_PREVIEW_LINES", "24"),
            ("FZF_QUERY", ""),
        ];
        popup.extend_from_slice(env);
        self.run_tm(args, &popup)
    }

    fn tm(&self, args: &[&str]) -> String {
        self.tm_env(args, &[])
    }

    /// Run tm the way a user would from a shell on this server: no popup,
    /// no fzf variables.
    fn tm_shell(&self, args: &[&str]) -> String {
        self.run_tm(args, &[])
    }

    /// tm inside this server, with none of a popup's variables.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(TM);
        command
            .args(args)
            .env("TMUX", &self.tmux_env)
            .env("TM_STATE_DIR", &self.state);
        for var in POPUP_VARS {
            command.env_remove(var);
        }
        command
    }

    fn run_tm(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let mut command = self.command(args);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "tm {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// Run the picker (`tm` and `args`) from a shell on this server, the
    /// fake fzf picking the row whose id is `pick`, or with none, leaving
    /// `query` typed. A failing picker fails the test with its stderr.
    fn pick(&self, args: &[&str], pick: &str, query: &str) {
        let output = picker(&mut self.command(args), &self.state, pick, query);
        assert!(
            output.status.success(),
            "tm {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// What the picker last gave fzf: its arguments, one per line, and rows.
    fn fzf(&self) -> (String, String) {
        let read = |name: &str| fs::read_to_string(self.state.join("bin").join(name)).unwrap();
        (read("fzf.args"), read("fzf.rows"))
    }

    /// A control-mode client on `session`, standing in for a person's
    /// terminal.
    fn attach(&self, session: &str) -> Client {
        let child = Command::new("tmux")
            .args(["-L", &self.socket, "-C", "attach", "-t", session])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Its drop reaps the process, even if it never shows up.
        let mut client = Client {
            child,
            name: String::new(),
        };
        for _ in 0..30 {
            client.name = self.tmux(&["list-clients", "-F", "#{client_name}"]);
            if !client.name.is_empty() {
                return client;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("the control client never attached");
    }

    /// `session:window index` the client looks at.
    fn showing(&self, client: &Client) -> String {
        let prefix = format!("{}\t", client.name);
        self.tmux(&[
            "list-clients",
            "-F",
            "#{client_name}\t#{session_name}:#{window_index}",
        ])
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_default()
    }

    /// Open an inline prompt for `id` and answer it, as fzf would: the
    /// `prompt` transform first, then `enter` with the typed query.
    fn answer(&self, kind: &str, id: &str, text: &str) -> String {
        let opened = self.tm(&["prompt", kind, id]);
        assert!(opened.starts_with("change-prompt("), "{opened}");
        self.tm_env(&["enter"], &[("FZF_QUERY", text)])
    }

    fn session_id(&self, name: &str) -> String {
        self.tmux(&[
            "display-message",
            "-p",
            "-t",
            &format!("{name}:"),
            "#{session_id}",
        ])
    }

    fn sessions(&self) -> Vec<String> {
        self.tmux(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Holds its stdin open: control mode detaches on end of input.
struct Client {
    child: Child,
    name: String,
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
        let _ = fs::remove_dir_all(&self.state);
    }
}

/// Text without its ANSI colour sequences.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('\x1b') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('m')
            .map_or(rest.len(), |i| start + i + 1);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn ids(rows: &str) -> Vec<String> {
    rows.lines()
        .map(|line| line.split('\t').next().unwrap().to_string())
        .collect()
}

fn names(rows: &str) -> Vec<String> {
    rows.lines()
        .map(|line| line.split('\t').nth(1).unwrap().to_string())
        .collect()
}

#[test]
fn rows_favorites_legend_and_refresh() {
    let Some(server) = Server::start("rows") else {
        return;
    };
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tmux(&["new-session", "-d", "-s", "gamma", "-x", "120", "-y", "40"]);
    server.tmux(&["new-window", "-d", "-t", "beta:"]);

    let rows = server.tm(&["list"]);
    assert_eq!(names(&rows), ["alpha", "beta", "gamma"]);
    // A mode given to `list` inside a popup prints that mode's rows and
    // leaves the popup alone: its snapshot still holds what fzf shows. Rows
    // are compared by id, since a window's name follows its command while
    // the shell starts.
    let snapshot = server.state.join("snap.txt");
    fs::write(&snapshot, "opened\n").unwrap();
    assert_eq!(server.tm(&["list"]), rows);
    assert_eq!(fs::read_to_string(&snapshot).unwrap(), rows);
    for mode in ["windows", "projects"] {
        server.tm(&["mode", &format!("--{mode}")]);
        let theirs = ids(&server.tm(&["list"]));
        server.tm(&["mode", "--sessions"]);
        server.tm(&["list"]);
        let before = fs::read_to_string(&snapshot).unwrap();
        let flagged = server.tm(&["list", &format!("--{mode}")]);
        assert_eq!(ids(&flagged), theirs, "{mode}");
        assert_eq!(fs::read_to_string(&snapshot).unwrap(), before, "{mode}");
        assert!(!server.state.join("snap.mode").exists(), "{mode}");
    }
    assert_eq!(
        server.tm_env(&["refresh"], &[("FZF_IDLE_TIME_MS", "1500")]),
        "",
        "the snapshot still matches the rows on show"
    );
    let beta: Vec<&str> = rows.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(beta[2].trim(), "beta");
    assert_eq!(
        beta[3].trim(),
        "beta",
        "sessions mode: both name columns alike"
    );
    assert!(beta[5].contains("2 windows"));

    server.tm(&["favorite", "gamma"]);
    let rows = server.tm(&["list"]);
    assert_eq!(names(&rows), ["gamma", "alpha", "beta"]);
    assert!(
        rows.lines()
            .next()
            .unwrap()
            .split('\t')
            .nth(2)
            .unwrap()
            .starts_with("★ gamma")
    );
    assert!(
        fs::read_to_string(server.state.join("switcher.json"))
            .unwrap()
            .contains("gamma")
    );
    server.tm(&["favorite", "gamma"]);
    assert_eq!(
        names(&server.tm(&["list"])),
        ["alpha", "beta", "gamma"],
        "a second star takes it back"
    );

    assert_eq!(server.tm(&["legend"]), "", "first toggle hides the legend");
    assert!(server.state.join("snap.help").exists());
    assert!(
        server.tm(&["legend"]).contains("ctrl-j/k move"),
        "second toggle restores it"
    );
    assert!(!server.state.join("snap.help").exists());

    let snapshot = server.state.join("snap.txt");
    fs::write(&snapshot, "stale\n").unwrap();
    let idle = [("FZF_IDLE_TIME_MS", "1500")];
    let refresh = || server.tm_env(&["refresh"], &idle);
    assert!(refresh().starts_with("reload-sync(cat "));
    assert_eq!(fs::read_to_string(&snapshot).unwrap(), server.tm(&["list"]));
    assert_eq!(refresh(), "", "unchanged rows do not reload");
    assert_eq!(
        server.tm_env(&["refresh"], &[("FZF_IDLE_TIME_MS", "200")]),
        "",
        "no reload while the user is moving"
    );
}

#[test]
fn inline_prompts_and_closing() {
    let Some(server) = Server::start("prompts") else {
        return;
    };
    let alpha = server.session_id("alpha");

    let opened = server.tm(&["prompt", "create", &alpha]);
    assert!(
        opened.contains("change-prompt(create> )") && opened.contains("change-query()"),
        "{opened}"
    );
    assert!(
        opened.contains("change-header(New session") && opened.contains("disable-search"),
        "{opened}"
    );
    assert!(server.state.join("snap.prompt").exists());

    let done = server.answer("create", &alpha, "delta");
    assert!(
        done.contains("change-prompt(sessions> )")
            && done.contains("enable-search")
            && done.contains("reload-sync("),
        "{done}"
    );
    assert!(
        !server.state.join("snap.prompt").exists(),
        "answering clears the pending prompt"
    );
    assert!(server.sessions().contains(&"delta".to_string()));
    let nothing = server.answer("create", &alpha, "");
    assert!(
        !nothing.contains("reload-sync"),
        "an empty name creates nothing: {nothing}"
    );

    let opened = server.tm(&["prompt", "rename", &alpha]);
    assert!(
        opened.contains("change-prompt(rename> )") && opened.contains("change-query(alpha)"),
        "prefilled: {opened}"
    );
    server.answer("rename", &alpha, "omega");
    assert!(
        server.sessions().contains(&"omega".to_string())
            && !server.sessions().contains(&"alpha".to_string())
    );

    let delta = server.session_id("delta");
    let closed = server.tm(&["close", &delta]);
    assert!(
        closed.starts_with("reload-sync("),
        "closing needs no confirmation: {closed}"
    );
    assert!(!server.sessions().contains(&"delta".to_string()));
    let missing = server.tm(&["close", "$999"]);
    assert!(missing.starts_with("change-header(tm:"), "{missing}");

    for prompt in ["windows> ", "projects> ", "sessions> "] {
        let switched = server.tm(&["mode"]);
        assert!(
            switched.starts_with(&format!("change-prompt({prompt})+reload-sync(")),
            "{switched}"
        );
    }

    // fzf hands over an empty {1} when nothing matches the filter; tmux would
    // read that as the current session, so the row keys must do nothing.
    let before = server.sessions();
    assert_eq!(server.tm(&["close", ""]), "");
    assert_eq!(server.tm(&["prompt", "rename", ""]), "");
    assert_eq!(server.tm(&["preview", ""]), "");
    server.tm(&["preview-next", ""]);
    assert_eq!(server.sessions(), before);

    // The filter typed before a prompt comes back afterwards.
    let omega = server.session_id("omega");
    let opened = server.tm_env(&["prompt", "rename", &omega], &[("FZF_QUERY", "ome")]);
    assert!(opened.contains("change-query(omega)"), "{opened}");
    let cancelled = server.tm(&["esc"]);
    assert!(cancelled.contains("change-query(ome)"), "{cancelled}");
    server.tm_env(&["prompt", "rename", &omega], &[("FZF_QUERY", "ome")]);
    let answered = server.tm_env(&["enter"], &[("FZF_QUERY", "omega")]);
    assert!(answered.contains("change-query(ome)"), "{answered}");

    // Tab cancels an open prompt along with switching the mode.
    server.tm(&["prompt", "rename", &omega]);
    let switched = server.tm(&["mode"]);
    assert!(
        switched.contains("change-prompt(windows> )") && switched.contains("change-header()"),
        "{switched}"
    );
    assert!(!server.state.join("snap.prompt").exists());
    server.tm(&["mode", "--sessions"]);

    assert_eq!(
        server.tm(&["enter"]).trim(),
        "accept",
        "Enter outside a prompt accepts the row"
    );
    assert_eq!(
        server.tm(&["esc"]).trim(),
        "abort",
        "Esc outside a prompt closes the popup"
    );
    server.tm(&["prompt", "rename", &server.session_id("omega")]);
    let cancelled = server.tm(&["esc"]);
    assert!(
        cancelled.contains("change-header()") && cancelled.contains("enable-search"),
        "{cancelled}"
    );
    assert!(
        !server.state.join("snap.prompt").exists(),
        "Esc drops the pending prompt"
    );

    // The tree hides a window's session; filtering shows the breadcrumbs.
    assert_eq!(server.tm(&["with-nth"]).trim(), "3,5,6");
    let filtering = server.tm_env(&["with-nth"], &[("FZF_QUERY", "om")]);
    assert_eq!(filtering.trim(), "4,5,6");
    // While a prompt borrows the query line, the filter it saved decides.
    server.tm(&["prompt", "rename", &server.session_id("omega")]);
    let typing = server.tm_env(&["with-nth"], &[("FZF_QUERY", "omeg")]);
    assert_eq!(typing.trim(), "3,5,6");
    server.tm(&["esc"]);
}

#[test]
fn windows_mode_lists_manages_and_previews_windows() {
    let Some(server) = Server::start("windows") else {
        return;
    };
    server.tmux(&["new-window", "-d", "-t", "alpha:", "-n", "editor"]);
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tm(&["mode", "--windows"]);

    let rows = server.tm(&["list"]);
    let ids = ids(&rows);
    assert_eq!(
        ids.len(),
        5,
        "alpha, its two windows, beta, its window: {rows}"
    );
    assert!(
        ids[0].starts_with('$')
            && ids[1].starts_with('@')
            && ids[2].starts_with('@')
            && ids[3].starts_with('$')
    );
    let header: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert!(
        header[2].contains("\x1b[1malpha"),
        "session rows are bold headers: {:?}",
        header[2]
    );
    // The layout itself is unit-tested in rows/; here only that real windows
    // flow through it.
    let editor: Vec<&str> = rows.lines().nth(2).unwrap().split('\t').collect();
    assert_eq!(
        editor[1], "alpha",
        "window rows carry their session name for ctrl-s"
    );
    assert_eq!(plain(editor[2]).trim(), "└ 1   editor");

    let editor_id = ids[2].clone();
    let preview = server.tm(&["preview", &editor_id]);
    assert!(preview.contains("alpha  ·  1: editor"), "{preview:?}");
    server.tm(&["preview-next", &editor_id]);
    assert!(
        server.tm(&["preview", &editor_id]).contains("1: editor"),
        "window rows do not cycle"
    );

    let opened = server.tm(&["prompt", "rename", &editor_id]);
    assert!(
        opened.contains("change-query(editor)") && opened.contains("Rename window 1: editor"),
        "{opened}"
    );
    server.answer("rename", &editor_id, "code");
    assert_eq!(
        server.tmux(&["display-message", "-p", "-t", &editor_id, "#{window_name}"]),
        "code"
    );

    let beta = server.session_id("beta");
    let opened = server.tm(&["prompt", "create", &beta]);
    assert!(opened.contains("New window in beta"), "{opened}");
    server.answer("create", &beta, "logs");
    assert_eq!(
        server
            .tmux(&["list-windows", "-t", "beta", "-F", "#{window_name}"])
            .lines()
            .count(),
        2
    );
    server.answer("create", &beta, "");
    assert_eq!(
        server
            .tmux(&["list-windows", "-t", "beta", "-F", "#{window_name}"])
            .lines()
            .count(),
        3,
        "an unnamed window is fine"
    );

    server.tmux(&["split-window", "-d", "-t", &editor_id]);
    let listed = server.tm(&["list"]);
    let split: Vec<&str> = listed.lines().nth(2).unwrap().split('\t').collect();
    assert!(split[5].contains("2 panes"), "{:?}", split[5]);

    assert!(
        server
            .tm(&["close", &editor_id])
            .starts_with("reload-sync(")
    );
    assert_eq!(
        server
            .tmux(&["list-windows", "-t", "alpha", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );
    let last = server.tmux(&["display-message", "-p", "-t", "alpha:0", "#{window_id}"]);
    assert!(
        server.tm(&["close", &last]).starts_with("reload-sync("),
        "closing the last window"
    );
    assert!(
        !server.sessions().contains(&"alpha".to_string()),
        "which ends its session"
    );

    let renamed = server.answer("rename", &beta, "gamma");
    assert!(renamed.contains("reload-sync("), "{renamed}");
    assert!(server.sessions().contains(&"gamma".to_string()));

    server.tm(&["mode", "--sessions"]);
    assert!(!server.state.join("snap.mode").exists());
    assert_eq!(
        server.tm(&["list"]).lines().count(),
        1,
        "sessions mode again, alpha gone"
    );
    server.tm(&["mode", "--windows"]);
    assert_eq!(
        server.tm(&["list"]).lines().count(),
        4,
        "gamma and its three windows"
    );
}

#[test]
fn preview_cycles_without_changing_the_active_window() {
    let Some(server) = Server::start("preview") else {
        return;
    };
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tmux(&["new-window", "-d", "-t", "beta:"]);
    let alpha = server.session_id("alpha");
    let beta = server.session_id("beta");

    let preview = server.tm(&["preview", &alpha]);
    assert!(
        preview.contains("alpha") && preview.contains("0:") && !preview.contains("\x1b[2J"),
        "{preview:?}"
    );
    assert!(
        !preview.ends_with('\n'),
        "no trailing newline, or fzf shows a scroll indicator"
    );

    let beta_preview = || server.tm(&["preview", &beta]);
    server.tm(&["preview-next", &beta]);
    assert!(beta_preview().contains("1:"), "moves on");
    server.tm(&["preview-next", &beta]);
    assert!(beta_preview().contains("0:"), "wraps");
    assert_eq!(
        server.tmux(&["display-message", "-p", "-t", "beta:", "#{window_index}"]),
        "0",
        "tmux's active window is untouched"
    );
    server.tm(&["preview-next", &beta]);
    server.tmux(&["kill-window", "-t", "beta:1"]);
    assert!(
        beta_preview().contains("0:"),
        "a closed preview window falls back to the active one"
    );
    assert!(
        server.tm(&["preview", "$999"]).contains("Gone"),
        "a vanished row previews a notice"
    );

    fs::write(server.state.join("snap.view"), "hide-preview").unwrap();
    assert_eq!(server.tm(&["view-return"]).trim(), "hide-preview");
    assert!(
        !server.state.join("snap.view").exists(),
        "the action is replayed once"
    );
}

#[test]
fn agent_activity_from_hooks_and_from_the_screen() {
    let Some(server) = Server::start("agent") else {
        return;
    };
    // A shell whose argv[0] is ".../claude": `ps` shows it as an agent, and
    // hooks typed into it run as its children, the way real hooks do. tmux
    // names its command after what the system says runs: `bash` on macOS,
    // where /bin/sh is bash, `claude` on Linux. Neither is `sh`, so with that
    // as the default shell the pre-filter never takes it for a pane at its
    // prompt, and the test goes through the filter as real panes do. `exec`,
    // because dash, Linux's /bin/sh, forks for `sh -c <command>` where bash
    // execs it, and the pane's pid must be the agent's.
    let fake = server.state.join("claude");
    std::os::unix::fs::symlink("/bin/sh", &fake).unwrap();
    server.tmux(&["set", "-g", "default-shell", "/bin/sh"]);
    server.tmux(&[
        "new-window",
        "-d",
        "-t",
        "alpha:",
        "-n",
        "agent",
        &format!("exec {}", fake.to_str().unwrap()),
    ]);
    let screen = |target: &str| server.tmux(&["capture-pane", "-p", "-t", target]);
    assert!(
        eventually(|| !screen("alpha:1").is_empty()),
        "the fake agent never showed its prompt"
    );
    let pane = server.tmux(&["display-message", "-p", "-t", "alpha:1", "#{pane_id}"]);
    let pid = server.tmux(&["display-message", "-p", "-t", "alpha:1", "#{pane_pid}"]);

    let activity = || server.tm(&["activity"]).trim().to_string();
    assert_eq!(
        activity(),
        r#"{"@0":"plain","@1":"waiting"}"#,
        "an agent with no busy hint is waiting"
    );

    let record = || server.tmux(&["show-options", "-pqv", "-t", &pane, "@tm-agent"]);
    // The shipped hooks pass a JSON payload on stdin; the event may also be
    // named on the command line.
    let hook = |event: &str, on_stdin: bool| {
        let command = if on_stdin {
            format!("printf '{{\"hook_event_name\":\"{event}\"}}' | {TM} hook claude")
        } else {
            format!("{TM} hook claude {event}")
        };
        server.tmux(&["send-keys", "-t", &pane, &command, "Enter"]);
        assert!(
            eventually(|| record().contains(&format!("\"event\":\"{event}\""))),
            "hook {event} left no record: {}",
            record()
        );
    };
    hook("UserPromptSubmit", true);
    assert!(
        record().contains("\"working\"") && record().contains(&format!("\"pid\":{pid}")),
        "{}",
        record()
    );
    assert_eq!(activity(), r#"{"@0":"plain","@1":"working"}"#);
    let session_dots = || {
        let rows = server.tm(&["list"]);
        rows.lines()
            .next()
            .unwrap()
            .split('\t')
            .nth(4)
            .unwrap()
            .to_string()
    };
    assert_eq!(
        session_dots(),
        "\x1b[32m●\x1b[0m ",
        "a working agent is one plain green dot"
    );
    hook("Stop", true);
    assert_eq!(activity(), r#"{"@0":"plain","@1":"waiting"}"#);
    assert_eq!(
        session_dots(),
        "\x1b[33m●\x1b[0m ",
        "a waiting agent is one plain yellow dot"
    );

    server.tmux(&["send-keys", "-t", &pane, "esc to interrupt", "Enter"]);
    assert!(eventually(|| screen(&pane).contains("esc to interrupt")));
    hook("PermissionRequest", false);
    assert_eq!(
        activity(),
        r#"{"@0":"plain","@1":"working"}"#,
        "a busy hint after a permission hook means it resumed"
    );

    server.tmux(&[
        "set-option",
        "-p",
        "-t",
        &pane,
        "@tm-agent",
        r#"{"state":"waiting","pid":1,"started":"never","harness":"claude","event":"Stop"}"#,
    ]);
    assert_eq!(
        activity(),
        r#"{"@0":"plain","@1":"working"}"#,
        "a record for another process is ignored"
    );

    hook("SessionEnd", true);
    assert_eq!(activity(), r#"{"@0":"plain","@1":"plain"}"#);
    assert_eq!(
        session_dots().matches('●').count(),
        0,
        "an ended agent leaves no dot"
    );
}

#[test]
fn projects_mode_lists_and_opens_directories() {
    let Some(server) = Server::start("projects") else {
        return;
    };
    let root = server.state.join("projects");
    for dir in ["alpha-app", "beta.app", "nested/alpha-app", ".hidden"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(root.join("a-file"), "").unwrap();
    server.tmux(&[
        "set",
        "-g",
        "@tm-projects",
        &format!("{}:2", root.display()),
    ]);
    let root = fs::canonicalize(&root).unwrap();

    server.tm(&["mode", "--projects"]);
    assert_eq!(
        fs::read_to_string(server.state.join("snap.mode")).unwrap(),
        "projects"
    );
    let rows = server.tm(&["list"]);
    assert_eq!(
        names(&rows),
        ["alpha-app", "alpha-app", "beta_app", "nested"],
        "{rows}"
    );
    let ids = ids(&rows);
    assert_eq!(
        ids[0],
        root.join("alpha-app").to_str().unwrap(),
        "a project with no session has its path for an id"
    );
    assert_eq!(ids[1], root.join("nested/alpha-app").to_str().unwrap());
    let alpha: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert_eq!(plain(alpha[4]).trim(), "", "no session, no dots");
    assert_eq!(
        plain(alpha[5]),
        root.to_str().unwrap(),
        "the badge says where it lives"
    );

    // Enter on a project row (prefix + f): its session, made in the
    // directory, and the client goes there.
    let client = server.attach("alpha");
    let enter = |pick: &Path, query: &Path| {
        let (pick, query) = (pick.to_str().unwrap(), query.to_str().unwrap());
        server.pick(&["--client", &client.name, "--projects"], pick, query);
    };
    let top = root.join("alpha-app");
    enter(&top, Path::new(""));
    assert_eq!(server.showing(&client), "alpha-app:0");
    let id = server.session_id("alpha-app");
    assert!(id.starts_with('$'), "{id}");
    assert_eq!(
        server.tmux(&["display-message", "-p", "-t", &id, "#{session_path}"]),
        top.to_str().unwrap()
    );
    // A typed path that matches no row opens that directory: the same
    // session, found by where it started.
    server.tmux(&["switch-client", "-c", &client.name, "-t", "alpha"]);
    enter(Path::new(""), &top);
    assert_eq!(server.showing(&client), "alpha-app:0");
    assert_eq!(server.sessions(), ["alpha", "alpha-app"]);
    // A namesake started elsewhere gets its parent's name appended.
    let nested = root.join("nested/alpha-app");
    enter(&nested, Path::new(""));
    assert_eq!(server.showing(&client), "alpha-app-nested:0");

    let rows = server.tm(&["list"]);
    assert_eq!(
        names(&rows),
        ["alpha-app", "alpha-app-nested", "beta_app", "nested"],
        "open projects first: {rows}"
    );
    assert_eq!(
        rows.lines().count(),
        4,
        "sessions outside the roots are not projects"
    );
    let first: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert_eq!(first[0], id, "an open project is its session's row");
    assert_eq!(first[4].matches('●').count(), 0, "no agent, no dot");
    assert!(plain(first[5]).contains("1 window"), "{:?}", first[5]);

    server.tm(&["favorite", "beta_app"]);
    let rows = server.tm(&["list"]);
    assert_eq!(
        names(&rows),
        ["beta_app", "alpha-app", "alpha-app-nested", "nested"],
        "starred first: {rows}"
    );
    assert!(
        plain(rows.lines().next().unwrap().split('\t').nth(2).unwrap()).starts_with("★ beta_app")
    );

    // A project with no session previews its directory, and the row keys
    // leave it alone.
    let beta = root.join("beta.app");
    fs::create_dir(beta.join("src")).unwrap();
    fs::write(beta.join("notes.md"), "").unwrap();
    let preview = server.tm_env(
        &["preview", beta.to_str().unwrap()],
        &[("FZF_PREVIEW_COLUMNS", "512")],
    );
    assert!(
        preview.contains("beta.app")
            && plain(&preview).contains("src/")
            && preview.contains("notes.md"),
        "{preview:?}"
    );
    assert_eq!(server.tm(&["close", beta.to_str().unwrap()]), "");
    assert_eq!(server.tm(&["prompt", "rename", beta.to_str().unwrap()]), "");
    server.tm(&["preview-next", beta.to_str().unwrap()]);
    assert!(server.sessions().contains(&"alpha-app".to_string()));
    // An open project's row is its session's row, so ctrl-x closes it.
    assert!(server.tm(&["close", &id]).starts_with("reload-sync("));
    assert!(!server.sessions().contains(&"alpha-app".to_string()));

    // tab cycles sessions → windows → projects → sessions.
    server.tm(&["mode", "--sessions"]);
    for prompt in ["windows> ", "projects> ", "sessions> "] {
        let switched = server.tm(&["mode"]);
        assert!(
            switched.contains(&format!("change-prompt({prompt})")),
            "{switched}"
        );
    }
}

#[test]
fn shell_runs_keep_nothing() {
    let Some(server) = Server::start("shell") else {
        return;
    };
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tmux(&["new-window", "-d", "-t", "beta:"]);
    let shell = |args: &[&str]| server.tm_shell(args);
    let alpha = server.session_id("alpha");
    let beta = server.session_id("beta");

    // Each run from a shell is the first key of a popup of its own, which
    // closes when the run ends: it prints what fzf would be given, and no
    // later run sees what it did.
    let sessions = shell(&["list"]);
    assert_eq!(names(&sessions), ["alpha", "beta"]);
    assert_eq!(shell(&["list", "--windows"]).lines().count(), 5);
    assert_eq!(shell(&["list", "--projects"]).trim(), "", "no roots set");
    for _ in 0..2 {
        let toggled = shell(&["mode"]);
        assert!(
            toggled.starts_with("change-prompt(windows> )+reload-sync("),
            "{toggled}"
        );
    }
    shell(&["mode", "--projects"]);
    assert_eq!(shell(&["list"]), sessions, "the mode went with its run");

    let opened = shell(&["prompt", "rename", &beta]);
    assert!(
        opened.starts_with("change-prompt(rename> )+change-query(beta)"),
        "{opened}"
    );
    let entered = server.run_tm(&["enter"], &[("FZF_QUERY", "stolen")]);
    assert_eq!(entered.trim(), "accept", "no prompt outlives its run");
    assert_eq!(server.sessions(), ["alpha", "beta"]);
    assert_eq!(shell(&["esc"]).trim(), "abort");
    assert_eq!(shell(&["legend"]), "", "a fresh popup shows the legend");
    assert_eq!(shell(&["legend"]), "", "so every run hides it");
    assert_eq!(shell(&["with-nth"]).trim(), "3,5,6");
    let filtering = server.run_tm(&["with-nth"], &[("FZF_QUERY", "be")]);
    assert_eq!(filtering.trim(), "4,5,6");
    shell(&["preview-next", &beta]);
    let title = plain(&shell(&["preview", &beta]));
    assert!(
        title.lines().next().unwrap().contains("·  0:"),
        "the choice went too: {title:?}"
    );
    assert_eq!(shell(&["view-return"]).trim(), "");
    assert_eq!(shell(&["refresh"]), "", "no rows on show");
    assert!(shell(&["activity"]).starts_with('{'));

    // What a person asks for from a shell does happen.
    shell(&["favorite", "beta"]);
    assert_eq!(names(&shell(&["list"])), ["beta", "alpha"]);
    shell(&["favorite", "beta"]);
    server.tmux(&["new-session", "-d", "-s", "gone"]);
    assert!(shell(&["close", &server.session_id("gone")]).starts_with("reload-sync("));
    assert!(shell(&["close", "$999"]).starts_with("change-header(tm:"));
    assert_eq!(server.sessions(), ["alpha", "beta"]);
    assert!(shell(&["preview", &alpha]).contains("alpha"));

    // An empty TM_SNAPSHOT is no popup either.
    let emptied = server.run_tm(&["mode", "--windows"], &[("TM_SNAPSHOT", "")]);
    assert!(emptied.starts_with("change-prompt(windows> )"), "{emptied}");

    assert_eq!(
        files(&server.state),
        ["switcher.json"],
        "only the stars reach the disk"
    );
}

#[test]
fn the_picker_takes_the_client_to_the_pick() {
    let Some(server) = Server::start("picker") else {
        return;
    };
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tmux(&["new-window", "-d", "-t", "beta:"]);
    let client = server.attach("alpha");
    let beta = server.session_id("beta");

    // `tm` at a prompt inside tmux: the client showing the session switches.
    // With no project roots it starts on the sessions.
    server.pick(&[], &beta, "");
    assert_eq!(server.showing(&client), "beta:0");
    let (args, rows) = server.fzf();
    assert!(args.contains("\nsessions> \n"), "{args}");
    assert_eq!(names(&rows), ["alpha", "beta"]);

    // A window row takes the client to the window.
    let window = server.tmux(&["display-message", "-p", "-t", "beta:1", "#{window_id}"]);
    server.pick(&["--windows"], &window, "");
    assert_eq!(server.showing(&client), "beta:1");
    // Opened for a client, the window list starts on the window it shows.
    server.pick(&["--client", &client.name, "--windows"], "", "");
    let (args, rows) = server.fzf();
    let at = rows
        .lines()
        .position(|row| row.starts_with(&window))
        .unwrap();
    assert!(args.contains(&format!("load:pos({})", at + 1)), "{args}");
    assert_eq!(server.showing(&client), "beta:1", "nothing picked");

    // Enter on no match makes the session it names.
    server.pick(&["--client", &client.name], "", "gamma");
    assert_eq!(server.showing(&client), "gamma:0");
    assert_eq!(server.sessions(), ["alpha", "beta", "gamma"]);

    // The picker leaves no file behind.
    assert_eq!(files(&server.state.join("run")), Vec::<String>::new());
}

/// A directory standing in for a machine with no tmux running: a home whose
/// tmux config names the project roots, and `TMUX_TMPDIR` for the sockets.
/// Dropping it stops whatever server tm started there.
struct Outside {
    dir: PathBuf,
}

impl Outside {
    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(args)
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &self.dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    }

    /// `tm` typed in a terminal outside tmux, the fake fzf picking `pick`
    /// or leaving `query` typed. Its stdin is not a terminal, so the
    /// `tmux attach` that tm hands over to fails, and says so.
    fn tm(&self, pick: &str, query: &str) -> Output {
        let mut command = Command::new(TM);
        command
            .env_remove("TMUX")
            .env_remove("XDG_CONFIG_HOME")
            .env("TMUX_TMPDIR", &self.dir)
            .env("HOME", self.dir.join("home"))
            .env("TM_STATE_DIR", self.dir.join("state"));
        for var in POPUP_VARS {
            command.env_remove(var);
        }
        let output = picker(&mut command, &self.dir, pick, query);
        assert!(
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains("not a terminal"),
            "tm did not hand over to tmux attach: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

impl Drop for Outside {
    fn drop(&mut self) {
        self.tmux(&["kill-server"]);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn outside_tmux_tm_starts_tmux_on_the_pick() {
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("tmux is not installed; skipping");
        return;
    }
    // A short path: a socket's must fit in about a hundred bytes.
    let dir = std::env::temp_dir().join(format!("tm-out-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let outside = Outside { dir: dir.clone() };
    fs::create_dir_all(dir.join("home")).unwrap();
    fs::create_dir_all(dir.join("projects/web")).unwrap();
    let root = fs::canonicalize(dir.join("projects")).unwrap();
    fs::write(
        dir.join("home/.tmux.conf"),
        format!("set -g @tm-projects '{}'\n", root.display()),
    )
    .unwrap();
    let fzf = |name: &str| fs::read_to_string(dir.join("bin").join(name)).unwrap();
    assert_eq!(outside.tmux(&["list-sessions"]), "", "no server yet");

    // With no server, tm starts one to read the roots from the config,
    // lists the projects, and on Enter makes the session and attaches.
    let web = root.join("web");
    outside.tm(web.to_str().unwrap(), "");
    assert!(fzf("fzf.args").contains("\nprojects> \n"));
    assert!(fzf("fzf.rows").starts_with(&format!("{}\t", web.display())));
    assert_eq!(
        outside.tmux(&["list-sessions", "-F", "#{session_name} #{session_path}"]),
        format!("web {}", web.display())
    );

    // With the server up, the project's row is its session's.
    let id = outside.tmux(&["display-message", "-p", "-t", "web:", "#{session_id}"]);
    outside.tm(&id, "");
    assert!(fzf("fzf.rows").starts_with(&format!("{id}\t")));
    // A name with no match is a new session.
    outside.tm("", "scratch");
    assert_eq!(
        outside.tmux(&["list-sessions", "-F", "#{session_name}"]),
        "scratch\nweb"
    );
    assert_eq!(files(&dir.join("state/run")), Vec::<String>::new());
}
