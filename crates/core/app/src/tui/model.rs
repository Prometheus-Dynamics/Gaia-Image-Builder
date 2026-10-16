use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Picker,
    Setup,
    /// The details of the build: plan, validation, reports, spec and events.
    Details,
    Monitor,
    /// A `gaia run` another process started: its status and recent output.
    Attach,
}

/// Which section of the start screen the cursor is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerFocus {
    /// The running and recently ended builds, from anywhere on the system.
    Runs,
    /// The build configs of this directory.
    Builds,
}

/// The items of the setup form, in order. Form items are edited in place;
/// the detail items open the details screen on their tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupItem {
    StartBuild,
    Branch,
    Target,
    Profile,
    Input(String),
    /// The variant of a build that declares `[[variants]]`.
    Variant,
    Jobs,
    Plan,
    Validation,
    Reports,
    Spec,
    PickBuild,
    Refresh,
}

impl SetupItem {
    pub(crate) fn defaults() -> Vec<Self> {
        vec![
            Self::StartBuild,
            Self::Branch,
            Self::Target,
            Self::Profile,
            Self::Jobs,
            Self::Plan,
            Self::Validation,
            Self::Reports,
            Self::Spec,
            Self::PickBuild,
            Self::Refresh,
        ]
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            Self::StartBuild => "Start build",
            Self::Branch => "Branch",
            Self::Target => "Target",
            Self::Profile => "Profile",
            Self::Input(name) => name.as_str(),
            Self::Variant => "Variant",
            Self::Jobs => "Jobs",
            Self::Plan => "Plan",
            Self::Validation => "Validation",
            Self::Reports => "Reports",
            Self::Spec => "Spec",
            Self::PickBuild => "Pick build",
            Self::Refresh => "Refresh",
        }
    }

    /// The details tab this item opens, for the detail items.
    pub(crate) fn details_tab(&self) -> Option<DetailTab> {
        match self {
            Self::Plan => Some(DetailTab::Plan),
            Self::Validation => Some(DetailTab::Validation),
            Self::Reports => Some(DetailTab::Reports),
            Self::Spec => Some(DetailTab::Spec),
            _ => None,
        }
    }
}

/// The tabs of the details screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailTab {
    Plan,
    Validation,
    Reports,
    Spec,
    Events,
}

impl DetailTab {
    pub(crate) const ALL: [DetailTab; 5] = [
        Self::Plan,
        Self::Validation,
        Self::Reports,
        Self::Spec,
        Self::Events,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Validation => "Validation",
            Self::Reports => "Reports",
            Self::Spec => "Spec",
            Self::Events => "Events",
        }
    }

    /// Moves to the tab `step` away, wrapping.
    pub(crate) fn step(self, step: isize) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0) as isize;
        Self::ALL[(index + step).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupEditField {
    Branch,
    Input(String),
    Jobs,
}

pub(crate) struct BuildEntry {
    pub(crate) label: String,
    pub(crate) path: String,
}

pub(crate) enum RunThreadMessage {
    Event(ExecutionEvent),
    Finished(Box<Result<RunArtifacts, String>>),
}

pub(crate) struct RefreshArtifacts {
    pub(crate) options: ResolveOptions,
    pub(crate) spec: ResolvedBuildSpec,
    pub(crate) validation: ValidationReport,
    pub(crate) plan: ExecutionPlan,
    pub(crate) plan_diagnostics: Vec<gaia_plan::PlanDiagnostic>,
    pub(crate) operation_durations: std::collections::BTreeMap<String, u64>,
}

pub(crate) struct RefreshThreadMessage {
    pub(crate) revision: u64,
    pub(crate) result: Result<RefreshArtifacts, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitState {
    Idle,
    /// Quit was pressed once during a build; the next press cancels it.
    Armed,
    /// Cancellation was requested; exit when the run thread finishes.
    CancelThenExit,
}

pub(crate) enum RunState {
    Idle,
    Running {
        receiver: Receiver<RunThreadMessage>,
        cancellation: ExecutionCancellation,
        started_at: Instant,
        spinner_tick: usize,
    },
}
