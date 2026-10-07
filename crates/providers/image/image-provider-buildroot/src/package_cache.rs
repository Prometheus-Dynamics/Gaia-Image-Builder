//! A cache of built Buildroot packages shared by every build of the user,
//! like Yocto's sstate for Buildroot packages.
//!
//! After a successful `make`, each package built in this run is archived:
//! the files it installed (Buildroot's per-package `.files-list*.txt`), read
//! from its per-package directories (`BR2_PER_PACKAGE_DIRECTORIES`), its
//! file lists, and the names of its stamps. The archive is stored under the
//! package's key ([`package_keys`]). Before the next `make` of any build,
//! packages that are not built yet and whose key is cached are restored
//! instead: their files are extracted, the output directory path is
//! rewritten in the text files that held the old one, and their stamps are
//! recreated in order, so `make` treats them as built.
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

const DEFAULT_PACKAGE_CACHE_DIR: &str = "buildroot/packages";
const DEFAULT_PACKAGE_CACHE_MAX_SIZE: &str = "100G";

/// Stamps in the order Buildroot creates them; restored stamps get
/// increasing modification times in this order.
const STAMP_ORDER: &[&str] = &[
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
    dir: PathBuf,
    max_size: u64,
}

/// The configured package cache, or `None` when it is off. It needs
/// per-package directories (`parallel_packages`), which hold each package's
/// files apart from its dependencies'.
pub(crate) fn package_cache(
    spec: &ResolvedBuildSpec,
    policy: &ImageExecutionPolicy,
) -> Result<Option<PackageCache>, ImageProviderError> {
    if !policy.package_cache_enabled || !policy.parallel_packages {
        return Ok(None);
    }
    let dir = match policy.package_cache_dir.as_deref() {
        Some(dir) => ensure_cache_dir(spec, dir, "buildroot package cache")?,
        None => default_cache_dir(spec, DEFAULT_PACKAGE_CACHE_DIR, "buildroot package cache")?,
    };
    let max_size = policy
        .package_cache_max_size
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
    Ok(Some(PackageCache { dir, max_size }))
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
        Some(Self {
            output_dir: value.get("output_dir")?.as_str()?.to_string(),
            stamps: strings("stamps")?,
            rewrite: strings("rewrite")?,
            relocatable: value.get("relocatable")?.as_bool()?,
        })
    }
}

impl PackageCache {
    /// The archive and manifest of a package build: relocatable, or
    /// `pinned` to the output directory it was built in.
    fn entry(&self, name: &str, key: &str, pinned: Option<&Path>) -> (PathBuf, PathBuf) {
        let dir = self.dir.join(name);
        let stem = match pinned {
            None => key.to_string(),
            Some(output_dir) => {
                let digest = sha2::Sha256::digest(output_dir.display().to_string().as_bytes());
                format!("{key}@{}", &hex(&digest)[..16])
            }
        };
        (
            dir.join(format!("{stem}.tar.zst")),
            dir.join(format!("{stem}.json")),
        )
    }

