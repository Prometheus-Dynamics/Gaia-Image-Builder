//! A cache of built Buildroot packages shared by every build of the user,
//! like Yocto's sstate for Buildroot packages.
//!
//! After a successful `make`, each package built in this run is stored as
//! a directory under its key ([`package_keys`]): the files it added to its
//! per-package trees (`BR2_PER_PACKAGE_DIRECTORIES`), its image files, file
//! lists and kconfig `.config`, with a manifest of them and of its stamps.
//! Files are cloned (`cp --reflink=auto`): on the build's own filesystem
//! with reflinks (btrfs, XFS) storing and restoring copy no data. Before the
//! next `make` of any build, packages that are not built yet and whose key
//! is cached are restored instead: their files are cloned back, the output
//! directory path is rewritten in the text files that held the old one, and
//! their stamps are recreated in order, so `make` treats them as built.
//!
//! A package is restored only once all its dependencies are built or
//! restored: like Buildroot's per-package preparation, its per-package
//! directories start as a copy of its dependencies' (hard links), then its
//! own files are added. Packages whose installed binaries embed the output
//! directory path (most host tools) can only be restored into a tree at the
//! same path, such as the same build after a wipe. `linux` is restored only
//! when nothing still to be built needs its build directory (out-of-tree
//! kernel modules build against it).
use super::*;
use sha2::Digest;
use std::os::unix::fs::MetadataExt;

const DEFAULT_PACKAGE_CACHE_DIR: &str = gaia_spec::USER_BUILDROOT_PACKAGE_CACHE_DIR;
const DEFAULT_PACKAGE_CACHE_MAX_SIZE: &str = "100G";

/// Stamps in the order Buildroot creates them; restored stamps get
/// increasing modification times in this order.
pub(crate) const STAMP_ORDER: &[&str] = &[
    ".stamp_downloaded",
    ".stamp_rsynced",
    ".stamp_extracted",
    ".stamp_patched",
    ".stamp_configured",
    ".stamp_built",
    ".stamp_host_installed",
    ".stamp_staging_installed",
    ".stamp_target_installed",
    ".stamp_images_installed",
    ".stamp_installed",
];

/// Buildroot's per-package installed-file lists, kept with the package.
const FILE_LISTS: &[&str] = &[
    ".files-list.txt",
    ".files-list-staging.txt",
    ".files-list-host.txt",
    ".files-list-images.txt",
];

pub(crate) struct PackageCache {
    /// The system level (shared by every project), or `None` when its
    /// default location cannot clone from this build (then system-level
    /// packages go to the project level).
    system: Option<PathBuf>,
    /// The project level.
    project: PathBuf,
    /// Size kept at each level.
    max_size: u64,
    policy: gaia_spec::BuildrootPackageCachePolicySpec,
    /// Where the cache is, when that needs saying.
    pub(crate) note: Option<String>,
}

/// The configured package cache, or `None` when it is off. It needs
/// per-package directories (`parallel_packages`), which hold each package's
/// files apart from its dependencies'.
pub(crate) fn package_cache(
    spec: &ResolvedBuildSpec,
    policy: &ImageExecutionPolicy,
    output_dir: &Path,
) -> Result<Option<PackageCache>, ImageProviderError> {
    let settings = &policy.package_cache;
    if !settings.enabled || !policy.parallel_packages {
        return Ok(None);
    }
    let label = "buildroot package cache";
    let project = match settings.project_dir.as_deref() {
        Some(dir) => ensure_cache_dir(spec, dir, label)?,
        None => ensure_cache_dir(
            spec,
            &format!(".gaia/cache/{DEFAULT_PACKAGE_CACHE_DIR}"),
            label,
        )?,
    };
    let mut note = None;
    let system = match settings.system_dir.as_deref() {
        Some(dir) => Some(ensure_cache_dir(spec, dir, label)?),
        None => {
            let shared = default_cache_dir(spec, DEFAULT_PACKAGE_CACHE_DIR, label)?;
            // Storing and restoring clone files; across filesystems that
            // would copy everything, so the default stays on the build's
            // filesystem.
            if shared == project
                || reflinks_between(output_dir, &shared)
                || !supports_reflinks(output_dir)
            {
                Some(shared)
            } else {
                note = Some(format!(
                    "package cache: the system level '{}' is on another filesystem than the \
                     build, so its packages are kept at the project level '{}'; set \
                     [providers.buildroot.package_cache] system_dir to a directory on the \
                     build's filesystem to share packages between projects",
                    shared.display(),
                    project.display()
                ));
                None
            }
        }
    };
    let max_size = settings
        .max_size
        .as_deref()
        .unwrap_or(DEFAULT_PACKAGE_CACHE_MAX_SIZE)
        .parse::<gaia_spec::ByteSize>()
        .map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                format!("providers.buildroot.package_cache.max_size: {error}"),
            )
        })?
        .bytes();
    Ok(Some(PackageCache {
        system,
        project,
        max_size,
        policy: settings.clone(),
        note,
    }))
}

