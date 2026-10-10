//! Where the build's tree goes, decided from facts about the disk and RAM
//! (`[providers.buildroot] work_dir`). [`decide_work_dir`] is pure: the run
//! acts on its decision ([`super::ram_tree::place_work_dir`]) and `gaia
//! preview` reports it.
use super::*;

/// The `work_dir` setting: the build dir (`disk`), tmpfs (`ram`), or a
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkDirSetting {
    Disk,
    Ram,
    Path(PathBuf),
}

impl WorkDirSetting {
    pub(crate) fn parse(setting: &str) -> Self {
        match setting.trim() {
            "" | "disk" => Self::Disk,
            "ram" => Self::Ram,
            path => Self::Path(PathBuf::from(path)),
        }
    }

    /// The directory the tree goes under; `None` for disk.
    fn base(&self) -> Option<PathBuf> {
        match self {
            Self::Disk => None,
            Self::Ram => Some(PathBuf::from(RAM_BASE)),
            Self::Path(path) => Some(path.clone()),
        }
    }
}

/// The facts [`decide_work_dir`] decides from. [`work_dir_facts`] reads
/// them; nothing here changes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkDirFacts {
    pub(crate) setting: WorkDirSetting,
    /// `<build_dir>/image/buildroot-output`.
    pub(crate) output_dir: PathBuf,
    /// Where a tree that is not on disk goes ([`tree_dir`]).
    pub(crate) tree: PathBuf,
    /// Most RAM a RAM tree may use.
    pub(crate) budget: u64,
    /// RAM the tree needs: the last RAM tree's size, or the default.
    pub(crate) need: u64,
    /// Bytes of the tree already in RAM (counted as used).
    pub(crate) present: u64,
    pub(crate) available_memory: u64,
    /// Free space on the tree's filesystem.
    pub(crate) tmpfs_free: u64,
    /// Where `output_dir` links to, when it is a link.
    pub(crate) link_target: Option<PathBuf>,
    /// A real file or directory is at `output_dir`.
    pub(crate) output_is_real: bool,
    pub(crate) tree_exists: bool,
}