    /// A cached build of the package that can be restored into
    /// `output_dir`: a relocatable one, or one built at that path.
    fn usable(&self, output_dir: &Path, name: &str, key: &str) -> Option<(PathBuf, Manifest)> {
        [None, Some(output_dir)].into_iter().find_map(|pinned| {
            let (archive, manifest) = self.entry(name, key, pinned);
            let manifest = Manifest::parse(&fs::read_to_string(manifest).ok()?)?;
            (archive.is_file()
                && (manifest.relocatable || Path::new(&manifest.output_dir) == output_dir))
                .then_some((archive, manifest))
        })
    }

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
            match self.restore_one(output_dir, graph, &name, key) {
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
        for dependency in recursive_dependencies(graph, name) {
            for tree in ["host", "target"] {
                let source = per_package.join(&dependency).join(tree);
                if !source.is_dir() {
                    continue;
                }
                let destination = per_package.join(name).join(tree);
                fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
                let status = Command::new("rsync")
                    .arg("-a")
                    .arg(format!("--link-dest={}/", source.display()))
                    .arg(format!("{}/", source.display()))
                    .arg(format!("{}/", destination.display()))
                    .status()
                    .map_err(|error| format!("rsync: {error}"))?;
                if !status.success() {
                    return Err(format!("rsync of {dependency} exited with {status}"));
                }
            }
        }
        let status = Command::new("tar")
            .arg("-I")
            .arg("zstd -d -q")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(output_dir)
            .status()
            .map_err(|error| format!("tar: {error}"))?;
        if !status.success() {
            return Err(format!("tar exited with {status}"));
        }
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
        // Recently used entries are evicted last.
        if let Ok(file) = fs::File::options().append(true).open(&archive) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
        Ok(())
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
        let mut inodes = InodeIndex::default();
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
            if let Some((archive, _)) = self.usable(output_dir, name, key) {
                if let Ok(file) = fs::File::options().append(true).open(&archive) {
                    let _ = file.set_modified(std::time::SystemTime::now());
                }
                continue;
            }
            match self.store_one(output_dir, graph, name, key, stamp_dir, &mut inodes) {
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
        inodes: &mut InodeIndex,
    ) -> Result<(), String> {
        let per_package = format!("per-package/{name}");
        if !output_dir.join(&per_package).is_dir() {
            return Err("no per-package directory".to_string());
        }
        let output_text = output_dir.display().to_string();
        let mut members = BTreeSet::new();
        let mut rewrite = Vec::new();
        let mut relocatable = true;
        // Its own files: everything in its per-package trees that is not a
        // hard link to a dependency's file (how Buildroot copies those in).
        // This includes what it installs outside its install steps, such as
        // an external toolchain extracted into the host directory.
        let dependency_files = graph
            .get(name)
            .map(|package| package.dependencies.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|dependency| inodes.of(output_dir, &dependency))
            .collect::<Vec<_>>();
        let mut own = Vec::new();
        for tree in ["host", "target"] {
            walk_files(
                output_dir,
                &format!("{per_package}/{tree}"),
                &mut |member, metadata| {
                    let inode = (metadata.dev(), metadata.ino());
                    if !dependency_files.iter().any(|files| files.contains(&inode)) {
                        own.push((member, metadata.is_file()));
                    }
                },
            );
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
                if owner == name && normal && output_dir.join(&member).exists() {
                    own.push((member, true));
                }
            }
        }
        for (member, regular) in own {
            if regular {
                let contents = fs::read(output_dir.join(&member))
                    .map_err(|error| format!("{member}: {error}"))?;
                if find_bytes(&contents, output_text.as_bytes()).is_some() {
                    if contents.contains(&0) {
                        relocatable = false;
                    } else {
                        rewrite.push(member.clone());
                    }
                }
            }
            members.insert(member);
        }
        for list in FILE_LISTS {
            if output_dir.join(stamp_dir).join(list).is_file() {
                members.insert(format!("{stamp_dir}/{list}"));
            }
        }
        // Build-tree files Gaia or Buildroot read after the build: the
        // kernel's module list and a kconfig package's configuration.
        for file in ["modules.order", ".config"] {
            if output_dir.join(stamp_dir).join(file).is_file() {
                members.insert(format!("{stamp_dir}/{file}"));
            }
        }
        // Directories too, so empty ones the package created survive.
        for root in ["target", "host"] {
            collect_dirs(output_dir, &format!("{per_package}/{root}"), &mut members);
        }
        let stamps = stamps_in(&output_dir.join(stamp_dir));

        let (archive, manifest_path) = self.entry(name, key, (!relocatable).then_some(output_dir));
        let entry_dir = archive.parent().ok_or("no cache directory")?;
        fs::create_dir_all(entry_dir).map_err(|error| error.to_string())?;
        let temporary = entry_dir.join(format!(".{key}.{}.tmp", std::process::id()));
        let list_path = entry_dir.join(format!(".{key}.{}.list", std::process::id()));
        let list = members
            .iter()
            .flat_map(|member| member.bytes().chain(std::iter::once(0)))
            .collect::<Vec<_>>();
        fs::write(&list_path, list).map_err(|error| error.to_string())?;
        // `-C` applies to the names that follow it, so it comes first.
        let status = Command::new("tar")
            .arg("-C")
            .arg(output_dir)
            .arg("--null")
            .arg("--no-recursion")
            .arg("-T")
            .arg(&list_path)
            .arg("-I")
            .arg("zstd -T0 -3 -q")
            .arg("-cf")
            .arg(&temporary)
            .stdin(std::process::Stdio::null())
            .output();
        let _ = fs::remove_file(&list_path);
        match status {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let _ = fs::remove_file(&temporary);
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "tar exited with {}: {}",
                    output.status,
                    stderr.lines().take(3).collect::<Vec<_>>().join(" | ")
                ));
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(format!("tar: {error}"));
            }
        }
        let manifest = Manifest {
            output_dir: output_text,
            stamps,
            rewrite,
            relocatable,
        };
        fs::write(
            &manifest_path,
            manifest.to_json(
                name,
                graph
                    .get(name)
                    .and_then(|package| package.version.as_deref()),
            ),
        )
        .and_then(|()| fs::rename(&temporary, &archive))
        .map_err(|error| {
            let _ = fs::remove_file(&temporary);
            let _ = fs::remove_file(&manifest_path);
            error.to_string()
        })
    }

    /// Removes the least recently used archives beyond `max_size`.
    fn evict(&self) {
        let mut archives = Vec::new();
        let mut total = 0u64;
        for package in fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            for entry in fs::read_dir(package.path()).into_iter().flatten().flatten() {
                let path = entry.path();
                if !path.to_string_lossy().ends_with(".tar.zst") {
                    continue;
                }
                if let Ok(metadata) = entry.metadata() {
                    total += metadata.len();
                    let used = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
                    archives.push((used, metadata.len(), path));
                }
            }
        }
        if total <= self.max_size {
            return;
        }
        archives.sort();
        let target = self.max_size / 10 * 9;
        for (_, size, archive) in archives {
            if total <= target {
                break;
            }
            let _ = fs::remove_file(archive.with_extension("").with_extension("json"));
            if fs::remove_file(&archive).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

/// Package names, each after all its dependencies.
fn dependency_order(graph: &PackageGraph) -> Vec<String> {
    fn visit(
        graph: &PackageGraph,
        name: &str,
        seen: &mut BTreeSet<String>,
        order: &mut Vec<String>,
    ) {
        if !seen.insert(name.to_string()) {
            return;
        }
        if let Some(package) = graph.get(name) {
            for dependency in &package.dependencies {
                visit(graph, dependency, seen, order);
            }
            order.push(name.to_string());
        }
    }
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for name in graph.package_names() {
        visit(graph, name, &mut seen, &mut order);
    }
    order
}

/// Every package `name` depends on, directly or not.
fn recursive_dependencies(graph: &PackageGraph, name: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut queue = graph
        .get(name)
        .map(|package| package.dependencies.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    while let Some(dependency) = queue.pop() {
        if all.insert(dependency.clone())
            && let Some(package) = graph.get(&dependency)
        {
            queue.extend(package.dependencies.iter().cloned());
        }
    }
    all
}

/// Inodes of the files in each package's per-package trees, computed once.
#[derive(Default)]
struct InodeIndex {
    packages: BTreeMap<String, std::collections::HashSet<(u64, u64)>>,
}

impl InodeIndex {
    fn of(&mut self, output_dir: &Path, name: &str) -> std::collections::HashSet<(u64, u64)> {
        self.packages
            .entry(name.to_string())
            .or_insert_with(|| {
                let mut inodes = std::collections::HashSet::new();
                for tree in ["host", "target"] {
                    walk_files(
                        output_dir,
                        &format!("per-package/{name}/{tree}"),
                        &mut |_, metadata| {
                            inodes.insert((metadata.dev(), metadata.ino()));
                        },
                    );
                }
                inodes
            })
            .clone()
    }
}

/// Calls `visit` with the path (relative to `output_dir`) and metadata of
/// every file and symlink under `relative`.
fn walk_files(output_dir: &Path, relative: &str, visit: &mut dyn FnMut(String, fs::Metadata)) {
    let Ok(entries) = fs::read_dir(output_dir.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let member = format!("{relative}/{name}");
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.is_dir() {
            walk_files(output_dir, &member, visit);
        } else {
            visit(member, metadata);
        }
    }
}

fn collect_dirs(output_dir: &Path, relative: &str, members: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(output_dir.join(relative)) else {
        return;
    };
    members.insert(relative.to_string());
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Some(name) = entry.file_name().to_str()
        {
            collect_dirs(output_dir, &format!("{relative}/{name}"), members);
        }
    }
}

/// A package's stamps in the order its build created them (by modification
/// time, then Buildroot's usual order).
fn stamps_in(dir: &Path) -> Vec<String> {
    let mut stamps = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let modified = entry.metadata().ok()?.modified().ok()?;
            name.starts_with(".stamp_").then_some((modified, name))
        })
        .collect::<Vec<_>>();
    stamps.sort_by_key(|(modified, name)| {
        (
            *modified,
            STAMP_ORDER
                .iter()
                .position(|known| known == name)
                .unwrap_or(STAMP_ORDER.len() - 1),
            name.clone(),
        )
    });
    stamps.into_iter().map(|(_, name)| name).collect()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn replace_bytes(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(index) = find_bytes(rest, from) {
        result.extend_from_slice(&rest[..index]);
        result.extend_from_slice(to);
        rest = &rest[index + from.len()..];
    }
    result.extend_from_slice(rest);
    result
}

#[cfg(test)]
#[path = "package_cache_tests.rs"]
mod tests;