impl PackageCache {
    /// A warning when the cache's filesystem cannot hold `max_size` more.
    pub(crate) fn space_warning(&self) -> Vec<String> {
        self.levels()
            .into_iter()
            .filter_map(|(level, dir)| {
                cache_space_warning(
                    &format!("package cache ({} level)", level.as_str()),
                    dir,
                    self.max_size,
                )
            })
            .collect()
    }

    /// The levels in lookup order (the project's own first).
    fn levels(&self) -> Vec<(gaia_spec::PackageCacheLevelSpec, &Path)> {
        let mut levels = vec![(
            gaia_spec::PackageCacheLevelSpec::Project,
            self.project.as_path(),
        )];
        if let Some(system) = self
            .system
            .as_deref()
            .filter(|system| *system != self.project)
        {
            levels.push((gaia_spec::PackageCacheLevelSpec::System, system));
        }
        levels
    }

    /// Where a package is stored.
    fn store_dir(&self, name: &str) -> &Path {
        match self.policy.level_of(name) {
            gaia_spec::PackageCacheLevelSpec::System => {
                self.system.as_deref().unwrap_or(&self.project)
            }
            gaia_spec::PackageCacheLevelSpec::Project => &self.project,
        }
    }
}

/// What packages are built with, as part of every key: the Docker image
/// (by id when Docker can tell) or the host compiler.
pub(crate) fn execution_identity(execution: &ImageExecutionContext) -> String {
    let output = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    match &execution.docker_image {
        Some(image) => {
            let id = output(
                "docker",
                &["image", "inspect", "--format", "{{.Id}}", image],
            );
            format!("docker:{}", id.unwrap_or_else(|| image.clone()))
        }
        None => format!(
            "host:{}:{}",
            output("gcc", &["-dumpfullversion"]).unwrap_or_default(),
            output("gcc", &["-dumpmachine"]).unwrap_or_default()
        ),
    }
}

struct Manifest {
    output_dir: String,
    /// Directories (with their modes) and files of the entry, relative to
    /// the output directory.
    dirs: Vec<(String, u32)>,
    files: Vec<String>,
    /// Bytes of its files.
    size: u64,
    stamps: Vec<String>,
    /// Archive paths of text files that hold `output_dir`.
    rewrite: Vec<String>,
    /// False when a binary holds `output_dir`: the package can only be
    /// restored into a tree at the same path.
    relocatable: bool,
}

impl Manifest {
    fn to_json(&self, name: &str, version: Option<&str>) -> String {
        serde_json::json!({
            "package": name,
            "version": version,
            "output_dir": self.output_dir,
            "dirs": self.dirs,
            "files": self.files,
            "size": self.size,
            "stamps": self.stamps,
            "rewrite": self.rewrite,
            "relocatable": self.relocatable,
        })
        .to_string()
    }

    fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let strings = |field: &str| -> Option<Vec<String>> {
            value
                .get(field)?
                .as_array()?
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect()
        };
        let dirs = value
            .get("dirs")?
            .as_array()?
            .iter()
            .map(|pair| {
                let pair = pair.as_array()?;
                Some((
                    pair.first()?.as_str()?.to_string(),
                    u32::try_from(pair.get(1)?.as_u64()?).ok()?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            output_dir: value.get("output_dir")?.as_str()?.to_string(),
            dirs,
            files: strings("files")?,
            size: value.get("size")?.as_u64()?,
            stamps: strings("stamps")?,
            rewrite: strings("rewrite")?,
            relocatable: value.get("relocatable")?.as_bool()?,
        })
    }
}

impl PackageCache {
    /// The directory and manifest of a package build: relocatable, or
    /// `pinned` to the output directory it was built in.
    fn entry(&self, name: &str, key: &str, pinned: Option<&Path>) -> (PathBuf, PathBuf) {
        entry_in(self.store_dir(name), name, key, pinned)
    }

    /// A cached build of the package that can be restored into
    /// `output_dir`, from the project level first, then the system level:
    /// a relocatable one, or one built at that path.
    fn usable(&self, output_dir: &Path, name: &str, key: &str) -> Option<(PathBuf, Manifest)> {
        self.levels().into_iter().find_map(|(_, level)| {
            [None, Some(output_dir)].into_iter().find_map(|pinned| {
                let (archive, manifest) = entry_in(level, name, key, pinned);
                let manifest = Manifest::parse(&fs::read_to_string(manifest).ok()?)?;
                (archive.is_dir()
                    && (manifest.relocatable || Path::new(&manifest.output_dir) == output_dir))
                    .then_some((archive, manifest))
            })
        })
    }
}

/// The directory and manifest of a package build in a cache level:
/// relocatable, or `pinned` to the output directory it was built in.
pub(crate) fn entry_in(
    level: &Path,
    name: &str,
    key: &str,
    pinned: Option<&Path>,
) -> (PathBuf, PathBuf) {
    let dir = level.join(name);
    let stem = match pinned {
        None => key.to_string(),
        Some(output_dir) => {
            let digest = sha2::Sha256::digest(output_dir.display().to_string().as_bytes());
            format!("{key}@{}", &hex(&digest)[..16])
        }
    };
    (dir.join(&stem), dir.join(format!("{stem}.json")))
}

impl PackageCache {
    /// Restores, dependencies first, every package that is not built in
    /// `output_dir`, whose key is cached and whose dependencies are all
    /// built or restored. Returns the restored package names.
    pub(crate) fn restore(
        &self,
        output_dir: &Path,
        graph: &PackageGraph,
        keys: &BTreeMap<String, Option<String>>,
    ) -> Vec<String> {
        let built = |name: &str| {
            graph
                .get(name)
                .and_then(|package| package.stamp_dir.as_deref())
                .is_some_and(|stamp_dir| {
                    output_dir
                        .join(stamp_dir)
                        .join(".stamp_installed")
                        .is_file()
                })
        };
        let order = dependency_order(graph);
        let plan = |excluded: &BTreeSet<String>| {
            let mut ready = order
                .iter()
                .filter(|name| built(name))
                .cloned()
                .collect::<BTreeSet<_>>();
            let mut plan = Vec::new();
            for name in &order {
                if ready.contains(name) || excluded.contains(name) {
                    continue;
                }
                let Some(package) = graph.get(name) else {
                    continue;
                };
                let dependencies_ready = package
                    .dependencies
                    .iter()
                    .all(|dependency| ready.contains(dependency));
                let cached = keys
                    .get(name)
                    .and_then(Option::as_deref)
                    .and_then(|key| self.usable(output_dir, name, key))
                    .is_some();
                if dependencies_ready && cached {
                    ready.insert(name.clone());
                    plan.push(name.clone());
                }
            }
            plan
        };
        let mut plan_list = plan(&BTreeSet::new());
        // Out-of-tree kernel modules build against the kernel build tree,
        // which an archive does not hold.
        if plan_list.iter().any(|name| name == "linux") {
            let dependents_ready = graph.get("linux").is_some_and(|linux| {
                linux.reverse_dependencies.iter().all(|dependent| {
                    plan_list.iter().any(|name| name == dependent) || built(dependent)
                })
            });
            if !dependents_ready {
                plan_list = plan(&BTreeSet::from(["linux".to_string()]));
            }
        }
        let mut restored = BTreeSet::new();
        let mut listings = TreeListings::default();
        for name in plan_list {
            let Some(package) = graph.get(&name) else {
                continue;
            };
            // A dependency that failed to restore will be built instead.
            if !package
                .dependencies
                .iter()
                .all(|dependency| restored.contains(dependency) || built(dependency))
            {
                continue;
            }
            let Some(Some(key)) = keys.get(&name) else {
                continue;
            };
            match self.restore_one(output_dir, graph, &name, key, &mut listings) {
                Ok(()) => {
                    restored.insert(name);
                }
                Err(error) => {
                    tracing::warn!(package = %name, %error, "package cache restore failed");
                    self.discard_partial(output_dir, graph, &name);
                }
            }
        }
        restored.into_iter().collect()
    }

