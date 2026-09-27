//! Confirmation-gated Git commit, push, and GitHub pull-request workflow.

use std::process::Output;
use std::time::Duration;
use std::{hash::Hash, hash::Hasher};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use super::api_errors::ApiError;
use super::git::{self, ACTION_TIMEOUT, STATUS_TIMEOUT};

const MAX_FIELD_BYTES: usize = 8 * 1024;
const MAX_COMMAND_STDOUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_COMMAND_STDERR_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShipPlan {
    branch: Option<String>,
    base_branch: String,
    suggested_branch: String,
    commit_message: String,
    pull_request_title: String,
    pull_request_body: String,
    has_changes: bool,
    changed_files: usize,
    ahead_count: u32,
    unpushed_count: u32,
    behind_count: u32,
    has_remote: bool,
    has_head: bool,
    pull_request: Option<PullRequest>,
    steps: Vec<ShipStep>,
    blockers: Vec<String>,
    review_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShipStep {
    id: &'static str,
    label: &'static str,
    required: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PullRequest {
    number: u64,
    url: String,
    state: String,
    title: String,
    base_ref_name: String,
    head_ref_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ShipRequest {
    confirmed: bool,
    branch: String,
    base_branch: String,
    commit_message: String,
    pull_request_title: String,
    pull_request_body: String,
    review_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShipResult {
    branch: String,
    commit: String,
    pull_request: PullRequest,
    settled: bool,
}

pub(super) async fn plan(cwd: &str, title: Option<&str>) -> Result<ShipPlan, ApiError> {
    plan_with_gh(cwd, title, "gh").await
}

async fn plan_with_gh(
    cwd: &str,
    title: Option<&str>,
    gh_program: &str,
) -> Result<ShipPlan, ApiError> {
    let root = git_stdout(cwd, &["rev-parse", "--show-toplevel"], STATUS_TIMEOUT).await?;
    let root = root.trim();
    let branch = optional_git_stdout(root, &["symbolic-ref", "--short", "HEAD"]).await;
    let has_head = git_success(root, &["rev-parse", "--verify", "HEAD"]).await?;
    let status = command_output(
        "git",
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        STATUS_TIMEOUT,
    )
    .await?;
    if !status.status.success() {
        return Err(command_failure("read Git status", &status));
    }
    let changed_files =
        status.stdout.split(|byte| *byte == 0).filter(|entry| !entry.is_empty()).count();
    let has_changes = changed_files > 0;
    let has_remote = git_success(root, &["remote", "get-url", "origin"]).await?;
    let base_branch = remote_default_branch(root, gh_program)
        .await
        .unwrap_or_else(|| default_branch_fallback(branch.as_deref()));
    let (behind_count, ahead_count) = base_divergence(root, &base_branch).await;
    let unpushed_count = upstream_divergence(root).await.1;
    let clean_title = clean_subject(title.unwrap_or("Ship workspace changes"));
    let suggested_branch = unique_branch(root, &format!("pecan/{}", slug(&clean_title))).await;
    let pull_request = if has_remote && branch.as_deref() != Some(base_branch.as_str()) {
        current_pull_request(gh_program, root, &base_branch, branch.as_deref().unwrap_or_default())
            .await
    } else {
        None
    };
    let needs_branch = branch.as_deref() == Some(base_branch.as_str());
    let mut blockers = Vec::new();
    if branch.is_none() {
        blockers.push("Detached HEAD: checkout a branch before shipping.".to_owned());
    }
    if !has_head {
        blockers.push("Create the repository's initial commit before shipping.".to_owned());
    }
    if !has_remote {
        blockers.push("Add an origin remote before shipping.".to_owned());
    }
    if has_remote && !command_succeeds(gh_program, root, &["auth", "status"]).await {
        blockers.push("Install and authenticate GitHub CLI before shipping.".to_owned());
    }
    if behind_count > 0 {
        blockers.push("The branch is behind its base. Pull or rebase before shipping.".to_owned());
    }
    if !has_changes && ahead_count == 0 && pull_request.is_none() {
        blockers.push("There are no workspace changes or local commits to ship.".to_owned());
    }
    let steps = vec![
        ShipStep { id: "branch", label: "Create feature branch", required: needs_branch },
        ShipStep { id: "commit", label: "Commit all workspace changes", required: has_changes },
        ShipStep {
            id: "push",
            label: "Push branch to origin",
            required: has_changes || unpushed_count > 0 || pull_request.is_none(),
        },
        ShipStep {
            id: "pr",
            label: "Create GitHub pull request",
            required: pull_request.is_none(),
        },
    ];
    let review_token =
        checkout_fingerprint(root, branch.as_deref(), &base_branch, has_head, &status.stdout)
            .await?;
    Ok(ShipPlan {
        branch,
        base_branch,
        suggested_branch,
        commit_message: clean_title.clone(),
        pull_request_title: clean_title,
        pull_request_body:
            "## Summary\n\nShipped from Pecan after reviewing all workspace changes.".to_owned(),
        has_changes,
        changed_files,
        ahead_count,
        unpushed_count,
        behind_count,
        has_remote,
        has_head,
        pull_request,
        steps,
        blockers,
        review_token,
    })
}

pub(super) async fn execute(
    cwd: &str,
    title: Option<&str>,
    request: &ShipRequest,
) -> Result<ShipResult, ApiError> {
    execute_with_gh(cwd, title, request, "gh").await
}

async fn execute_with_gh(
    cwd: &str,
    title: Option<&str>,
    request: &ShipRequest,
    gh_program: &str,
) -> Result<ShipResult, ApiError> {
    validate_request(request)?;
    let initial = plan_with_gh(cwd, title, gh_program).await?;
    if let Some(blocker) = initial.blockers.first() {
        return Err(ApiError::conflict(blocker));
    }
    if request.base_branch != initial.base_branch {
        return Err(ApiError::conflict("the base branch changed; review Ship again"));
    }
    if request.review_token != initial.review_token {
        return Err(ApiError::conflict("the checkout changed after review; review Ship again"));
    }
    let root = git_stdout(cwd, &["rev-parse", "--show-toplevel"], STATUS_TIMEOUT).await?;
    let root = root.trim();
    let mut branch = initial.branch.clone().ok_or_else(|| ApiError::conflict("detached HEAD"))?;
    if branch == initial.base_branch {
        ensure_valid_branch(root, &request.branch).await?;
        run_git(root, &["switch", "-c", &request.branch], ACTION_TIMEOUT, "create branch").await?;
        branch.clone_from(&request.branch);
    } else if request.branch != branch {
        return Err(ApiError::conflict("the checked-out branch changed; review Ship again"));
    }

    if initial.has_changes {
        run_git(root, &["add", "-A"], ACTION_TIMEOUT, "stage changes").await?;
        run_git(root, &["commit", "-m", &request.commit_message], ACTION_TIMEOUT, "commit changes")
            .await?;
    }
    ensure_clean(root).await?;
    let commit = git_stdout(root, &["rev-parse", "HEAD"], STATUS_TIMEOUT).await?;
    if initial.has_changes || initial.unpushed_count > 0 || initial.pull_request.is_none() {
        run_git(
            root,
            &["push", "--set-upstream", "origin", &branch],
            ACTION_TIMEOUT,
            "push branch",
        )
        .await?;
    }
    ensure_clean(root).await?;
    let pull_request = if let Some(existing) =
        current_pull_request(gh_program, root, &request.base_branch, &branch).await
    {
        existing
    } else {
        create_pull_request(gh_program, root, request, &branch).await?
    };
    Ok(ShipResult { branch, commit: commit.trim().to_owned(), pull_request, settled: true })
}

fn validate_request(request: &ShipRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request("Ship requires explicit confirmation".to_owned()));
    }
    for (name, value) in [
        ("branch", request.branch.as_str()),
        ("base branch", request.base_branch.as_str()),
        ("commit message", request.commit_message.as_str()),
        ("pull request title", request.pull_request_title.as_str()),
        ("review token", request.review_token.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ApiError::bad_request(format!("{name} cannot be empty")));
        }
        if value.len() > MAX_FIELD_BYTES {
            return Err(ApiError::bad_request(format!("{name} is too long")));
        }
    }
    if request.pull_request_body.len() > MAX_FIELD_BYTES {
        return Err(ApiError::bad_request("pull request body is too long".to_owned()));
    }
    Ok(())
}

async fn ensure_valid_branch(cwd: &str, branch: &str) -> Result<(), ApiError> {
    if !git_success(cwd, &["check-ref-format", "--branch", branch]).await? {
        return Err(ApiError::bad_request("invalid feature branch name".to_owned()));
    }
    if git_success(cwd, &["show-ref", "--verify", &format!("refs/heads/{branch}")]).await? {
        return Err(ApiError::conflict("the requested feature branch already exists"));
    }
    Ok(())
}

async fn create_pull_request(
    gh_program: &str,
    cwd: &str,
    request: &ShipRequest,
    branch: &str,
) -> Result<PullRequest, ApiError> {
    let output = command_output(
        gh_program,
        cwd,
        &[
            "pr",
            "create",
            "--base",
            &request.base_branch,
            "--head",
            branch,
            "--title",
            &request.pull_request_title,
            "--body",
            &request.pull_request_body,
        ],
        ACTION_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(command_failure("create pull request", &output));
    }
    current_pull_request(gh_program, cwd, &request.base_branch, branch).await.ok_or_else(|| {
        ApiError::bad_gateway("pull request was created but could not be read".to_owned())
    })
}

async fn current_pull_request(
    gh_program: &str,
    cwd: &str,
    expected_base: &str,
    expected_head: &str,
) -> Option<PullRequest> {
    let output = command_output(
        gh_program,
        cwd,
        &["pr", "view", "--json", "number,url,state,title,baseRefName,headRefName"],
        STATUS_TIMEOUT,
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let pull_request = serde_json::from_slice::<PullRequest>(&output.stdout).ok()?;
    if pull_request.state.eq_ignore_ascii_case("open")
        && pull_request.base_ref_name == expected_base
        && pull_request.head_ref_name == expected_head
    {
        Some(pull_request)
    } else {
        None
    }
}

async fn remote_default_branch(cwd: &str, gh_program: &str) -> Option<String> {
    let symbolic =
        optional_git_stdout(cwd, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .await
            .and_then(|value| value.strip_prefix("origin/").map(str::to_owned));
    if symbolic.is_some() {
        return symbolic;
    }
    let output = command_output(
        gh_program,
        cwd,
        &["repo", "view", "--json", "defaultBranchRef", "--jq", ".defaultBranchRef.name"],
        STATUS_TIMEOUT,
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!branch.is_empty()).then_some(branch)
}

fn default_branch_fallback(branch: Option<&str>) -> String {
    match branch {
        Some("master") => "master".to_owned(),
        _ => "main".to_owned(),
    }
}

async fn base_divergence(cwd: &str, base: &str) -> (u32, u32) {
    let comparison = format!("origin/{base}...HEAD");
    if !git_success(cwd, &["rev-parse", "--verify", &format!("origin/{base}")])
        .await
        .unwrap_or(false)
    {
        return upstream_divergence(cwd).await;
    }
    parse_divergence(cwd, &comparison).await
}

async fn upstream_divergence(cwd: &str) -> (u32, u32) {
    if !git_success(cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"]).await.unwrap_or(false) {
        return (0, 0);
    }
    parse_divergence(cwd, "@{upstream}...HEAD").await
}

async fn parse_divergence(cwd: &str, comparison: &str) -> (u32, u32) {
    optional_git_stdout(cwd, &["rev-list", "--left-right", "--count", comparison])
        .await
        .and_then(|value| {
            let mut counts = value.split_whitespace().filter_map(|part| part.parse::<u32>().ok());
            Some((counts.next()?, counts.next()?))
        })
        .unwrap_or((0, 0))
}

async fn unique_branch(cwd: &str, preferred: &str) -> String {
    if !git_success(cwd, &["show-ref", "--verify", &format!("refs/heads/{preferred}")])
        .await
        .unwrap_or(true)
    {
        return preferred.to_owned();
    }
    for suffix in 2..100 {
        let candidate = format!("{preferred}-{suffix}");
        if !git_success(cwd, &["show-ref", "--verify", &format!("refs/heads/{candidate}")])
            .await
            .unwrap_or(true)
        {
            return candidate;
        }
    }
    format!("{preferred}-new")
}

fn clean_subject(value: &str) -> String {
    let line = value.lines().next().unwrap_or("Ship workspace changes").trim();
    let mut subject = line.chars().take(72).collect::<String>();
    if subject.is_empty() {
        subject = "Ship workspace changes".to_owned();
    }
    subject
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(ch);
            separator = false;
        } else {
            separator = true;
        }
        if result.len() >= 48 {
            break;
        }
    }
    result.trim_end_matches('-').to_owned()
}

async fn git_success(cwd: &str, args: &[&str]) -> Result<bool, ApiError> {
    Ok(command_output("git", cwd, args, STATUS_TIMEOUT).await?.status.success())
}

async fn optional_git_stdout(cwd: &str, args: &[&str]) -> Option<String> {
    git_stdout(cwd, args, STATUS_TIMEOUT).await.ok().map(|value| value.trim().to_owned())
}

async fn git_stdout(cwd: &str, args: &[&str], timeout: Duration) -> Result<String, ApiError> {
    let output = command_output("git", cwd, args, timeout).await?;
    if !output.status.success() {
        return Err(command_failure("read Git state", &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn run_git(
    cwd: &str,
    args: &[&str],
    timeout: Duration,
    action: &str,
) -> Result<(), ApiError> {
    let output = command_output("git", cwd, args, timeout).await?;
    if !output.status.success() {
        return Err(command_failure(action, &output));
    }
    Ok(())
}

/// Runs `program` bounded and timed via [`git::run_bounded`], turning a
/// truncated stream into a hard failure: Ship's mutation path would rather
/// error out than act on a command whose output was cut off.
async fn command_output(
    program: &str,
    cwd: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, ApiError> {
    let bounded = git::run_bounded(
        program,
        cwd,
        args,
        timeout,
        MAX_COMMAND_STDOUT_BYTES,
        MAX_COMMAND_STDERR_BYTES,
    )
    .await?;
    if bounded.stdout_truncated {
        return Err(ApiError::bad_gateway(format!(
            "could not read {program} output: command output exceeded the safety limit"
        )));
    }
    if bounded.stderr_truncated {
        return Err(ApiError::bad_gateway(format!(
            "could not read {program} error: command output exceeded the safety limit"
        )));
    }
    Ok(Output { status: bounded.status, stdout: bounded.stdout, stderr: bounded.stderr })
}

async fn ensure_clean(cwd: &str) -> Result<(), ApiError> {
    let status =
        git_stdout(cwd, &["status", "--porcelain=v1", "--untracked-files=all"], STATUS_TIMEOUT)
            .await?;
    if !status.is_empty() {
        return Err(ApiError::conflict(
            "the workspace changed during Ship; review and commit the remaining changes",
        ));
    }
    Ok(())
}

async fn checkout_fingerprint(
    cwd: &str,
    branch: Option<&str>,
    base_branch: &str,
    has_head: bool,
    status: &[u8],
) -> Result<String, ApiError> {
    tokio::time::timeout(
        STATUS_TIMEOUT,
        checkout_fingerprint_inner(cwd, branch, base_branch, has_head, status),
    )
    .await
    .map_err(|_elapsed| ApiError::bad_gateway("checkout fingerprint timed out".to_owned()))?
}

async fn checkout_fingerprint_inner(
    cwd: &str,
    branch: Option<&str>,
    base_branch: &str,
    has_head: bool,
    status: &[u8],
) -> Result<String, ApiError> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cwd.hash(&mut hasher);
    branch.hash(&mut hasher);
    base_branch.hash(&mut hasher);
    status.hash(&mut hasher);
    if has_head {
        let head = git_stdout(cwd, &["rev-parse", "HEAD"], STATUS_TIMEOUT).await?;
        head.hash(&mut hasher);
        let diff = command_output(
            "git",
            cwd,
            &["diff", "--no-ext-diff", "--binary", "HEAD", "--"],
            STATUS_TIMEOUT,
        )
        .await?;
        if !diff.status.success() {
            return Err(command_failure("fingerprint tracked changes", &diff));
        }
        diff.stdout.hash(&mut hasher);
    }
    for entry in status.split(|byte| *byte == 0).filter(|entry| entry.starts_with(b"?? ")) {
        let path =
            entry.get(3..).and_then(|bytes| std::str::from_utf8(bytes).ok()).ok_or_else(|| {
                ApiError::bad_gateway("Ship cannot review a non-UTF-8 path".to_owned())
            })?;
        path.hash(&mut hasher);
        hash_file(&std::path::Path::new(cwd).join(path), &mut hasher).await?;
    }
    Ok(format!("{:016x}", hasher.finish()))
}

async fn hash_file(path: &std::path::Path, hasher: &mut impl Hasher) -> Result<(), ApiError> {
    let mut file = tokio::fs::File::open(path).await.map_err(|error| {
        ApiError::bad_gateway(format!("could not review {}: {error}", path.display()))
    })?;
    let mut chunk = vec![0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut chunk).await.map_err(|error| {
            ApiError::bad_gateway(format!("could not review {}: {error}", path.display()))
        })?;
        if read == 0 {
            return Ok(());
        }
        let bytes = chunk
            .get(..read)
            .ok_or_else(|| ApiError::internal("file read exceeded its buffer".to_owned()))?;
        hasher.write(bytes);
    }
}

async fn command_succeeds(program: &str, cwd: &str, args: &[&str]) -> bool {
    command_output(program, cwd, args, STATUS_TIMEOUT)
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_failure(action: &str, output: &Output) -> ApiError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.lines().next().unwrap_or("unknown command error");
    ApiError::bad_gateway(format!("could not {action}: {detail}"))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        ShipPlan, ShipRequest, clean_subject, execute_with_gh, plan, plan_with_gh, slug,
        validate_request,
    };

    #[test]
    fn deterministic_copy_does_not_require_model_generation() {
        assert_eq!(clean_subject("Improve mobile Ship UI\nignored"), "Improve mobile Ship UI");
        assert_eq!(slug("Improve mobile Ship UI"), "improve-mobile-ship-ui");
    }

    #[test]
    fn ship_requires_confirmation_and_bounded_copy() {
        let request = ShipRequest {
            confirmed: false,
            branch: "feature/ship".to_owned(),
            base_branch: "main".to_owned(),
            commit_message: "Ship it".to_owned(),
            pull_request_title: "Ship it".to_owned(),
            pull_request_body: String::new(),
            review_token: "review-token".to_owned(),
        };
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn main_branch_plan_requires_feature_branch_and_all_changes() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("pecan-ship-plan-{nonce}"));
        assert!(std::fs::create_dir_all(&root).is_ok());
        run(&root, &["init", "-b", "main"]);
        run(&root, &["config", "user.email", "pecan@example.invalid"]);
        run(&root, &["config", "user.name", "Pecan Test"]);
        assert!(std::fs::write(root.join("README.md"), "before\n").is_ok());
        run(&root, &["add", "README.md"]);
        run(&root, &["commit", "-m", "Initial commit"]);
        assert!(std::fs::write(root.join("README.md"), "after\n").is_ok());

        let path = root.to_string_lossy();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        assert!(runtime.is_ok());
        if let Ok(runtime) = runtime {
            let result = runtime.block_on(plan(&path, Some("Finish ship flow")));
            assert!(result.is_ok());
            if let Ok(result) = result {
                assert_eq!(result.branch.as_deref(), Some("main"));
                assert_eq!(result.suggested_branch, "pecan/finish-ship-flow");
                assert!(result.has_changes);
                assert_eq!(result.changed_files, 1);
                assert!(result.steps.first().is_some_and(|step| step.required));
                assert!(result.blockers.iter().any(|blocker| blocker.contains("origin")));
                assert!(std::fs::write(root.join("README.md"), "changed after review\n").is_ok());
                let refreshed = runtime.block_on(plan(&path, Some("Finish ship flow")));
                assert!(refreshed.is_ok());
                if let Ok(refreshed) = refreshed {
                    assert_ne!(result.review_token, refreshed.review_token);
                }
            }
        }
        let cleanup = std::fs::remove_dir_all(root);
        assert!(cleanup.is_ok());
    }

    #[test]
    fn execute_retries_after_pr_failure_without_losing_committed_work() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let temp = std::env::temp_dir();
        let root = temp.join(format!("pecan-ship-execute-{nonce}"));
        let remote = temp.join(format!("pecan-ship-remote-{nonce}.git"));
        let fake_gh = temp.join(format!("pecan-fake-gh-{nonce}"));
        let marker = temp.join(format!("pecan-fake-pr-{nonce}"));
        assert!(std::fs::create_dir_all(&root).is_ok());
        run(&root, &["init", "-b", "main"]);
        run(&root, &["config", "user.email", "pecan@example.invalid"]);
        run(&root, &["config", "user.name", "Pecan Test"]);
        assert!(std::fs::write(root.join("README.md"), "before\n").is_ok());
        run(&root, &["add", "README.md"]);
        run(&root, &["commit", "-m", "Initial commit"]);
        run(&temp, &["init", "--bare", &remote.to_string_lossy()]);
        run(&root, &["remote", "add", "origin", &remote.to_string_lossy()]);
        run(&root, &["push", "--set-upstream", "origin", "main"]);
        run(&root, &["remote", "set-head", "origin", "main"]);
        run(&root, &["switch", "-c", "feature/ship"]);
        assert!(std::fs::write(root.join("README.md"), "after\n").is_ok());
        write_fake_gh(&fake_gh, &marker, false);

        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        assert!(runtime.is_ok());
        if let Ok(runtime) = runtime {
            let root_text = root.to_string_lossy();
            let gh_text = fake_gh.to_string_lossy();
            let first = runtime.block_on(plan_with_gh(&root_text, Some("Ship safely"), &gh_text));
            assert!(first.is_ok());
            if let Ok(first) = first {
                let failed = runtime.block_on(execute_with_gh(
                    &root_text,
                    Some("Ship safely"),
                    &request(&first),
                    &gh_text,
                ));
                assert!(failed.is_err());
            }
            assert_eq!(git(&root, &["status", "--porcelain=v1"]), "");
            write_fake_gh(&fake_gh, &marker, true);
            let retry = runtime.block_on(plan_with_gh(&root_text, Some("Ship safely"), &gh_text));
            assert!(retry.is_ok());
            if let Ok(retry) = retry {
                let shipped = runtime.block_on(execute_with_gh(
                    &root_text,
                    Some("Ship safely"),
                    &request(&retry),
                    &gh_text,
                ));
                assert!(shipped.is_ok());
                if let Ok(shipped) = shipped {
                    assert_eq!(shipped.pull_request.base_ref_name, "main");
                    assert_eq!(shipped.pull_request.head_ref_name, "feature/ship");
                }
            }
            assert_eq!(git(&root, &["status", "--porcelain=v1"]), "");
            assert_eq!(
                git(&root, &["rev-parse", "HEAD"]),
                git(&root, &["rev-parse", "refs/remotes/origin/feature/ship"]),
            );
        }
        for path in [&root, &remote, &fake_gh, &marker] {
            let cleanup = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            assert!(cleanup.is_ok() || !path.exists());
        }
    }

    fn request(plan: &ShipPlan) -> ShipRequest {
        ShipRequest {
            confirmed: true,
            branch: plan.branch.clone().unwrap_or_else(|| plan.suggested_branch.clone()),
            base_branch: plan.base_branch.clone(),
            commit_message: plan.commit_message.clone(),
            pull_request_title: plan.pull_request_title.clone(),
            pull_request_body: plan.pull_request_body.clone(),
            review_token: plan.review_token.clone(),
        }
    }

    fn write_fake_gh(path: &std::path::Path, marker: &std::path::Path, succeed: bool) {
        let create = if succeed {
            format!("touch '{}'\nexit 0", marker.display())
        } else {
            "echo 'simulated PR failure' >&2\nexit 1".to_owned()
        };
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = auth ]; then exit 0; fi\nif [ \"$1\" = pr ] && [ \"$2\" = view ]; then\n  if [ -f '{}' ]; then echo '{{\"number\":7,\"url\":\"https://example.invalid/pr/7\",\"state\":\"OPEN\",\"title\":\"Ship safely\",\"baseRefName\":\"main\",\"headRefName\":\"feature/ship\"}}'; exit 0; fi\n  exit 1\nfi\nif [ \"$1\" = pr ] && [ \"$2\" = create ]; then\n{create}\nfi\nexit 1\n",
            marker.display(),
        );
        assert!(std::fs::write(path, script).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o700);
            assert!(std::fs::set_permissions(path, permissions).is_ok());
        }
    }

    fn git(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git").current_dir(cwd).args(args).output();
        assert!(output.as_ref().is_ok_and(|output| output.status.success()));
        output
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default()
    }

    fn run(cwd: &std::path::Path, args: &[&str]) {
        let status = Command::new("git").current_dir(cwd).args(args).status();
        assert!(status.is_ok_and(|status| status.success()), "git command failed: {args:?}");
    }
}
