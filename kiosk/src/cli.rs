use anyhow::Context;
use kiosk_core::{
    config::Config,
    git::{GitProvider, Repo},
    pending_delete::{
        PendingWorktreeDelete, load_pending_worktree_deletes, save_pending_worktree_deletes,
    },
    state::{BranchEntry, worktree_dir},
    tmux::TmuxProvider,
};
use serde::Serialize;
use std::{collections::HashSet, fmt::Write, fs, path::PathBuf, process::Command};

pub type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Clone)]
pub struct CliError {
    message: String,
    code: i32,
}

impl CliError {
    pub fn user(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }

    pub fn system(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn code(&self) -> i32 {
        self.code
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

impl From<anyhow::Error> for CliError {
    fn from(value: anyhow::Error) -> Self {
        Self::system(value.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct OpenArgs {
    pub repo: String,
    pub branch: Option<String>,
    pub new_branch: Option<String>,
    pub base: Option<String>,
    pub no_switch: bool,
    pub run: Option<String>,
    pub log: bool,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct StatusArgs {
    pub repo: String,
    pub branch: Option<String>,
    pub json: bool,
    pub lines: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct DeleteArgs {
    pub repo: String,
    pub branch: String,
    pub force: bool,
    pub json: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RepoOutput {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct OpenOutput {
    session: String,
    path: PathBuf,
    created: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct StatusOutput {
    session: String,
    path: PathBuf,
    attached: bool,
    clients: usize,
    output: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionOutput {
    session: String,
    repo: String,
    branch: Option<String>,
    path: PathBuf,
    attached: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DeleteOutput {
    deleted: bool,
    repo: String,
    branch: String,
    session: String,
}

pub fn resolve_repo_exact<'a>(repos: &'a [Repo], name: &str) -> CliResult<&'a Repo> {
    repos.iter().find(|repo| repo.name == name).ok_or_else(|| {
        let available = repos
            .iter()
            .map(|repo| repo.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        if available.is_empty() {
            CliError::user(format!("no repo named '{name}' found. Available: (none)"))
        } else {
            CliError::user(format!(
                "no repo named '{name}' found. Available: {available}"
            ))
        }
    })
}

pub fn cmd_list(config: &Config, git: &dyn GitProvider, json: bool) -> CliResult<()> {
    let repos = git.discover_repos(&config.resolved_search_dirs());
    let output: Vec<RepoOutput> = repos
        .into_iter()
        .map(|repo| RepoOutput {
            name: repo.name,
            path: repo.path,
        })
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|e| CliError::system(e.to_string()))?
        );
    } else {
        print!("{}", format_repo_table(&output));
    }

    Ok(())
}

pub fn cmd_branches(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    repo: &str,
    json: bool,
) -> CliResult<()> {
    let repos = git.discover_repos(&config.resolved_search_dirs());
    let repo = resolve_repo_exact(&repos, repo)?;

    let mut repo = repo.clone();
    repo.worktrees = git.list_worktrees(&repo.path);

    let local = git.list_branches(&repo.path);
    let active_sessions = tmux.list_sessions();
    let mut entries = BranchEntry::build_sorted(&repo, &local, &active_sessions);
    let remote = BranchEntry::build_remote(&git.list_remote_branches(&repo.path), &local);
    entries.extend(remote);
    BranchEntry::sort_entries(&mut entries);

    if json {
        println!(
            "{}",
            serde_json::to_string(&entries).map_err(|e| CliError::system(e.to_string()))?
        );
    } else {
        print!("{}", format_branch_table(&entries));
    }

    Ok(())
}

pub fn cmd_open(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    args: &OpenArgs,
) -> CliResult<()> {
    let output = open_internal(config, git, tmux, args)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|e| CliError::system(e.to_string()))?
        );
    } else {
        println!("session: {}", output.session);
        println!("path: {}", output.path.display());
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn open_internal(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    args: &OpenArgs,
) -> CliResult<OpenOutput> {
    if args.branch.is_some() && args.new_branch.is_some() {
        return Err(CliError::user(
            "cannot use positional branch and --new-branch together",
        ));
    }
    if args.base.is_some() && args.new_branch.is_none() {
        return Err(CliError::user(
            "--base can only be used together with --new-branch",
        ));
    }
    if args.new_branch.is_some() && args.base.is_none() {
        return Err(CliError::user("--new-branch requires --base"));
    }
    if !args.no_switch && !tmux.is_inside_tmux() {
        return Err(CliError::user(
            "not inside tmux. Use --no-switch to create the session without switching",
        ));
    }

    let repos = git.discover_repos(&config.resolved_search_dirs());
    let repo = resolve_repo_exact(&repos, &args.repo)?;

    let mut repo = repo.clone();
    repo.worktrees = git.list_worktrees(&repo.path);
    let local = git.list_branches(&repo.path);
    let remote = git.list_remote_branches(&repo.path);

    let mut created = false;
    let (worktree_path, session_name) = if let Some(new_branch) = &args.new_branch {
        if local.iter().any(|branch| branch == new_branch)
            || remote.iter().any(|branch| branch == new_branch)
        {
            return Err(CliError::user(format!(
                "branch '{new_branch}' already exists"
            )));
        }

        let base = args
            .base
            .as_deref()
            .ok_or_else(|| CliError::user("--new-branch requires --base"))?;
        if !local.iter().any(|branch| branch == base) {
            return Err(CliError::user(format!("base branch '{base}' not found")));
        }

        let wt = worktree_dir(&repo, new_branch).map_err(CliError::from)?;
        git.create_branch_and_worktree(&repo.path, new_branch, base, &wt)
            .map_err(CliError::from)?;
        created = true;
        let session = repo.tmux_session_name(&wt);
        (wt, session)
    } else if let Some(branch) = &args.branch {
        if let Some(existing) = find_worktree_by_branch(&repo, branch) {
            let session = repo.tmux_session_name(&existing);
            (existing, session)
        } else if local.iter().any(|name| name == branch) {
            let wt = worktree_dir(&repo, branch).map_err(CliError::from)?;
            git.add_worktree(&repo.path, branch, &wt)
                .map_err(CliError::from)?;
            created = true;
            let session = repo.tmux_session_name(&wt);
            (wt, session)
        } else if remote.iter().any(|name| name == branch) {
            let wt = worktree_dir(&repo, branch).map_err(CliError::from)?;
            git.create_tracking_branch_and_worktree(&repo.path, branch, &wt)
                .map_err(CliError::from)?;
            created = true;
            let session = repo.tmux_session_name(&wt);
            (wt, session)
        } else {
            return Err(CliError::user(format!(
                "branch '{branch}' not found. Use --new-branch to create it"
            )));
        }
    } else {
        let wt = repo.path.clone();
        let session = repo.tmux_session_name(&wt);
        (wt, session)
    };

    if !tmux.session_exists(&session_name) {
        tmux.create_session(
            &session_name,
            &worktree_path,
            config.session.split_command.as_deref(),
        )
        .map_err(CliError::from)?;
        created = true;
    }

    if args.log {
        let log_path = log_path_for_session(&session_name)?;
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).map_err(|e| CliError::system(e.to_string()))?;
        }
        tmux.pipe_pane(&session_name, &log_path)
            .map_err(CliError::from)?;
    }

    if let Some(command) = &args.run {
        tmux.send_keys(&session_name, command)
            .map_err(CliError::from)?;
    }

    if !args.no_switch {
        tmux.switch_to_session(&session_name);
    }

    Ok(OpenOutput {
        session: session_name,
        path: worktree_path,
        created,
    })
}

pub fn cmd_status(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    args: &StatusArgs,
) -> CliResult<()> {
    let output = status_internal(config, git, tmux, args)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|e| CliError::system(e.to_string()))?
        );
    } else {
        println!("session: {}", output.session);
        println!("path: {}", output.path.display());
        println!("attached: {}", output.attached);
        println!("clients: {}", output.clients);
        println!("output:\n{}", output.output);
    }

    Ok(())
}

fn status_internal(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    args: &StatusArgs,
) -> CliResult<StatusOutput> {
    let repos = git.discover_repos(&config.resolved_search_dirs());
    let repo = resolve_repo_exact(&repos, &args.repo)?;

    let mut repo = repo.clone();
    repo.worktrees = git.list_worktrees(&repo.path);

    let worktree_path = if let Some(branch) = &args.branch {
        find_worktree_by_branch(&repo, branch)
            .ok_or_else(|| CliError::user(format!("no worktree for branch '{branch}'")))?
    } else {
        repo.path.clone()
    };

    let lines = args.lines.unwrap_or(50).max(1);
    let session_name = repo.tmux_session_name(&worktree_path);
    let session_exists = tmux.session_exists(&session_name);

    let (output, clients) = if session_exists {
        let captured = tmux
            .capture_pane(&session_name, lines)
            .map_err(CliError::from)?;
        let clients = tmux.list_clients(&session_name);
        (captured, clients)
    } else {
        let log_path = log_path_for_session(&session_name)?;
        if !log_path.exists() {
            return Err(CliError::user(format!(
                "session '{session_name}' does not exist"
            )));
        }
        let log = fs::read_to_string(&log_path)
            .with_context(|| format!("failed to read log file {}", log_path.display()))
            .map_err(CliError::from)?;
        (tail_lines(&log, lines), Vec::new())
    };

    Ok(StatusOutput {
        session: session_name,
        path: worktree_path,
        attached: !clients.is_empty(),
        clients: clients.len(),
        output,
    })
}

pub fn cmd_sessions(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    json: bool,
) -> CliResult<()> {
    let repos = git.discover_repos(&config.resolved_search_dirs());
    let active_sessions: HashSet<String> = tmux.list_sessions().into_iter().collect();
    let mut output = Vec::new();

    for mut repo in repos {
        repo.worktrees = git.list_worktrees(&repo.path);
        for worktree in &repo.worktrees {
            let session = repo.tmux_session_name(&worktree.path);
            if !active_sessions.contains(&session) {
                continue;
            }
            output.push(SessionOutput {
                session: session.clone(),
                repo: repo.name.clone(),
                branch: worktree.branch.clone(),
                path: worktree.path.clone(),
                attached: !tmux.list_clients(&session).is_empty(),
            });
        }
    }

    output.sort_by(|left, right| left.session.cmp(&right.session));

    if json {
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|e| CliError::system(e.to_string()))?
        );
    } else {
        print!("{}", format_session_table(&output));
    }

    Ok(())
}

pub fn cmd_delete(
    config: &Config,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
    args: &DeleteArgs,
) -> CliResult<()> {
    let repos = git.discover_repos(&config.resolved_search_dirs());
    let repo = resolve_repo_exact(&repos, &args.repo)?;

    let mut repo = repo.clone();
    repo.worktrees = git.list_worktrees(&repo.path);
    let local = git.list_branches(&repo.path);
    let sessions = tmux.list_sessions();
    let entries = BranchEntry::build_sorted(&repo, &local, &sessions);

    let entry = entries
        .iter()
        .find(|entry| entry.name == args.branch)
        .ok_or_else(|| CliError::user(format!("branch '{}' not found", args.branch)))?;

    let worktree_path = entry
        .worktree_path
        .as_ref()
        .ok_or_else(|| CliError::user(format!("no worktree for branch '{}'", args.branch)))?;

    if entry.is_current {
        return Err(CliError::user(
            "cannot delete the current branch's worktree",
        ));
    }

    let session_name = repo.tmux_session_name(worktree_path);
    if tmux.session_exists(&session_name) {
        let clients = tmux.list_clients(&session_name);
        if !clients.is_empty() && !args.force {
            return Err(CliError::user(format!(
                "session '{session_name}' is attached. Use --force"
            )));
        }
        tmux.kill_session(&session_name);
    }
    let log_path = log_path_for_session(&session_name)?;
    if log_path.exists() {
        fs::remove_file(&log_path)
            .with_context(|| format!("failed to remove log file {}", log_path.display()))
            .map_err(CliError::from)?;
    }

    let mut pending = load_pending_worktree_deletes();
    if pending
        .iter()
        .any(|entry| entry.repo_path == repo.path && entry.branch_name == args.branch)
    {
        return Err(CliError::user("worktree deletion already in progress"));
    }

    pending.push(PendingWorktreeDelete::new(
        repo.path.clone(),
        args.branch.clone(),
        worktree_path.clone(),
    ));
    save_pending_worktree_deletes(&pending).map_err(CliError::from)?;

    let remove_result = git.remove_worktree(worktree_path);

    pending.retain(|entry| !(entry.repo_path == repo.path && entry.branch_name == args.branch));
    save_pending_worktree_deletes(&pending).map_err(CliError::from)?;

    remove_result.map_err(CliError::from)?;
    let prune = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&repo.path)
        .output()
        .map_err(|e| CliError::system(e.to_string()))?;
    if !prune.status.success() {
        return Err(CliError::system(format!(
            "git worktree prune failed: {}",
            String::from_utf8_lossy(&prune.stderr).trim()
        )));
    }