    fn restore_one(
        &self,
        output_dir: &Path,
        graph: &PackageGraph,
        name: &str,
        key: &str,
        listings: &mut TreeListings,
    ) -> Result<(), String> {
        let (archive, manifest) = self
            .usable(output_dir, name, key)
            .ok_or("no usable cache entry")?;
        let stamp_dir = graph
            .get(name)
            .and_then(|package| package.stamp_dir.clone())
            .ok_or("no build directory")?;
        self.discard_partial(output_dir, graph, name);
        // What Buildroot's prepare-per-package-directory does before a
        // package's configure step: its dependencies' trees first.
        let per_package = output_dir.join("per-package");
        let dependencies = recursive_dependencies(graph, name);
        for tree in ["host", "target"] {
            let sources = dependencies
                .iter()
                .map(|dependency| per_package.join(dependency).join(tree))
                .filter(|source| source.is_dir())
                .collect::<Vec<_>>();
            if sources.is_empty() {
                continue;
            }
            listings.link(&per_package.join(name).join(tree), &sources)?;
        }
        clone_members(&archive, output_dir, &manifest.dirs, &manifest.files)?;
        let new_output = output_dir.display().to_string();
        if manifest.output_dir != new_output {
            for relative in &manifest.rewrite {
                let path = output_dir.join(relative);
                let contents = fs::read(&path).map_err(|error| format!("{relative}: {error}"))?;
                let rewritten = replace_bytes(
                    &contents,
                    manifest.output_dir.as_bytes(),
                    new_output.as_bytes(),
                );
                fs::write(&path, rewritten).map_err(|error| format!("{relative}: {error}"))?;
            }
        }
        let stamp_dir = output_dir.join(stamp_dir);
        fs::create_dir_all(&stamp_dir).map_err(|error| error.to_string())?;
        // In the order the build created them, and newer than the sources
        // materialized for this run, which some stamps depend on (for
        // example a kconfig package's configuration file).
        let start = std::time::SystemTime::now();
        for (index, stamp) in manifest.stamps.iter().enumerate() {
            let path = stamp_dir.join(stamp);
            let file = fs::File::create(&path).map_err(|error| error.to_string())?;
            file.set_modified(start + Duration::from_millis(10 * index as u64))
                .map_err(|error| error.to_string())?;
        }
        record_package_key(&stamp_dir, key);
        self.mark_used(name, key, output_dir);
        Ok(())
    }

