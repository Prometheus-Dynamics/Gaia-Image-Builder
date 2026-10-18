use super::*;

impl SourceProvider for GitSourceProvider {
    fn id(&self) -> &'static str {
        "source.git"
    }

    fn kind(&self) -> SourceProviderKind {
        SourceProviderKind::Git
    }

    fn execute_source(
        &self,
        spec: &ResolvedBuildSpec,
        source: &SourceSpec,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, SourceProviderError> {
        let SourceDefinition::Git(git) = &source.definition else {
            return Err(SourceProviderError::new(
                SourceProviderErrorKind::RuntimeState,
                format!("source '{}' was not a git source", source.id.as_str()),
            ));
        };
        let (execution, backend_note) = git_command_execution(spec, git, source.id.as_str());
        let mut result = self.materialize(
            spec,
            source,
            git,
            &execution,
            backend_note,
            log_sink,
            cancel_check,
        );
        if let Err(error) = &mut result
            && error.kind == SourceProviderErrorKind::BackendCommand
            && execution.docker.is_none()
            && !is_local_git_repo(&git.repo)
        {
            error.message = format!(
                "{} (git source '{}' from '{}', git ran on the host; GIT_TERMINAL_PROMPT=0 is set so missing credentials fail instead of prompting. For a private repository configure a git credential helper (for example `gh auth setup-git`) or an ssh key loaded in ssh-agent for this host. To run git through the execution backend instead, set `git_backend = \"execution\"` on the source.)",
                error.message,
                source.id.as_str(),
                redact_url_userinfo(&git.repo),
            );
        }
        // Credentials embedded in a URL never leave through a message.
        result
            .map(|messages| {
                messages
                    .into_iter()
                    .map(|message| redact_url_userinfo(&message))
                    .collect()
            })
            .map_err(|mut error| {
                error.message = redact_url_userinfo(&error.message);
                error
            })
    }
}

/// What a remote git source checks out: the commit, the refs listing that
/// names it (recorded in `refs.txt`), and the mirror the commit comes from.
pub(crate) struct RemoteCheckout {
    pub(crate) mirror: PathBuf,
    pub(crate) target: String,
    pub(crate) refs: String,
}