    let output = DeleteOutput {
        deleted: true,
        repo: repo.name.clone(),
        branch: args.branch.clone(),
        session: session_name,
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|e| CliError::system(e.to_string()))?
        );
    } else {
        println!("deleted: {} {}", repo.name, args.branch);
    }

    Ok(())
}

fn find_worktree_by_branch(repo: &Repo, branch: &str) -> Option<PathBuf> {
    repo.worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .map(|worktree| worktree.path.clone())
}

fn log_path_for_session(session_name: &str) -> CliResult<PathBuf> {
    Ok(log_dir()?.join(format!("{session_name}.log")))
}

fn tail_lines(content: &str, lines: usize) -> String {
    let mut selected = content.lines().rev().take(lines).collect::<Vec<_>>();
    selected.reverse();
    selected.join("\n")
}

fn format_repo_table(repos: &[RepoOutput]) -> String {
    let name_header = "repo";
    let path_header = "path";
    let name_width = repos
        .iter()
        .map(|repo| repo.name.len())
        .max()
        .unwrap_or(name_header.len())
        .max(name_header.len());

    let mut out = String::new();
    let _ = writeln!(out, "{name_header:<name_width$}  {path_header}");
    for repo in repos {
        let _ = writeln!(out, "{:<name_width$}  {}", repo.name, repo.path.display());
    }
    out
}

