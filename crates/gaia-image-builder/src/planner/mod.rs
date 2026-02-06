use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::error::{Error, Result};

pub type TaskId = String;

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub label: String,
    pub module: String,
    pub phase: String,
    pub after: Vec<TaskId>,
    pub provides: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Plan {
    tasks: BTreeMap<TaskId, Task>,
}

impl Plan {
    pub fn add(&mut self, task: Task) -> Result<()> {
        if self.tasks.contains_key(&task.id) {
            return Err(Error::msg(format!("duplicate task id '{}'", task.id)));
        }
        self.tasks.insert(task.id.clone(), task);
        Ok(())
    }

    fn provides_index(&self) -> Result<BTreeMap<&str, &str>> {
        let mut out: BTreeMap<&str, &str> = BTreeMap::new();
        for (id, task) in &self.tasks {
            for p in &task.provides {
                if let Some(existing) = out.insert(p.as_str(), id.as_str()) {
                    return Err(Error::msg(format!(
                        "provide token '{}' is produced by both '{}' and '{}'",
                        p, existing, id
                    )));
                }
            }
        }
        Ok(out)
    }

    fn resolve_dep<'a>(
        &'a self,
        provides: &BTreeMap<&'a str, &'a str>,
        dep: &'a str,
    ) -> Result<&'a str> {
        if self.tasks.contains_key(dep) {
            return Ok(dep);
        }
        if let Some(&provider) = provides.get(dep) {
            return Ok(provider);
        }
        Err(Error::msg(format!("unknown dependency '{}'", dep)))
    }

    fn resolve_dep_maybe<'a>(
        &'a self,
        provides: &BTreeMap<&'a str, &'a str>,
        dep: &'a str,
    ) -> Result<Option<&'a str>> {
        let (dep, optional) = dep
            .strip_suffix('?')
            .map(|d| (d, true))
            .unwrap_or((dep, false));
        match self.resolve_dep(provides, dep) {
            Ok(id) => Ok(Some(id)),
            Err(e) => {
                if optional {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    pub fn ordered(&self) -> Result<Vec<&Task>> {
        let provides = self.provides_index()?;

        // Build adjacency from "after" constraints.
        let mut incoming: BTreeMap<&str, usize> = BTreeMap::new();
        let mut outgoing: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();

        for (id, task) in &self.tasks {
            incoming.insert(id.as_str(), 0);
            outgoing.entry(id.as_str()).or_default();
            for dep in &task.after {
                let Some(dep_id) =
                    self.resolve_dep_maybe(&provides, dep.as_str())
                        .map_err(|e| {
                            Error::msg(format!(
                                "task '{}' has invalid dependency '{}': {}",
                                id, dep, e
                            ))
                        })?
                else {
                    continue;
                };

                outgoing.entry(dep_id).or_default().insert(id.as_str());
                *incoming.get_mut(id.as_str()).unwrap() += 1;
            }
        }

        let mut q: VecDeque<&str> = incoming
            .iter()
            .filter_map(|(k, v)| (*v == 0).then_some(*k))
            .collect();
        let mut out: Vec<&str> = Vec::with_capacity(self.tasks.len());

        while let Some(n) = q.pop_front() {
            out.push(n);
            if let Some(children) = outgoing.get(n) {
                for &m in children {
                    let slot = incoming.get_mut(m).unwrap();
                    *slot -= 1;
                    if *slot == 0 {
                        q.push_back(m);
                    }
                }
            }
        }

        if out.len() != self.tasks.len() {
            // cycle; attempt to show the nodes involved
            let remaining: Vec<&str> = incoming
                .iter()
                .filter_map(|(k, v)| (*v > 0).then_some(*k))
                .collect();
            return Err(Error::msg(format!(
                "task graph contains a cycle; remaining nodes: {}",
                remaining.join(", ")
            )));
        }

        Ok(out
            .into_iter()
            .map(|id| self.tasks.get(id).expect("task must exist"))
            .collect())
    }

    pub fn finalize_default(&mut self) -> Result<()> {
        self.add_stage_barrier()
    }

    fn add_stage_barrier(&mut self) -> Result<()> {
        // Global ordering primitive:
        // - Any task that provides a token starting with "stage:" is considered a "staging" producer.
        // - We add a barrier task that depends on all of them and provides "stage:done".
        // Consumers (like buildroot.configure) depend only on "stage:done" rather than enumerating modules.
        const BARRIER_ID: &str = "core.barrier.stage";
        if self.tasks.contains_key(BARRIER_ID) {
            return Ok(());
        }

        let mut deps: Vec<String> = Vec::new();
        for t in self.tasks.values() {
            if t.id == BARRIER_ID {
                continue;
            }
            if t.provides.iter().any(|p| p.starts_with("stage:")) {
                deps.push(t.id.clone());
            }
        }
        deps.sort();

        self.add(Task {
            id: BARRIER_ID.into(),
            label: "Stage barrier".into(),
            module: "core".into(),
            phase: "barrier".into(),
            after: deps,
            provides: vec!["stage:done".into()],
        })?;
        Ok(())
    }

    pub fn to_dot(&self) -> Result<String> {
        let provides = self.provides_index()?;

        let mut out = String::from("digraph plan {\n  rankdir=LR;\n");
        for task in self.tasks.values() {
            out.push_str(&format!(
                "  \"{}\" [label=\"{}\\n{}:{}\"];\n",
                task.id, task.label, task.module, task.phase
            ));
        }
        for task in self.tasks.values() {
            for dep in &task.after {
                let Some(dep_id) =
                    self.resolve_dep_maybe(&provides, dep.as_str())
                        .map_err(|e| {
                            Error::msg(format!(
                                "task '{}' has invalid dependency '{}': {}",
                                task.id, dep, e
                            ))
                        })?
                else {
                    continue;
                };
                out.push_str(&format!("  \"{}\" -> \"{}\";\n", dep_id, task.id));
            }
        }
        out.push_str("}\n");
        Ok(out)
    }
}
