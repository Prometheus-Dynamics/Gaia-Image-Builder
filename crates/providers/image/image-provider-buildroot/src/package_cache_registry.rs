//! The package cache's registry cache (a `backend = "oci"` checkpoint).
//!
//! A cache entry is one OCI artifact per package build, tagged by its input
//! key: the tar.zst of its files and its manifest (`<stem>` and
//! `<stem>.json`, see [`entry_stem`]), signed by the registry client. Before
//! a `make`, the packages it has to build whose key is not cached locally are
//! pulled and unpacked into the cache level, where the usual restore finds
//! them. After a `make`, the packages it stored are packed and pushed on
//! scoped threads, which all end before the function returns.
//!
//! A pull that fails is a warning and the package is built. A signature that
//! does not verify refuses the entry, with a message that says so.
use super::*;
use gaia_process::{PushOutcome, RegistryError, RemoteCache, pack_entry, unpack_entry};

/// The registry kind of package entries (the image is `<repository>/package`).
pub(crate) const PACKAGE_KIND: &str = "package";
/// Annotations of a package entry: its package name, the file stem of its
/// archive and manifest, and whether it is relocatable.
pub(crate) const PACKAGE_ANNOTATION: &str = "dev.gaia.package";
pub(crate) const STEM_ANNOTATION: &str = "dev.gaia.stem";
pub(crate) const RELOCATABLE_ANNOTATION: &str = "dev.gaia.relocatable";

impl PackageCache {
    /// Pulls every package that this tree still has to build, and whose key is
    /// set and not in a local level, from the registry cache. Returns the run
    /// messages. Nothing is pulled when no registry cache pulls.
    pub(crate) fn pull_missing(
        &self,
        output_dir: &Path,
        graph: &PackageGraph,
        keys: &BTreeMap<String, Option<String>>,
        excluded: &BTreeSet<String>,
    ) -> Vec<String> {
        let mut messages = self.registry_warnings.clone();
        let Some(registry) = self.registry.as_ref().filter(|registry| registry.pull) else {
            return messages;
        };
        let client = registry.client();
        let mut pulled = Vec::new();
        for name in graph.package_names() {
            if excluded.contains(name) || stamp_built(output_dir, graph, name) {
                continue;
            }
            let Some(Some(key)) = keys.get(name) else {
                continue;
            };
            if self.usable(output_dir, name, key).is_some() {
                continue;
            }
            match self.pull_one(&client, name, key) {
                Ok(true) => pulled.push(name.to_string()),
                Ok(false) => {}
                Err(message) => messages.push(message),
            }
        }
        if !pulled.is_empty() {
            messages.push(format!(
                "package cache: {} Buildroot package(s) pulled from the registry checkpoint '{}': {}",
                pulled.len(),
                registry.checkpoint_id,
                pulled.join(", ")
            ));
        }
        messages
    }

    /// Pulls one package entry into its level. `Ok(true)` when it was stored,
    /// `Ok(false)` when the registry has no such entry or it is already local.
    fn pull_one(&self, client: &RemoteCache, name: &str, key: &str) -> Result<bool, String> {
        let level = self.store_dir(name);
        let staging = level.join(name).join(format!(".pull-{key}"));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
        let result = self.pull_into(client, name, key, level, &staging);
        let _ = fs::remove_dir_all(&staging);
        result.map_err(|reason| format!("registry pull of package '{name}' ({key}): {reason}"))
    }

