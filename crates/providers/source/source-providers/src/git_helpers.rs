use super::*;

pub(crate) fn is_local_git_repo(repo: &str) -> bool {
    repo.starts_with("file://") || Path::new(repo).exists()
}

pub(crate) fn clone_or_update_local_git_source(
    git: &GitSourceSpec,
    output_dir: &Path,
    execution: &SourceExecutionContext,
    policy: SourceCommandPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), SourceProviderError> {
    let repo = git.repo.strip_prefix("file://").unwrap_or(&git.repo);
    let mut clone = git_command();
    clone.arg("clone");
    if git.rev.is_none() {
        clone.arg("--depth").arg("1");
        if let Some(branch) = &git.branch {
            clone.arg("--branch").arg(branch);
        } else if let Some(tag) = &git.tag {
            clone.arg("--branch").arg(tag);
        }
    }
    clone.arg(repo).arg(output_dir);
    run_command_with_policy(
        clone,
        execution,
        "clone local git source",
        policy,
        log_sink.clone(),
        cancel_check.clone(),
    )?;

    if let Some(branch) = &git.branch {
        let mut checkout = git_command();
        checkout
            .arg("-C")
            .arg(output_dir)
            .arg("checkout")
            .arg(branch);
        run_command_with_policy(
            checkout,
            execution,
            "checkout git branch",
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
    }
    if let Some(tag) = &git.tag {
        let mut checkout = git_command();
        checkout.arg("-C").arg(output_dir).arg("checkout").arg(tag);
        run_command_with_policy(
            checkout,
            execution,
            "checkout git tag",
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
    }
    if let Some(rev) = &git.rev {
        let mut checkout = git_command();
        checkout.arg("-C").arg(output_dir).arg("checkout").arg(rev);
        run_command_with_policy(
            checkout,
            execution,
            "checkout git revision",
            policy,
            log_sink,
            cancel_check,
        )?;
    }
    if let Some(subdir) = &git.subdir {
        fs::write(output_dir.join("selected-subdir.txt"), subdir).map_err(|error| {
            SourceProviderError::runtime_state(format!(
                "failed to write selected git subdir marker '{}': {error}",
                output_dir.join("selected-subdir.txt").display()
            ))
        })?;
    }
    Ok(())
}

pub(crate) fn resolve_remote_git_refs(
    git: &GitSourceSpec,
    execution: &SourceExecutionContext,
    policy: SourceCommandPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<String, SourceProviderError> {
    let selector = git_selected_ref(git).1;
    let attempts = policy.attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=attempts {
        let mut command = git_command();
        command.arg("ls-remote").arg(&git.repo).arg(selector);
        let output = command_output_with_timeout(
            &mut command,
            execution,
            Duration::from_secs(policy.timeout_seconds.max(1)),
            &format!("git ls-remote for '{}'", git.repo),
            policy.output_retention,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        last_error = Some(format!(
            "git ls-remote failed for '{}' on attempt {}/{}: {}",
            git.repo,
            attempt,
            attempts,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        if attempt < attempts {
            let retry_backoff = retry_backoff_duration(
                policy.retry_backoff_strategy,
                policy.retry_backoff_ms,
                attempt,
            );
            if !sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                return Err(SourceProviderError::new(
                    SourceProviderErrorKind::Cancelled,
                    format!(
                        "git ls-remote for '{}' cancelled during retry backoff",
                        git.repo
                    ),
                ));
            }
        }
    }
    Err(SourceProviderError::backend_command(
        last_error.unwrap_or_else(|| {
            format!(
                "git ls-remote failed for '{}' after {} attempt(s)",
                git.repo, attempts
            )
        }),
    ))
}

pub(crate) fn git_selected_ref(git: &GitSourceSpec) -> (&'static str, &str) {
    if let Some(branch) = git.branch.as_deref() {
        ("branch", branch)
    } else if let Some(tag) = git.tag.as_deref() {
        ("tag", tag)
    } else if let Some(rev) = git.rev.as_deref() {
        ("rev", rev)
    } else {
        ("head", "HEAD")
    }
}

pub(crate) fn git_head_commit(repo_dir: &Path) -> Option<String> {
    let output = git_command()
        .arg("-C")
        .arg(repo_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn git_command() -> Command {
    let mut command = Command::new("git");
    command.arg("-c").arg("safe.directory=*");
    command
}

pub(crate) fn parse_resolved_remote_ref(contents: &str) -> Option<(String, String)> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?.to_string();
        let name = parts.next().unwrap_or("HEAD").to_string();
        Some((sha, name))
    })
}

pub(crate) fn sanitize_state_value(value: &str) -> String {
    value.replace('\n', "\\n")
}
