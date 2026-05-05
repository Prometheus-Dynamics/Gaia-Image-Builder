use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Picker,
    Setup,
    Monitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailView {
    Overview,
    Selection,
    Validation,
    Plan,
    Events,
    Logs,
    Reports,
    Spec,
}

impl DetailView {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Selection => "Selection",
            Self::Validation => "Validation",
            Self::Plan => "Plan",
            Self::Events => "Events",
            Self::Logs => "Logs",
            Self::Reports => "Reports",
            Self::Spec => "Spec",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupItem {
    StartBuild,
    Branch,
    Target,
    Profile,
    Input(String),
    Jobs,
    Overview,
    Selection,
    Validation,
    Plan,
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
            Self::Overview,
            Self::Selection,
            Self::Validation,
            Self::Plan,
            Self::Reports,
            Self::Spec,
            Self::PickBuild,
            Self::Refresh,
        ]
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            Self::StartBuild => "Start Build",
            Self::Branch => "Branch",
            Self::Target => "Target",
            Self::Profile => "Profile",
            Self::Input(name) => name.as_str(),
            Self::Jobs => "Jobs",
            Self::Overview => "Overview",
            Self::Selection => "Selection",
            Self::Validation => "Validation",
            Self::Plan => "Plan",
            Self::Reports => "Reports",
            Self::Spec => "Spec",
            Self::PickBuild => "Pick Build",
            Self::Refresh => "Refresh",
        }
    }

    pub(crate) fn detail_view(&self) -> DetailView {
        match self {
            Self::StartBuild
            | Self::Branch
            | Self::Target
            | Self::Profile
            | Self::Input(_)
            | Self::Jobs
            | Self::Overview => DetailView::Overview,
            Self::Selection => DetailView::Selection,
            Self::Validation => DetailView::Validation,
            Self::Plan => DetailView::Plan,
            Self::Reports => DetailView::Reports,
            Self::Spec => DetailView::Spec,
            Self::PickBuild | Self::Refresh => DetailView::Overview,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitorView {
    Overview,
    Events,
    Logs,
    Reports,
    Spec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupEditField {
    Branch,
    Input(String),
    Jobs,
}

impl MonitorView {
    pub(crate) fn all() -> &'static [Self] {
        &[
            Self::Overview,
            Self::Events,
            Self::Logs,
            Self::Reports,
            Self::Spec,
        ]
    }

    pub(crate) fn detail_view(self) -> DetailView {
        match self {
            Self::Overview => DetailView::Overview,
            Self::Events => DetailView::Events,
            Self::Logs => DetailView::Logs,
            Self::Reports => DetailView::Reports,
            Self::Spec => DetailView::Spec,
        }
    }
}

pub(crate) struct BuildEntry {
    pub(crate) label: String,
    pub(crate) path: String,
}

pub(crate) struct OperationItem {
    pub(crate) label: String,
    pub(crate) status: &'static str,
    pub(crate) color: Color,
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
}

pub(crate) struct RefreshThreadMessage {
    pub(crate) revision: u64,
    pub(crate) result: Result<RefreshArtifacts, String>,
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