    /// Recently used entries are evicted last.
    fn mark_used(&self, name: &str, key: &str, output_dir: &Path) {
        for (pinned, (_, level)) in [None, Some(output_dir)]
            .into_iter()
            .flat_map(|pinned| self.levels().into_iter().map(move |level| (pinned, level)))
        {
            let (_, manifest) = entry_in(level, name, key, pinned);
            if let Ok(file) = fs::File::options().append(true).open(&manifest) {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
        }
    }

    fn discard_partial(&self, output_dir: &Path, graph: &PackageGraph, name: &str) {
        if let Some(stamp_dir) = graph
            .get(name)
            .and_then(|package| package.stamp_dir.as_deref())
        {
            let _ = fs::remove_dir_all(output_dir.join(stamp_dir));
        }
        let _ = fs::remove_dir_all(output_dir.join("per-package").join(name));
    }

    /// Archives every package built in `output_dir` whose key is not cached
    /// yet (skipping `restored`). Returns the stored package names and the
    /// packages that could not be stored, with the reason.
    pub(crate) fn store(
        &self,
        output_dir: &Path,
        graph: &PackageGraph,
        keys: &BTreeMap<String, Option<String>>,
        restored: &[String],
    ) -> (Vec<String>, Vec<String>) {
        let mut stored = Vec::new();
        let mut skipped = Vec::new();
        for name in graph.package_names() {
            let Some(package) = graph.get(name) else {
                continue;
            };
            let (Some(Some(key)), Some(stamp_dir)) = (keys.get(name), package.stamp_dir.as_deref())
            else {
                continue;
            };
            if restored.iter().any(|restored| restored == name)
                || !output_dir
                    .join(stamp_dir)
                    .join(".stamp_installed")
                    .is_file()
            {
                continue;
            }
            // The key this build of the package was made with, so later runs
            // can tell its stamps still describe its inputs.
            record_package_key(&output_dir.join(stamp_dir), key);
            if self.usable(output_dir, name, key).is_some() {
                self.mark_used(name, key, output_dir);
                continue;
            }
            match self.store_one(output_dir, graph, name, key, stamp_dir) {
                Ok(()) => stored.push(name.to_string()),
                Err(reason) => skipped.push(format!("{name}: {reason}")),
            }
        }
        if !stored.is_empty() {
            self.evict();
        }
        (stored, skipped)
    }

    fn store_one(
        &self,
        output_dir: &Path,
        graph: &PackageGraph,
        name: &str,
        key: &str,
        stamp_dir: &str,
    ) -> Result<(), String> {
        let per_package = format!("per-package/{name}");
        if !output_dir.join(&per_package).is_dir() {
            return Err("no per-package directory".to_string());
        }
        let output_text = output_dir.display().to_string();
        let dependencies = graph
            .get(name)
            .map(|package| package.dependencies.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        // Its own files: everything in its per-package trees that is not a
        // hard link to the file at the same path in a direct dependency's
        // trees (how Buildroot copies dependencies in; those hold their own
        // dependencies' files the same way). This includes what it
        // installs outside its install steps, such as an external
        // toolchain extracted into the host directory.
        let mut own = Vec::new();
        for tree in ["host", "target"] {
            let root = format!("{per_package}/{tree}");
            walk_files(output_dir, &root, &mut |member, metadata| {
                let relative = &member[root.len() + 1..];
                let inode = (metadata.dev(), metadata.ino());
                let from_dependency = dependencies.iter().any(|dependency| {
                    fs::symlink_metadata(
                        output_dir
                            .join("per-package")
                            .join(dependency)
                            .join(tree)
                            .join(relative),
                    )
                    .is_ok_and(|other| (other.dev(), other.ino()) == inode)
                });
                if !from_dependency {
                    own.push((member, metadata.is_file(), metadata.len()));
                }
            });
        }
        if let Ok(images) =
            fs::read_to_string(output_dir.join(stamp_dir).join(".files-list-images.txt"))
        {
            for line in images.lines() {
                let Some((owner, path)) = line.split_once(',') else {
                    continue;
                };
                let path = path.trim_start_matches("./");
                let normal = Path::new(path)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)));
                let member = format!("images/{path}");
                if owner == name
                    && normal
                    && let Ok(metadata) = fs::symlink_metadata(output_dir.join(&member))
                {
                    own.push((member, metadata.is_file(), metadata.len()));
                }
            }
        }
        // Build-tree files Buildroot or Gaia read after the build: the file
        // lists, the kernel's module list, a kconfig package's `.config`.
        for file in FILE_LISTS.iter().chain(&["modules.order", ".config"]) {
            let member = format!("{stamp_dir}/{file}");
            if let Ok(metadata) = fs::symlink_metadata(output_dir.join(&member)) {
                own.push((member, metadata.is_file(), metadata.len()));
            }
        }
        let mut files = BTreeSet::new();
        let mut rewrite = Vec::new();
        let mut relocatable = true;
        let mut size = 0u64;
        for (member, regular, length) in own {
            if regular {
                size += length;
                match file_holds(&output_dir.join(&member), output_text.as_bytes()) {
                    Ok(Holds::Text) => rewrite.push(member.clone()),
                    Ok(Holds::Binary) => relocatable = false,
                    Ok(Holds::No) => {}
                    Err(error) => return Err(format!("{member}: {error}")),
                }
            }
            files.insert(member);
        }
        // Directories too, so empty ones the package created survive.
        let mut dir_names = BTreeSet::new();
        for root in ["target", "host"] {
            collect_dirs(output_dir, &format!("{per_package}/{root}"), &mut dir_names);
        }
        for file in &files {
            let mut parent = Path::new(file).parent();
            while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
                dir_names.insert(dir.to_string_lossy().into_owned());
                parent = dir.parent();
            }
        }
        let dirs = dir_names
            .into_iter()
            .map(|dir| {
                let mode = fs::metadata(output_dir.join(&dir))
                    .map(|metadata| metadata.mode() & 0o7777)
                    .unwrap_or(0o755);
                (dir, mode)
            })
            .collect::<Vec<_>>();
        let files = files.into_iter().collect::<Vec<_>>();
        let stamps = stamps_in(&output_dir.join(stamp_dir));