/// Where the build's tree goes, and what moving it there discards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkDirDecision {
    /// The usual tree in the build dir. `dropped`: where a link from an
    /// earlier RAM or work dir build pointed; that tree is discarded.
    Disk { dropped: Option<PathBuf> },
    /// A tree in RAM or another directory.
    Tree(TreePlacement),
    /// RAM does not fit: the build runs on disk.
    RamFallback {
        shortfall: RamShortfall,
        dropped: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreePlacement {
    pub(crate) dir: PathBuf,
    pub(crate) ram: bool,
    /// The build dir links elsewhere: that link is replaced (its tree is
    /// kept).
    pub(crate) replaced_link: bool,
    /// A real tree sits in the build dir: it is discarded, and the packages
    /// come back from the package cache.
    pub(crate) moved_from_disk: bool,
    /// The build dir must (re)link to `dir`.
    pub(crate) relink: bool,
    /// The build dir links here, but the tree is gone (a reboot).
    pub(crate) gone: bool,
    /// `dir` is created.
    pub(crate) create: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RamShortfall {
    pub(crate) need: u64,
    pub(crate) more: u64,
    pub(crate) budget: u64,
    pub(crate) available_memory: u64,
    pub(crate) tmpfs_free: u64,
    pub(crate) base: PathBuf,
}

impl RamShortfall {
    fn message(&self) -> String {
        format!(
            "RAM build: the tree needs about {} and {} more is not free (budget {}, \
             available memory {}, {} free in {}); building on disk",
            gib(self.need),
            gib(self.more),
            gib(self.budget),
            gib(self.available_memory),
            gib(self.tmpfs_free),
            self.base.display()
        )
    }
}

impl WorkDirDecision {
    /// The directory the build runs in.
    pub(crate) fn tree(&self, output_dir: &Path) -> PathBuf {
        match self {
            Self::Tree(placement) => placement.dir.clone(),
            Self::Disk { .. } | Self::RamFallback { .. } => output_dir.to_path_buf(),
        }
    }

    /// Whether the tree is in RAM.
    pub(crate) fn is_ram(&self) -> bool {
        matches!(self, Self::Tree(TreePlacement { ram: true, .. }))
    }
}

/// Decides where the build's tree goes from `facts`. Pure: this is the
/// decision a run acts on, and what a preview reports.
pub(crate) fn decide_work_dir(facts: &WorkDirFacts) -> WorkDirDecision {
    let setting = &facts.setting;
    if *setting == WorkDirSetting::Disk {
        return WorkDirDecision::Disk {
            dropped: facts.link_target.clone(),
        };
    }
    let ram = *setting == WorkDirSetting::Ram;
    if ram {
        // A tree already in RAM is already counted as used memory.
        let more = facts.need.saturating_sub(facts.present);
        let allowed = facts
            .budget
            .saturating_sub(facts.present)
            .min(facts.available_memory.saturating_sub(RAM_MARGIN))
            .min(facts.tmpfs_free);
        if facts.need > facts.budget || more > allowed {
            return WorkDirDecision::RamFallback {
                shortfall: RamShortfall {
                    need: facts.need,
                    more,
                    budget: facts.budget,
                    available_memory: facts.available_memory,
                    tmpfs_free: facts.tmpfs_free,
                    base: setting.base().unwrap_or_default(),
                },
                dropped: facts.link_target.clone(),
            };
        }
    }
    let pointing_here = facts.link_target.as_deref() == Some(facts.tree.as_path());
    WorkDirDecision::Tree(TreePlacement {
        dir: facts.tree.clone(),
        ram,
        replaced_link: !pointing_here && facts.link_target.is_some(),
        moved_from_disk: !pointing_here && facts.link_target.is_none() && facts.output_is_real,
        relink: !pointing_here,
        gone: pointing_here && !facts.tree_exists,
        create: !facts.tree_exists,
    })
}

/// The messages a placement adds to the run.
pub(crate) fn work_dir_messages(decision: &WorkDirDecision, output_dir: &Path) -> Vec<String> {
    let dropped_message = |dropped: &Option<PathBuf>| {
        dropped.as_ref().map(|target| {
            format!(
                "dropped the Buildroot tree at '{}'; building in '{}'",
                target.display(),
                output_dir.display()
            )
        })
    };
    match decision {
        WorkDirDecision::Disk { dropped } => dropped_message(dropped).into_iter().collect(),
        WorkDirDecision::RamFallback { shortfall, dropped } => std::iter::once(shortfall.message())
            .chain(dropped_message(dropped))
            .collect(),
        WorkDirDecision::Tree(placement) => {
            let mut messages = Vec::new();
            if placement.moved_from_disk {
                messages.push(format!(
                    "moved the Buildroot tree from '{}' to '{}'; packages are restored from the \
                     package cache",
                    output_dir.display(),
                    placement.dir.display()
                ));
            }
            if placement.gone {
                messages.push(format!(
                    "the RAM tree '{}' was gone (a reboot?); starting a fresh one",
                    placement.dir.display()
                ));
            }
            messages.push(format!(
                "building the Buildroot tree in {} at '{}'",
                if placement.ram { "RAM" } else { "the work dir" },
                placement.dir.display()
            ));
            messages
        }
    }
}

/// Reads what [`decide_work_dir`] needs; changes nothing.
pub(crate) fn work_dir_facts(
    output_dir: &Path,
    policy: &ImageExecutionPolicy,
) -> Result<WorkDirFacts, ImageProviderError> {
    let setting = WorkDirSetting::parse(&policy.work_dir.work_dir);
    let base = setting.base();
    let tree = match &base {
        Some(base) => tree_dir(base, output_dir),
        None => output_dir.to_path_buf(),
    };
    let tree_exists = tree.is_dir();
    let mut facts = WorkDirFacts {
        setting: setting.clone(),
        output_dir: output_dir.to_path_buf(),
        tree: tree.clone(),
        budget: 0,
        need: 0,
        present: 0,
        available_memory: 0,
        tmpfs_free: 0,
        link_target: fs::read_link(output_dir).ok(),
        output_is_real: fs::symlink_metadata(output_dir)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink()),
        tree_exists,
    };
    if setting == WorkDirSetting::Ram {
        facts.budget = policy
            .work_dir
            .ram_budget
            .as_deref()
            .unwrap_or(DEFAULT_RAM_BUDGET)
            .parse::<gaia_spec::ByteSize>()
            .map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::PolicyBlocked,
                    format!("providers.buildroot.ram_budget: {error}"),
                )
            })?
            .bytes();
        facts.need = recorded_tree_size(output_dir).unwrap_or(DEFAULT_RAM_NEED);
        facts.present = if tree_exists { tree_size(&tree) } else { 0 };
        facts.available_memory = mem_available().unwrap_or(0);
        facts.tmpfs_free = base
            .as_deref()
            .and_then(filesystem_available_bytes)
            .unwrap_or(0);
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(setting: WorkDirSetting) -> WorkDirFacts {
        let output = PathBuf::from("/build/image/buildroot-output");
        WorkDirFacts {
            setting,
            output_dir: output.clone(),
            tree: PathBuf::from("/dev/shm/gaia-u/abc/buildroot-output"),
            budget: 60 * GIB,
            need: 30 * GIB,
            present: 0,
            available_memory: 100 * GIB,
            tmpfs_free: 100 * GIB,
            link_target: None,
            output_is_real: false,
            tree_exists: false,
        }
    }

    #[test]
    fn disk_drops_only_a_link_to_an_earlier_tree() {
        let mut disk = facts(WorkDirSetting::Disk);
        assert_eq!(
            decide_work_dir(&disk),
            WorkDirDecision::Disk { dropped: None }
        );
        disk.link_target = Some(PathBuf::from("/dev/shm/old"));
        let decision = decide_work_dir(&disk);
        assert_eq!(
            decision,
            WorkDirDecision::Disk {
                dropped: Some(PathBuf::from("/dev/shm/old"))
            }
        );
        assert!(
            work_dir_messages(&decision, &disk.output_dir)[0]
                .contains("dropped the Buildroot tree at '/dev/shm/old'")
        );
    }

    #[test]
    fn ram_falls_back_to_disk_when_the_tree_does_not_fit() {
        let mut ram = facts(WorkDirSetting::Ram);
        ram.need = 500 * GIB;
        let decision = decide_work_dir(&ram);
        assert!(matches!(decision, WorkDirDecision::RamFallback { .. }));
        assert!(work_dir_messages(&decision, &ram.output_dir)[0].contains("building on disk"));
        assert!(!decision.is_ram());
        assert_eq!(decision.tree(&ram.output_dir), ram.output_dir);
    }

    #[test]
    fn ram_counts_a_tree_already_in_ram_as_used_memory() {
        // A 50 GiB tree needs 40 GiB more; the budget leaves 50 GiB beside the
        // 10 GiB already in RAM, and memory leaves 92 GiB less the margin.
        let mut ram = facts(WorkDirSetting::Ram);
        ram.need = 50 * GIB;
        ram.present = 10 * GIB;
        ram.tree_exists = true;
        ram.link_target = Some(ram.tree.clone());
        assert!(matches!(decide_work_dir(&ram), WorkDirDecision::Tree(_)));
        // Only 40 GiB of memory: 32 GiB is free beyond the margin.
        ram.available_memory = 40 * GIB;
        assert!(matches!(
            decide_work_dir(&ram),
            WorkDirDecision::RamFallback { .. }
        ));
    }

    #[test]
    fn a_disk_tree_moves_into_ram_and_is_discarded() {
        let mut ram = facts(WorkDirSetting::Ram);
        ram.output_is_real = true;
        let decision = decide_work_dir(&ram);
        let WorkDirDecision::Tree(placement) = &decision else {
            panic!("expected a RAM tree: {decision:?}");
        };
        assert!(placement.ram && placement.moved_from_disk && placement.create);
        assert!(placement.relink && !placement.gone && !placement.replaced_link);
        assert!(decision.is_ram());
        assert!(
            work_dir_messages(&decision, &ram.output_dir)
                .iter()
                .any(|message| message.starts_with("moved the Buildroot tree from"))
        );
    }

    #[test]
    fn an_existing_ram_tree_is_kept_and_a_missing_one_starts_fresh() {
        let mut kept = facts(WorkDirSetting::Ram);
        kept.tree_exists = true;
        kept.link_target = Some(kept.tree.clone());
        let decision = decide_work_dir(&kept);
        let WorkDirDecision::Tree(placement) = &decision else {
            panic!("expected a RAM tree: {decision:?}");
        };
        assert!(!placement.relink && !placement.create && !placement.gone);
        assert!(!placement.moved_from_disk);
        assert_eq!(work_dir_messages(&decision, &kept.output_dir).len(), 1);

        // Linked here, but the tree is gone (a reboot).
        let mut gone = facts(WorkDirSetting::Ram);
        gone.link_target = Some(gone.tree.clone());
        let decision = decide_work_dir(&gone);
        let WorkDirDecision::Tree(placement) = &decision else {
            panic!("expected a RAM tree: {decision:?}");
        };
        assert!(placement.gone && placement.create && !placement.relink);
        assert!(
            work_dir_messages(&decision, &gone.output_dir)
                .iter()
                .any(|message| message.contains("was gone"))
        );
    }

    #[test]
    fn a_path_work_dir_replaces_a_link_to_another_tree() {
        let mut path = facts(WorkDirSetting::Path(PathBuf::from("/fast")));
        path.budget = 0;
        path.need = 500 * GIB;
        path.link_target = Some(PathBuf::from("/other/buildroot-output"));
        let decision = decide_work_dir(&path);
        let WorkDirDecision::Tree(placement) = &decision else {
            panic!("a path work dir never falls back: {decision:?}");
        };
        assert!(placement.replaced_link && placement.relink && !placement.moved_from_disk);
        assert!(!placement.ram);
    }

    #[test]
    fn work_dir_setting_parses_disk_ram_and_paths() {
        assert_eq!(WorkDirSetting::parse(""), WorkDirSetting::Disk);
        assert_eq!(WorkDirSetting::parse(" disk "), WorkDirSetting::Disk);
        assert_eq!(WorkDirSetting::parse("ram"), WorkDirSetting::Ram);
        assert_eq!(
            WorkDirSetting::parse("/fast"),
            WorkDirSetting::Path(PathBuf::from("/fast"))
        );
    }
}