impl GitSourceProvider {
    #[allow(clippy::too_many_arguments)]
    fn materialize(
        &self,
        spec: &ResolvedBuildSpec,
        source: &SourceSpec,
        git: &GitSourceSpec,
        execution: &SourceExecutionContext,
        backend_note: Option<String>,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, SourceProviderError> {
        let materialized_dir = materialized_dir(spec, source);
        // Build state kept inside the checkout (such as .gaia/cargo-target)
        // survives the update; restored when this guard drops.
        let _preserved = PreservedGaiaDir::stash(&materialized_dir)?;
        // A source that checks out is updated in place (see
        // checkout_git_commit), so its checkout is only emptied when it is
        // not checked out at all.
        let checks_out =
            is_local_git_repo(&git.repo) || spec.policy.providers.git.allow_remote_resolution;
        if !checks_out {
            prepare_materialized_dir(&materialized_dir)?;
        }

        let mut messages: Vec<String> = backend_note.into_iter().collect();
        let policy = SourceCommandPolicy {
            attempts: spec.policy.providers.git.retry_attempts.max(1),
            retry_backoff_ms: spec.policy.providers.git.retry_backoff_ms,
            retry_backoff_strategy: spec.policy.providers.git.retry_backoff_strategy,
            timeout_seconds: spec.policy.providers.git.timeout_seconds.max(1),
            output_retention: process_output_retention(spec),
        };
        let run = GitRun {
            execution,
            policy,
            log_sink,
            cancel_check,
        };
        let (ref_kind, ref_value) = git_selected_ref(git);
        let source_id = source.id.as_str();

        if is_local_git_repo(&git.repo) {
            let repo = PathBuf::from(git.repo.strip_prefix("file://").unwrap_or(&git.repo));
            checkout_git_commit(
                git,
                &materialized_dir,
                &CheckoutSource::Local(repo),
                &local_git_target(git),
                &run,
            )?;
            let resolved_head =
                git_head_commit(&materialized_dir).unwrap_or_else(|| "unknown".into());
            messages.push(format!(
                "git source '{source_id}' cloned into '{}'",
                materialized_dir.display()
            ));
            write_source_marker(
                spec,
                self.id(),
                source,
                &materialized_dir,
                &format!(
                    "git repo={}\nselected_ref_type={ref_kind}\nselected_ref_value={ref_value}\nresolved_mode=local\nresolved_commit_sha={resolved_head}\nmaterialized_tree_digest={}\n{}",
                    git.repo,
                    checkout_content_digest(&materialized_dir),
                    lock_marker(git),
                ),
            )?;
        } else if let Some(locked_commit) = &git.locked_commit
            && spec.policy.providers.git.allow_remote_resolution
        {
            // The lockfile names the commit, so the mirror is only fetched
            // when it lacks that commit.
            let mirror =
                ensure_remote_git_cache(git, &run, MirrorRefresh::IfMissing(locked_commit))?;
            checkout_git_commit(
                git,
                &materialized_dir,
                &CheckoutSource::Mirror(mirror),
                locked_commit,
                &run,
            )?;
            let resolved_head =
                git_head_commit(&materialized_dir).unwrap_or_else(|| locked_commit.clone());
            messages.push(format!(
                "git source '{source_id}' cloned locked commit {locked_commit} from '{}'",
                git.repo
            ));
            write_source_marker(
                spec,
                self.id(),
                source,
                &materialized_dir,
                &format!(
                    "git repo={}\nselected_ref_type={ref_kind}\nselected_ref_value={ref_value}\nresolved_mode=locked\nresolved_commit_sha={locked_commit}\nmaterialized_head_commit={resolved_head}\nmaterialized_tree_digest={}\n{}",
                    git.repo,
                    checkout_content_digest(&materialized_dir),
                    lock_marker(git),
                ),
            )?;
        } else if !spec.policy.providers.git.allow_remote_resolution {
            let reason = format!(
                "remote git resolution for '{}' skipped because policy.providers.git.allow_remote_resolution=false",
                git.repo
            );
            let fallback = materialized_dir.join("remote-resolution-error.txt");
            fs::write(&fallback, redact_url_userinfo(&reason)).map_err(|write_error| {
                format!(
                    "failed to write remote git fallback '{}': {write_error}",
                    fallback.display()
                )
            })?;
            messages.push(format!(
                "git source '{source_id}' skipped remote resolution and used metadata-only materialization"
            ));
            write_source_marker(
                spec,
                self.id(),
                source,
                &materialized_dir,
                &format!(
                    "git repo={}\nselected_ref_type={ref_kind}\nselected_ref_value={ref_value}\nresolved_mode=remote-fallback\nresolution_error={}\n",
                    git.repo,
                    sanitize_state_value(&reason),
                ),
            )?;
        } else {
            match resolve_remote_checkout(git, &run) {
                Ok(remote) => {
                    checkout_git_commit(
                        git,
                        &materialized_dir,
                        &CheckoutSource::Mirror(remote.mirror.clone()),
                        &remote.target,
                        &run,
                    )?;
                    let resolved_head =
                        git_head_commit(&materialized_dir).unwrap_or_else(|| "unknown".into());
                    messages.push(format!(
                        "git source '{source_id}' resolved remote refs from '{}' and cloned source tree",
                        git.repo
                    ));
                    let refs_path = materialized_dir.join("refs.txt");
                    fs::write(&refs_path, &remote.refs).map_err(|error| {
                        SourceProviderError::runtime_state(format!(
                            "failed to write remote refs '{}': {error}",
                            refs_path.display()
                        ))
                    })?;
                    let (resolved_commit_sha, resolved_ref_name) =
                        parse_resolved_remote_ref(&remote.refs)
                            .unwrap_or_else(|| ("unresolved".into(), ref_value.to_string()));
                    write_source_marker(
                        spec,
                        self.id(),
                        source,
                        &materialized_dir,
                        &format!(
                            "git repo={}\nselected_ref_type={ref_kind}\nselected_ref_value={ref_value}\nresolved_mode=remote-clone\nresolved_commit_sha={resolved_commit_sha}\nresolved_ref_name={resolved_ref_name}\nrefs_digest={}\nmaterialized_head_commit={resolved_head}\nmaterialized_tree_digest={}\n",
                            git.repo,
                            sha256_or_placeholder(&refs_path),
                            checkout_content_digest(&materialized_dir),
                        ),
                    )?;
                }
                Err(error) => {
                    let has_explicit_ref =
                        git.branch.is_some() || git.tag.is_some() || git.rev.is_some();
                    if has_explicit_ref {
                        // The fetch must succeed: a stale mirror must not
                        // stand in for the ref that could not be resolved.
                        let mirror = ensure_remote_git_cache(git, &run, MirrorRefresh::Always)?;
                        checkout_git_commit(
                            git,
                            &materialized_dir,
                            &CheckoutSource::Mirror(mirror),
                            &local_git_target(git),
                            &run,
                        )?;
                        let resolved_head =
                            git_head_commit(&materialized_dir).unwrap_or_else(|| "unknown".into());
                        messages.push(format!(
                            "git source '{source_id}' cloned directly after remote resolution failure"
                        ));
                        write_source_marker(
                            spec,
                            self.id(),
                            source,
                            &materialized_dir,
                            &format!(
                                "git repo={}\nselected_ref_type={ref_kind}\nselected_ref_value={ref_value}\nresolved_mode=clone-fallback\nresolved_commit_sha={resolved_head}\nresolution_error={}\nmaterialized_tree_digest={}\n",
                                git.repo,
                                sanitize_state_value(&error.message),
                                checkout_content_digest(&materialized_dir),
                            ),
                        )?;
                    } else {
                        prepare_materialized_dir(&materialized_dir)?;
                        let fallback = materialized_dir.join("remote-resolution-error.txt");
                        fs::write(&fallback, redact_url_userinfo(&error.message)).map_err(
                            |write_error| {
                                format!(
                                    "failed to write remote git fallback '{}': {write_error}",
                                    fallback.display()
                                )
                            },
                        )?;
                        messages.push(format!(
                            "git source '{source_id}' could not resolve remote refs and fell back to metadata-only materialization"
                        ));
                        write_source_marker(
                            spec,
                            self.id(),
                            source,
                            &materialized_dir,
                            &format!(
                                "git repo={}\nselected_ref_type={ref_kind}\nselected_ref_value={ref_value}\nresolved_mode=remote-fallback\nresolution_error={}\n",
                                git.repo,
                                sanitize_state_value(&error.message),
                            ),
                        )?;
                    }
                }
            }
        }
        Ok(messages)
    }
}

/// Resolves what a remote git source with `allow_remote_resolution` checks
/// out, and fetches the mirror as far as that needs.
///
/// A branch or tag is read from the mirror after its fetch, so no separate
/// `ls-remote` round trip is made. A full commit id (`rev`, or the lock) needs
/// no fetch when the mirror already has it. `HEAD` is read from the remote,
/// because a mirror's own `HEAD` does not follow the remote's default branch.
pub(crate) fn resolve_remote_checkout(
    git: &GitSourceSpec,
    run: &GitRun,
) -> Result<RemoteCheckout, SourceProviderError> {
    let (kind, value) = git_selected_ref(git);
    match kind {
        "branch" | "tag" => {
            let mirror = ensure_remote_git_cache(git, run, MirrorRefresh::Always)?;
            let ref_name = if kind == "branch" {
                format!("refs/heads/{value}")
            } else {
                format!("refs/tags/{value}")
            };
            let mut command = git_command();
            command.arg("ls-remote").arg(&mirror).arg(&ref_name);
            let label = format!("git ls-remote for '{}'", redact_url_userinfo(&git.repo));
            let output = run.output(&mut command, &label)?;
            if !output.status.success() {
                return Err(SourceProviderError::backend_command(redact_url_userinfo(
                    &format!(
                        "git ls-remote failed for '{}': {}",
                        git.repo,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                )));
            }
            let refs = String::from_utf8_lossy(&output.stdout).to_string();
            let (target, _) = parse_resolved_remote_ref(&refs).ok_or_else(|| {
                SourceProviderError::backend_command(format!(
                    "{kind} '{value}' was not found in '{}'",
                    redact_url_userinfo(&git.repo)
                ))
            })?;
            Ok(RemoteCheckout {
                mirror,
                target,
                refs,
            })
        }
        "rev" => {
            let refresh = if is_full_commit_id(value) {
                MirrorRefresh::IfMissing(value)
            } else {
                MirrorRefresh::Always
            };
            let mirror = ensure_remote_git_cache(git, run, refresh)?;
            let commit = resolve_commit_in(&mirror, value).ok_or_else(|| {
                SourceProviderError::backend_command(format!(
                    "revision '{value}' was not found in '{}'",
                    redact_url_userinfo(&git.repo)
                ))
            })?;
            Ok(RemoteCheckout {
                mirror,
                refs: format!("{commit}\t{value}\n"),
                target: commit,
            })
        }
        _ => {
            let refs = resolve_remote_git_refs(
                git,
                run.execution,
                run.policy,
                run.log_sink.clone(),
                run.cancel_check.clone(),
            )?;
            let (target, _) = parse_resolved_remote_ref(&refs).ok_or_else(|| {
                SourceProviderError::backend_command(format!(
                    "HEAD was not found in '{}'",
                    redact_url_userinfo(&git.repo)
                ))
            })?;
            let mirror = ensure_remote_git_cache(git, run, MirrorRefresh::IfMissing(&target))?;
            Ok(RemoteCheckout {
                mirror,
                target,
                refs,
            })
        }
    }
}

/// Lockfile details recorded in the source state, empty for unlocked sources
/// so their state is unchanged.
fn lock_marker(git: &GitSourceSpec) -> String {
    git.locked_commit
        .as_deref()
        .map(|commit| format!("lock_mode=locked\nlocked_commit_sha={commit}\n"))
        .unwrap_or_default()
}
