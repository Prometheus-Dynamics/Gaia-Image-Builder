use super::*;

impl SourceProvider for PathSourceProvider {
    fn id(&self) -> &'static str {
        "source.path"
    }

    fn kind(&self) -> SourceProviderKind {
        SourceProviderKind::Path
    }

    fn execute_source(
        &self,
        spec: &ResolvedBuildSpec,
        source: &SourceSpec,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, SourceProviderError> {
        let _ = log_sink;
        let _ = cancel_check;
        let SourceDefinition::Path(path) = &source.definition else {
            return Err(SourceProviderError::new(
                SourceProviderErrorKind::RuntimeState,
                format!("source '{}' was not a path source", source.id.as_str()),
            ));
        };
        let materialized_dir = materialized_dir(spec, source);
        prepare_materialized_dir(&materialized_dir)?;

        let resolved = resolve_workspace_path(&spec.workspace, &path.path)?;
        let link_path = materialized_dir.join("content");
        create_symlink_or_manifest(&resolved, &link_path)?;
        write_source_marker(
            spec,
            self.id(),
            source,
            &materialized_dir,
            &format!(
                "path={}\npath_digest={}\ncontent_identity_mode={}\nlink_mode={}\n",
                resolved.display(),
                path_source_digest(&resolved, &path.identity_ignore),
                "live-reference",
                if link_path.exists() {
                    "symlink"
                } else {
                    "manifest"
                }
            ),
        )?;
        Ok(vec![format!(
            "path source '{}' linked '{}'",
            source.id.as_str(),
            resolved.display()
        )])
    }
}
