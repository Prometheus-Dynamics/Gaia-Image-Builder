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
        let materialized_dir = materialized_dir(spec, source);
        prepare_materialized_dir(&materialized_dir)?;
        let execution = execution_context(spec);

        let mut messages = Vec::new();
        let git_policy = SourceCommandPolicy {
            attempts: spec.policy.providers.git.retry_attempts.max(1),
            retry_backoff_ms: spec.policy.providers.git.retry_backoff_ms,
            retry_backoff_strategy: spec.policy.providers.git.retry_backoff_strategy,
            timeout_seconds: spec.policy.providers.git.timeout_seconds.max(1),
            output_retention: process_output_retention(spec),
        };
        if is_local_git_repo(&git.repo) {
            clone_or_update_local_git_source(
                git,
                &materialized_dir,
                &execution,
                git_policy,
                log_sink.clone(),
                cancel_check.clone(),
            )?;
            let resolved_head =
                git_head_commit(&materialized_dir).unwrap_or_else(|| "unknown".into());
            messages.push(format!(
                "git source '{}' cloned into '{}'",
                source.id.as_str(),
                materialized_dir.display()
            ));
            let (selected_ref_type, selected_ref_value) = git_selected_ref(git);
            write_source_marker(
                spec,
                self.id(),
                source,
                &materialized_dir,
                &format!(
                    "git repo={}\nselected_ref_type={}\nselected_ref_value={}\nresolved_mode=local\nresolved_commit_sha={}\nmaterialized_tree_digest={}\n",
                    git.repo,
                    selected_ref_type,
                    selected_ref_value,
                    resolved_head,
                    tree_digest(
                        &materialized_dir,
                        &[".git", "source.txt", ".gaia-source-state.txt"]
                    ),
                ),
            )?;
        } else {
            if !spec.policy.providers.git.allow_remote_resolution {
                let reason = format!(
                    "remote git resolution for '{}' skipped because policy.providers.git.allow_remote_resolution=false",
                    git.repo
                );
                let fallback = materialized_dir.join("remote-resolution-error.txt");
                fs::write(&fallback, &reason).map_err(|write_error| {
                    format!(
                        "failed to write remote git fallback '{}': {write_error}",
                        fallback.display()
                    )
                })?;
                messages.push(format!(
                    "git source '{}' skipped remote resolution and used metadata-only materialization",
                    source.id.as_str()
                ));
                write_source_marker(
                    spec,
                    self.id(),
                    source,
                    &materialized_dir,
                    &format!(
                        "git repo={}\nselected_ref_type={}\nselected_ref_value={}\nresolved_mode=remote-fallback\nresolution_error={}\n",
                        git.repo,
                        git_selected_ref(git).0,
                        git_selected_ref(git).1,
                        sanitize_state_value(&reason),
                    ),
                )?;
            } else {
                match resolve_remote_git_refs(
                    git,
                    &execution,
                    git_policy,
                    log_sink.clone(),
                    cancel_check.clone(),
                ) {
                    Ok(refs) => {
                        clone_or_update_local_git_source(
                            git,
                            &materialized_dir,
                            &execution,
                            git_policy,
                            log_sink.clone(),
                            cancel_check.clone(),
                        )?;
                        let resolved_head =
                            git_head_commit(&materialized_dir).unwrap_or_else(|| "unknown".into());
                        messages.push(format!(
                            "git source '{}' resolved remote refs from '{}' and cloned source tree",
                            source.id.as_str(),
                            git.repo
                        ));
                        let refs_path = materialized_dir.join("refs.txt");
                        fs::write(&refs_path, &refs).map_err(|error| {
                            SourceProviderError::runtime_state(format!(
                                "failed to write remote refs '{}': {error}",
                                refs_path.display()
                            ))
                        })?;
                        let (resolved_commit_sha, resolved_ref_name) =
                            parse_resolved_remote_ref(&refs).unwrap_or_else(|| {
                                ("unresolved".into(), git_selected_ref(git).1.to_string())
                            });
                        write_source_marker(
                            spec,
                            self.id(),
                            source,
                            &materialized_dir,
                            &format!(
                                "git repo={}\nselected_ref_type={}\nselected_ref_value={}\nresolved_mode=remote-clone\nresolved_commit_sha={}\nresolved_ref_name={}\nrefs_digest={}\nmaterialized_head_commit={}\nmaterialized_tree_digest={}\n",
                                git.repo,
                                git_selected_ref(git).0,
                                git_selected_ref(git).1,
                                resolved_commit_sha,
                                resolved_ref_name,
                                sha256_or_placeholder(&refs_path),
                                resolved_head,
                                tree_digest(
                                    &materialized_dir,
                                    &[".git", "source.txt", ".gaia-source-state.txt"]
                                ),
                            ),
                        )?;
                    }
                    Err(error) => {
                        let has_explicit_ref =
                            git.branch.is_some() || git.tag.is_some() || git.rev.is_some();
                        if has_explicit_ref {
                            clone_or_update_local_git_source(
                                git,
                                &materialized_dir,
                                &execution,
                                git_policy,
                                log_sink.clone(),
                                cancel_check.clone(),
                            )?;
                            let resolved_head = git_head_commit(&materialized_dir)
                                .unwrap_or_else(|| "unknown".into());
                            messages.push(format!(
                                "git source '{}' cloned directly after remote resolution failure",
                                source.id.as_str()
                            ));
                            write_source_marker(
                                spec,
                                self.id(),
                                source,
                                &materialized_dir,
                                &format!(
                                    "git repo={}\nselected_ref_type={}\nselected_ref_value={}\nresolved_mode=clone-fallback\nresolved_commit_sha={}\nresolution_error={}\nmaterialized_tree_digest={}\n",
                                    git.repo,
                                    git_selected_ref(git).0,
                                    git_selected_ref(git).1,
                                    resolved_head,
                                    sanitize_state_value(&error.message),
                                    tree_digest(
                                        &materialized_dir,
                                        &[".git", "source.txt", ".gaia-source-state.txt"]
                                    ),
                                ),
                            )?;
                        } else {
                            let fallback = materialized_dir.join("remote-resolution-error.txt");
                            fs::write(&fallback, &error.message).map_err(|write_error| {
                                format!(
                                    "failed to write remote git fallback '{}': {write_error}",
                                    fallback.display()
                                )
                            })?;
                            messages.push(format!(
                                "git source '{}' could not resolve remote refs and fell back to metadata-only materialization",
                                source.id.as_str()
                            ));
                            write_source_marker(
                                spec,
                                self.id(),
                                source,
                                &materialized_dir,
                                &format!(
                                    "git repo={}\nselected_ref_type={}\nselected_ref_value={}\nresolved_mode=remote-fallback\nresolution_error={}\n",
                                    git.repo,
                                    git_selected_ref(git).0,
                                    git_selected_ref(git).1,
                                    sanitize_state_value(&error.message),
                                ),
                            )?;
                        }
                    }
                }
            }
        }
        Ok(messages)
    }
}