fn format_branch_table(entries: &[BranchEntry]) -> String {
    let branch_header = "branch";
    let stat_header = "stat";
    let worktree_header = "worktree";
    let branch_width = entries
        .iter()
        .map(|entry| entry.name.len())
        .max()
        .unwrap_or(branch_header.len())
        .max(branch_header.len());
    let stat_width = stat_header.len().max(4);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{branch_header:<branch_width$}  {stat_header:<stat_width$}  {worktree_header}"
    );
    for entry in entries {
        let stat = format!(
            "{}{}{}{}",
            if entry.is_current { '*' } else { '-' },
            if entry.worktree_path.is_some() {
                'W'
            } else {
                '-'
            },
            if entry.has_session { 'S' } else { '-' },
            if entry.is_remote { 'R' } else { '-' },
        );
        let worktree = entry
            .worktree_path
            .as_ref()
            .map_or_else(|| "-".to_string(), |path| path.display().to_string());
        let _ = writeln!(
            out,
            "{:<branch_width$}  {:<stat_width$}  {}",
            entry.name, stat, worktree
        );
    }
    out
}

fn format_session_table(rows: &[SessionOutput]) -> String {
    let session_header = "session";
    let repo_header = "repo";
    let branch_header = "branch";
    let path_header = "path";
    let attached_header = "attached";

    let session_width = rows
        .iter()
        .map(|row| row.session.len())
        .max()
        .unwrap_or(session_header.len())
        .max(session_header.len());
    let repo_width = rows
        .iter()
        .map(|row| row.repo.len())
        .max()
        .unwrap_or(repo_header.len())
        .max(repo_header.len());
    let branch_width = rows
        .iter()
        .map(|row| row.branch.as_deref().unwrap_or("(detached)").len())
        .max()
        .unwrap_or(branch_header.len())
        .max(branch_header.len());
    let path_width = rows
        .iter()
        .map(|row| row.path.display().to_string().len())
        .max()
        .unwrap_or(path_header.len())
        .max(path_header.len());

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{session_header:<session_width$}  {repo_header:<repo_width$}  {branch_header:<branch_width$}  {path_header:<path_width$}  {attached_header}"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "{:<session_width$}  {:<repo_width$}  {:<branch_width$}  {:<path_width$}  {}",
            row.session,
            row.repo,
            row.branch.as_deref().unwrap_or("(detached)"),
            row.path.display(),
            row.attached
        );
    }
    out
}