    fn pull_into(
        &self,
        client: &RemoteCache,
        name: &str,
        key: &str,
        level: &Path,
        staging: &Path,
    ) -> Result<bool, String> {
        let entry = match client.pull(PACKAGE_KIND, key, staging) {
            Ok(Some(entry)) => entry,
            Ok(None) => return Ok(false),
            Err(RegistryError::Signature(reason)) => {
                return Err(format!(
                    "SIGNATURE DOES NOT VERIFY, entry refused, the package is built instead: {reason}"
                ));
            }
            Err(error) => return Err(error.to_string()),
        };
        let annotation = |field: &str| entry.annotations.get(field).map(String::as_str);
        if annotation(PACKAGE_ANNOTATION) != Some(name) {
            return Err("the entry is for another package".into());
        }
        let stem = annotation(STEM_ANNOTATION)
            .ok_or("the entry has no stem annotation")?
            .to_string();
        unpack_entry(&entry.blob, staging, &stem).map_err(|error| error.to_string())?;
        let manifest_path = staging.join(format!("{stem}.json"));
        let text = fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
        let manifest = Manifest::parse(&text).ok_or("the manifest is unreadable")?;
        let manifest_name = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| value.get("package")?.as_str().map(str::to_string));
        if manifest_name.as_deref() != Some(name) {
            return Err("the manifest is for another package".into());
        }
        let expected = if manifest.relocatable {
            key.to_string()
        } else {
            entry_stem(key, Some(Path::new(&manifest.output_dir)))
        };
        if expected != stem {
            return Err("the entry's name does not match its manifest".into());
        }
        let archive = staging.join(&stem);
        if !archive.is_dir() {
            return Err("the entry has no archive directory".into());
        }
        let dir = level.join(name);
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let target = dir.join(&stem);
        if target.exists() {
            return Ok(false);
        }
        fs::rename(&archive, &target).map_err(|error| error.to_string())?;
        fs::rename(&manifest_path, dir.join(format!("{stem}.json")))
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Pushes the packages `stored` (name and key) that this run stored, on
    /// scoped threads joined before this returns. Returns the run messages.
    pub(crate) fn push_stored(&self, output_dir: &Path, stored: &[(String, String)]) -> Vec<String> {
        let Some(registry) = self.registry.as_ref().filter(|registry| registry.push) else {
            return Vec::new();
        };
        // The entries just stored: their file stems and whether they are
        // relocatable, as the store wrote them.
        let jobs: Vec<PushJob> = stored
            .iter()
            .filter_map(|(name, key)| {
                let (archive, manifest) = self.usable(output_dir, name, key)?;
                Some(PushJob {
                    name: name.clone(),
                    key: key.clone(),
                    stem: archive.file_name()?.to_string_lossy().into_owned(),
                    relocatable: manifest.relocatable,
                    dir: archive.parent()?.to_path_buf(),
                })
            })
            .collect();
        let outcomes: Vec<(String, Result<PushOutcome, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .iter()
                .map(|job| {
                    let client = registry.client();
                    scope.spawn(move || {
                        let outcome = push_one(&client, job);
                        (job.name.clone(), outcome)
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect()
        });
        let mut messages = Vec::new();
        let mut pushed = Vec::new();
        for (name, outcome) in outcomes {
            match outcome {
                Ok(PushOutcome::Pushed { .. }) => pushed.push(name),
                Ok(PushOutcome::AlreadyPresent | PushOutcome::Disabled) => {}
                Err(reason) => messages.push(format!(
                    "registry push of package '{name}' failed (the build is not affected): {reason}"
                )),
            }
        }
        if !pushed.is_empty() {
            messages.push(format!(
                "package cache: {} Buildroot package(s) pushed to the registry checkpoint '{}': {}",
                pushed.len(),
                registry.checkpoint_id,
                pushed.join(", ")
            ));
        }
        messages
    }

}

/// One stored package entry to push.
struct PushJob {
    name: String,
    key: String,
    /// The archive's file name; its manifest is this plus `.json`.
    stem: String,
    relocatable: bool,
    /// The cache level directory holding both.
    dir: PathBuf,
}

/// Packs the entry of `job` (its archive and manifest) and pushes it.
fn push_one(client: &RemoteCache, job: &PushJob) -> Result<PushOutcome, String> {
    let staging = job.dir.join(format!(".push-{}", job.key));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let blob = staging.join("entry.tar.zst");
    let manifest = format!("{}.json", job.stem);
    let result = pack_entry(&job.dir, &[job.stem.as_str(), manifest.as_str()], &blob)
        .map_err(|error| error.to_string())
        .and_then(|()| {
            let annotations = BTreeMap::from([
                (PACKAGE_ANNOTATION.to_string(), job.name.clone()),
                (STEM_ANNOTATION.to_string(), job.stem.clone()),
                (RELOCATABLE_ANNOTATION.to_string(), job.relocatable.to_string()),
            ]);
            client
                .push(PACKAGE_KIND, &job.key, &blob, &annotations)
                .map_err(|error| error.to_string())
        });
    let _ = fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_process::fake::FakeRegistry;
    use gaia_process::{RegistryTarget, SignatureKind};
    use gaia_testkit::TempDir;
    use std::os::unix::fs::PermissionsExt;

    /// A `minisign` stand-in (none is installed here): signing writes the
    /// file's sha256 to the `-x` path, and verifying compares it.
    fn stub_tools(dir: &Path) -> PathBuf {
        fs::create_dir_all(dir).expect("tools dir");
        let script = dir.join("minisign");
        let body = concat!(
            "#!/bin/sh\n",
            "mode=\"\"; file=\"\"; sig=\"\"\n",
            "while [ $# -gt 0 ]; do\n",
            "  case \"$1\" in\n",
            "    -S) mode=sign ;;\n",
            "    -V) mode=verify ;;\n",
            "    -m) file=\"$2\"; shift ;;\n",
            "    -x) sig=\"$2\"; shift ;;\n",
            "  esac\n",
            "  shift\n",
            "done\n",
            "sum=$(sha256sum \"$file\" | cut -d' ' -f1)\n",
            "if [ \"$mode\" = sign ]; then echo \"$sum\" > \"$sig\"; exit 0; fi\n",
            "[ \"$sum\" = \"$(cat \"$sig\")\" ]\n",
        );
        fs::write(&script, body).expect("stub");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("mode");
        dir.to_path_buf()
    }

    fn registry_use(registry: &FakeRegistry, tools: &Path, sign: bool) -> RegistryCacheUse {
        RegistryCacheUse {
            checkpoint_id: "registry".into(),
            target: RegistryTarget {
                registry: registry.addr().to_string(),
                repository: "owner/gaia-cache".into(),
                kind: SignatureKind::Minisign,
                sign_key: sign.then(|| "/keys/gaia.sec".to_string()),
                verify_key: Some("/keys/gaia.pub".to_string()),
                trust_unsigned: false,
                tool_dir: Some(tools.to_path_buf()),
            },
            pull: true,
            push: sign,
            warnings: Vec::new(),
        }
    }

    /// A relocatable package entry in `level`: an archive directory with one
    /// file and its manifest.
    fn write_entry(level: &Path, name: &str, key: &str, output_dir: &str) {
        let dir = level.join(name);
        fs::create_dir_all(dir.join(key).join("target/bin")).expect("archive");
        fs::write(dir.join(key).join("target/bin/tool"), b"tool bytes").expect("file");
        let manifest = serde_json::json!({
            "package": name,
            "version": "1.0",
            "output_dir": output_dir,
            "dirs": [["target", 493], ["target/bin", 493]],
            "files": ["target/bin/tool"],
            "size": 10,
            "stamps": [],
            "rewrite": [],
            "relocatable": true,
        });
        fs::write(dir.join(format!("{key}.json")), manifest.to_string()).expect("manifest");
    }

    #[test]
    fn a_pushed_package_is_pulled_into_an_empty_cache_and_found_there() {
        let scratch = TempDir::new("gaia-package-registry");
        let tools = stub_tools(&scratch.join("tools"));
        let registry = FakeRegistry::start(None);
        let output = scratch.join("out");
        let first = scratch.join("first");
        write_entry(&first, "foo", "k1", &output.display().to_string());
        let pusher =
            PackageCache::with_project_level(&first, Some(registry_use(&registry, &tools, true)));
        let messages = pusher.push_stored(&output, &[("foo".into(), "k1".into())]);
        assert!(
            messages
                .iter()
                .any(|line| line.contains("pushed to the registry checkpoint 'registry'")),
            "{messages:?}"
        );

        let second = scratch.join("second");
        let puller =
            PackageCache::with_project_level(&second, Some(registry_use(&registry, &tools, false)));
        assert!(puller.usable(&output, "foo", "k1").is_none());
        let client = puller.registry.as_ref().expect("registry").client();
        assert_eq!(puller.pull_one(&client, "foo", "k1"), Ok(true));
        let (archive, manifest) = puller
            .usable(&output, "foo", "k1")
            .expect("usable after pull");
        assert!(manifest.relocatable);
        assert_eq!(
            fs::read(archive.join("target/bin/tool")).expect("file"),
            b"tool bytes"
        );
        // A second pull finds the entry already local.
        assert_eq!(puller.pull_one(&client, "foo", "k1"), Ok(false));
    }

    #[test]
    fn a_manifest_whose_signature_does_not_verify_is_refused() {
        let scratch = TempDir::new("gaia-package-registry-forged");
        let tools = stub_tools(&scratch.join("tools"));
        let registry = FakeRegistry::start(None);
        let output = scratch.join("out");
        let first = scratch.join("first");
        write_entry(&first, "foo", "k1", &output.display().to_string());
        let pusher =
            PackageCache::with_project_level(&first, Some(registry_use(&registry, &tools, true)));
        pusher.push_stored(&output, &[("foo".into(), "k1".into())]);
        // The entry's manifest changes to bytes no key signed (still valid
        // JSON): the signature over the old bytes must not verify.
        {
            let mut state = registry.state();
            let tags = state
                .manifests
                .get_mut("owner/gaia-cache/package")
                .expect("image");
            let mut manifest = tags.get("k1").expect("entry tag").clone();
            manifest.push(b' ');
            tags.insert("k1".into(), manifest);
        }
        let second = scratch.join("second");
        let puller =
            PackageCache::with_project_level(&second, Some(registry_use(&registry, &tools, false)));
        let client = puller.registry.as_ref().expect("registry").client();
        let error = puller.pull_one(&client, "foo", "k1").expect_err("refused");
        assert!(error.contains("SIGNATURE"), "{error}");
        assert!(puller.usable(&output, "foo", "k1").is_none());
    }

    #[test]
    fn without_a_sign_key_nothing_is_pushed() {
        let scratch = TempDir::new("gaia-package-registry-nopush");
        let tools = stub_tools(&scratch.join("tools"));
        let registry = FakeRegistry::start(None);
        let output = scratch.join("out");
        let first = scratch.join("first");
        write_entry(&first, "foo", "k1", &output.display().to_string());
        let cache =
            PackageCache::with_project_level(&first, Some(registry_use(&registry, &tools, false)));
        assert!(
            cache
                .push_stored(&output, &[("foo".into(), "k1".into())])
                .is_empty()
        );
        assert!(
            registry.state().requests.is_empty(),
            "no request reached the registry"
        );
    }
}
