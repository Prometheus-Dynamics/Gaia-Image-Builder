use gaia_spec::BuildId;
use std::collections::{BTreeMap, BTreeSet};

use crate::OperationId;
use crate::PlannedOperation;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub build_id: BuildId,
    pub operations: Vec<PlannedOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDiagnostic {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReuseState {
    pub spec_fingerprint: u64,
    pub completed_operation_ids: BTreeSet<String>,
    pub operation_fingerprints: BTreeMap<String, u64>,
    pub operation_output_signatures: BTreeMap<String, String>,
}

impl ExecutionPlan {
    pub fn validate(&self) -> Vec<PlanDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut operation_ids = HashSet::new();

        for operation in &self.operations {
            if !operation_ids.insert(operation.id.as_str().to_string()) {
                diagnostics.push(PlanDiagnostic {
                    code: "duplicate_operation_id",
                    message: format!("duplicate operation id '{}'", operation.id.as_str()),
                });
            }
        }

        for operation in &self.operations {
            for dependency in &operation.depends_on {
                if !operation_ids.contains(dependency.as_str()) {
                    diagnostics.push(PlanDiagnostic {
                        code: "missing_dependency_node",
                        message: format!(
                            "operation '{}' depends on missing operation '{}'",
                            operation.id.as_str(),
                            dependency.as_str()
                        ),
                    });
                }
            }
        }

        let optionality_by_id: HashMap<&str, crate::OperationOptionality> = self
            .operations
            .iter()
            .map(|operation| (operation.id.as_str(), operation.optionality))
            .collect();

        for operation in &self.operations {
            if operation.optionality != crate::OperationOptionality::Required {
                continue;
            }
            for dependency in &operation.depends_on {
                if matches!(
                    optionality_by_id.get(dependency.as_str()),
                    Some(crate::OperationOptionality::BestEffort)
                ) {
                    diagnostics.push(PlanDiagnostic {
                        code: "required_depends_on_best_effort",
                        message: format!(
                            "required operation '{}' depends on best-effort operation '{}'",
                            operation.id.as_str(),
                            dependency.as_str()
                        ),
                    });
                }
            }
        }

        let graph: HashMap<&str, Vec<&str>> = self
            .operations
            .iter()
            .map(|operation| {
                (
                    operation.id.as_str(),
                    operation
                        .depends_on
                        .iter()
                        .map(OperationId::as_str)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();

        let mut permanent = HashSet::new();
        let mut visiting = Vec::<String>::new();
        for operation in &self.operations {
            detect_cycle(
                operation.id.as_str(),
                &graph,
                &mut permanent,
                &mut visiting,
                &mut diagnostics,
            );
        }

        diagnostics
    }
}

fn detect_cycle(
    operation_id: &str,
    graph: &HashMap<&str, Vec<&str>>,
    permanent: &mut HashSet<String>,
    visiting: &mut Vec<String>,
    diagnostics: &mut Vec<PlanDiagnostic>,
) {
    if permanent.contains(operation_id) {
        return;
    }

    if let Some(index) = visiting.iter().position(|id| id == operation_id) {
        let mut cycle = visiting[index..].to_vec();
        cycle.push(operation_id.to_string());
        diagnostics.push(PlanDiagnostic {
            code: "operation_cycle",
            message: format!("operation cycle detected: {}", cycle.join(" -> ")),
        });
        return;
    }

    let Some(dependencies) = graph.get(operation_id) else {
        return;
    };

    visiting.push(operation_id.to_string());
    for dependency in dependencies {
        if graph.contains_key(dependency) {
            detect_cycle(dependency, graph, permanent, visiting, diagnostics);
        }
    }
    visiting.pop();
    permanent.insert(operation_id.to_string());
}