fn log_dir() -> CliResult<PathBuf> {
    let base = if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME")
        && !xdg_data_home.is_empty()
    {
        PathBuf::from(xdg_data_home)
    } else {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| CliError::system("HOME environment variable not set"))?;
        home.join(".local").join("share")
    };
    Ok(base.join("kiosk").join("logs"))
}

pub fn print_error(error: &CliError, json: bool) {
    if json {
        let payload = serde_json::json!({ "error": error.message() });
        eprintln!("{payload}");
    } else {
        eprintln!("{}", error.message());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::{
        config, git::mock::MockGitProvider, git::repo::Worktree, tmux::mock::MockTmuxProvider,
    };
    use std::sync::Mutex;

    fn test_config() -> Config {
        config::load_config_from_str("search_dirs = [\"/tmp\"]").unwrap()
    }

    fn repo(path: &str, name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            session_name: name.to_string(),
            path: PathBuf::from(path),
            worktrees: vec![Worktree {
                path: PathBuf::from(path),
                branch: Some("main".to_string()),
                is_main: true,
            }],
        }
    }

    #[test]
    fn resolve_repo_exact_matches_only_exact_name() {
        let repos = vec![repo("/tmp/a", "alpha"), repo("/tmp/b", "beta")];
        let found = resolve_repo_exact(&repos, "beta").unwrap();
        assert_eq!(found.name, "beta");
        assert!(resolve_repo_exact(&repos, "bet").is_err());
    }

    #[test]
    fn open_is_idempotent_when_worktree_and_session_exist() {
        let config = test_config();
        let mut git = MockGitProvider::default();
        let tmux = MockTmuxProvider {
            sessions: vec!["demo--feat-test".to_string()],
            inside_tmux: true,
            ..Default::default()
        };

        git.repos = vec![repo("/tmp/demo", "demo")];
        git.worktrees = vec![
            Worktree {
                path: PathBuf::from("/tmp/demo"),
                branch: Some("main".to_string()),
                is_main: true,
            },
            Worktree {
                path: PathBuf::from("/tmp/.kiosk_worktrees/demo--feat-test"),
                branch: Some("feat/test".to_string()),
                is_main: false,
            },
        ];
        git.branches = vec!["main".to_string(), "feat/test".to_string()];

        let output = open_internal(
            &config,
            &git,
            &tmux,
            &OpenArgs {
                repo: "demo".to_string(),
                branch: Some("feat/test".to_string()),
                new_branch: None,
                base: None,
                no_switch: true,
                run: None,
                log: false,
                json: false,
            },
        )
        .unwrap();

        assert!(!output.created);
        assert_eq!(output.session, "demo--feat-test");
        assert!(tmux.created_sessions.lock().unwrap().is_empty());
    }

    #[test]
    fn open_rejects_unknown_branch_with_new_branch_hint() {
        let config = test_config();
        let mut git = MockGitProvider::default();
        let tmux = MockTmuxProvider {
            inside_tmux: true,
            ..Default::default()
        };

        git.repos = vec![repo("/tmp/demo", "demo")];
        git.worktrees = vec![Worktree {
            path: PathBuf::from("/tmp/demo"),
            branch: Some("main".to_string()),
            is_main: true,
        }];
        git.branches = vec!["main".to_string()];

        let error = open_internal(
            &config,
            &git,
            &tmux,
            &OpenArgs {
                repo: "demo".to_string(),
                branch: Some("missing".to_string()),
                new_branch: None,
                base: None,
                no_switch: true,
                run: None,
                log: false,
                json: false,
            },
        )
        .unwrap_err();

        assert!(error.message().contains("Use --new-branch"));
        assert_eq!(error.code(), 1);
    }

    #[test]
    fn open_with_run_sends_keys_after_session_creation() {
        let config = test_config();
        let mut git = MockGitProvider::default();
        let tmux = MockTmuxProvider {
            inside_tmux: true,
            ..Default::default()
        };

        git.repos = vec![repo("/tmp/demo", "demo")];
        git.worktrees = vec![Worktree {
            path: PathBuf::from("/tmp/demo"),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        let output = open_internal(
            &config,
            &git,
            &tmux,
            &OpenArgs {
                repo: "demo".to_string(),
                branch: None,
                new_branch: None,
                base: None,
                no_switch: true,
                run: Some("echo MARKER".to_string()),
                log: false,
                json: false,
            },
        )
        .unwrap();

        assert!(output.created);
        assert_eq!(
            tmux.sent_keys.lock().unwrap().as_slice(),
            &[("demo".to_string(), "echo MARKER".to_string())]
        );
    }

    #[test]
    fn status_reports_attached_from_client_count() {
        let config = test_config();
        let mut git = MockGitProvider::default();
        let tmux = MockTmuxProvider {
            sessions: vec!["demo".to_string()],
            clients: vec!["/dev/pts/1".to_string()],
            capture_output: Mutex::new("line a\nline b".to_string()),
            ..Default::default()
        };

        git.repos = vec![repo("/tmp/demo", "demo")];
        git.worktrees = vec![Worktree {
            path: PathBuf::from("/tmp/demo"),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        let output = status_internal(
            &config,
            &git,
            &tmux,
            &StatusArgs {
                repo: "demo".to_string(),
                branch: None,
                json: false,
                lines: Some(10),
            },
        )
        .unwrap();

        assert!(output.attached);
        assert_eq!(output.clients, 1);
        assert!(output.output.contains("line a"));
    }

    #[test]
    fn tail_lines_returns_requested_suffix() {
        let content = "a\nb\nc\nd\ne\n";
        assert_eq!(tail_lines(content, 2), "d\ne");
        assert_eq!(tail_lines(content, 10), "a\nb\nc\nd\ne");
    }

    #[test]
    fn format_repo_table_snapshot() {
        let rows = vec![
            RepoOutput {
                name: "kiosk".to_string(),
                path: PathBuf::from("/tmp/kiosk"),
            },
            RepoOutput {
                name: "dotfiles".to_string(),
                path: PathBuf::from("/tmp/dotfiles"),
            },
        ];
        let rendered = format_repo_table(&rows);
        assert_eq!(
            rendered,
            "repo      path\n\
             kiosk     /tmp/kiosk\n\
             dotfiles  /tmp/dotfiles\n"
        );
    }

    #[test]
    fn format_branch_table_snapshot() {
        let rows = vec![
            BranchEntry {
                name: "main".to_string(),
                worktree_path: Some(PathBuf::from("/tmp/repo")),
                has_session: false,
                is_current: true,
                is_remote: false,
            },
            BranchEntry {
                name: "feat/test".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_remote: true,
            },
        ];
        let rendered = format_branch_table(&rows);
        assert_eq!(
            rendered,
            "branch     stat  worktree\n\
             main       *W--  /tmp/repo\n\
             feat/test  ---R  -\n"
        );
    }

    #[test]
    fn format_session_table_snapshot() {
        let rows = vec![
            SessionOutput {
                session: "repo--feat".to_string(),
                repo: "repo".to_string(),
                branch: Some("feat/test".to_string()),
                path: PathBuf::from("/tmp/repo-feat"),
                attached: false,
            },
            SessionOutput {
                session: "repo".to_string(),
                repo: "repo".to_string(),
                branch: None,
                path: PathBuf::from("/tmp/repo"),
                attached: true,
            },
        ];
        let rendered = format_session_table(&rows);
        assert_eq!(
            rendered,
            "session     repo  branch      path            attached\n\
             repo--feat  repo  feat/test   /tmp/repo-feat  false\n\
             repo        repo  (detached)  /tmp/repo       true\n"
        );
    }
}