        let (entry, manifest_path) = self.entry(name, key, (!relocatable).then_some(output_dir));
        let parent = entry.parent().ok_or("no cache directory")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            entry.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temporary);
        clone_members(output_dir, &temporary, &dirs, &files).inspect_err(|_| {
            let _ = fs::remove_dir_all(&temporary);
        })?;
        let manifest = Manifest {
            output_dir: output_text,
            dirs,
            files,
            size,
            stamps,
            rewrite,
            relocatable,
        };
        let _ = fs::remove_dir_all(&entry);
        fs::rename(&temporary, &entry)
            .and_then(|()| {
                fs::write(
                    &manifest_path,
                    manifest.to_json(
                        name,
                        graph
                            .get(name)
                            .and_then(|package| package.version.as_deref()),
                    ),
                )
            })
            .map_err(|error| {
                let _ = fs::remove_dir_all(&temporary);
                let _ = fs::remove_dir_all(&entry);
                error.to_string()
            })
    }

    /// Removes the least recently used entries beyond `max_size`, by their
    /// manifests (size, and last use as the manifest's modification time).
    fn evict(&self) {
        for (_, level) in self.levels() {
            evict_level(level, self.max_size);
        }
    }
}

/// Removes a level's least recently used entries beyond `max_size`.
fn evict_level(level: &Path, max_size: u64) {
    let mut entries = Vec::new();
    let mut total = 0u64;
    for package in fs::read_dir(level).into_iter().flatten().flatten() {
        for file in fs::read_dir(package.path()).into_iter().flatten().flatten() {
            let path = file.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Some(manifest) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| Manifest::parse(&text))
            else {
                continue;
            };
            let used = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            total += manifest.size;
            entries.push((used, manifest.size, path));
        }
    }
    if total <= max_size {
        return;
    }
    entries.sort();
    let target = max_size / 10 * 9;
    for (_, size, manifest) in entries {
        if total <= target {
            break;
        }
        let _ = gaia_process::discard(&manifest.with_extension(""));
        if fs::remove_file(&manifest).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

#[cfg(test)]
#[path = "package_cache_tests.rs"]
mod tests;
