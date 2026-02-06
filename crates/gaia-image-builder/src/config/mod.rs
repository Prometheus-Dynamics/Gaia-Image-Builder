use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use toml::Value;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ConfigDoc {
    pub path: PathBuf,
    pub value: Value,
}

impl ConfigDoc {
    pub fn table(&self, key: &str) -> Option<&toml::value::Table> {
        self.value.as_table().and_then(|t| t.get(key)?.as_table())
    }

    pub fn has_table(&self, key: &str) -> bool {
        self.table(key).is_some()
    }

    pub fn value_path(&self, path: &str) -> Option<&Value> {
        let path = path.trim();
        if path.is_empty() {
            return Some(&self.value);
        }

        let mut cur = &self.value;
        for seg in path.split('.') {
            let tbl = cur.as_table()?;
            cur = tbl.get(seg)?;
        }
        Some(cur)
    }

    pub fn deserialize_path<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        let Some(v) = self.value_path(path) else {
            return Ok(None);
        };
        let owned = v.clone();
        let parsed = owned
            .try_into()
            .map_err(|e| Error::msg(format!("failed to deserialize config at '{}': {e}", path)))?;
        Ok(Some(parsed))
    }

    pub fn table_path(&self, path: &str) -> Option<&toml::value::Table> {
        let path = path.trim();
        if path.is_empty() {
            return self.value.as_table();
        }
        let mut cur = self.value.as_table()?;
        let mut it = path.split('.').peekable();
        while let Some(seg) = it.next() {
            let v = cur.get(seg)?;
            if it.peek().is_none() {
                return v.as_table();
            }
            cur = v.as_table()?;
        }
        None
    }

    pub fn has_table_path(&self, path: &str) -> bool {
        self.table_path(path).is_some()
    }
}

fn merge_values(base: &mut Value, child: Value) {
    match (base, child) {
        (Value::Table(base_tbl), Value::Table(child_tbl)) => {
            for (k, v) in child_tbl {
                match base_tbl.get_mut(&k) {
                    Some(existing) => merge_values(existing, v),
                    None => {
                        base_tbl.insert(k, v);
                    }
                }
            }
        }
        (base_slot, child_val) => {
            *base_slot = child_val;
        }
    }
}

pub fn merge(base: &mut Value, overlay: Value) {
    merge_values(base, overlay);
}

fn resolve_ref_path(from_file: &Path, reference: &str) -> PathBuf {
    let p = PathBuf::from(reference);
    if p.is_absolute() {
        p
    } else {
        from_file.parent().unwrap_or_else(|| Path::new(".")).join(p)
    }
}

fn parse_imports(path: &Path, table: &toml::value::Table) -> Result<Vec<String>> {
    let Some(arr) = table.get("imports").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for v in arr {
        let Some(s) = v.as_str() else {
            return Err(Error::msg(format!(
                "invalid imports entry in {} (expected string)",
                path.display()
            )));
        };
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        out.push(s.to_string());
    }
    Ok(out)
}

fn inline_imports_in_value(
    file_path: &Path,
    value: &mut Value,
    stack: &mut HashSet<PathBuf>,
) -> Result<()> {
    let Value::Table(tbl) = value else {
        return Ok(());
    };

    // Inline imports at this table level.
    let imports = parse_imports(file_path, tbl)?;
    if !imports.is_empty() {
        let mut acc = Value::Table(Default::default());
        for imp in imports {
            let imp_path = resolve_ref_path(file_path, &imp);
            let loaded = load_value_inner(&imp_path, stack)?;
            merge_values(&mut acc, loaded);
        }

        let mut local = Value::Table(tbl.clone());
        if let Some(local_tbl) = local.as_table_mut() {
            local_tbl.remove("imports");
        }
        merge_values(&mut acc, local);

        *tbl = acc.as_table().expect("acc must be a table").clone();
    } else {
        tbl.remove("imports");
    }

    // Recurse.
    for (_, v) in tbl.iter_mut() {
        inline_imports_in_value(file_path, v, stack)?;
    }

    Ok(())
}

fn load_value_inner(path: &Path, stack: &mut HashSet<PathBuf>) -> Result<Value> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !stack.insert(canonical.clone()) {
        return Err(Error::msg(format!(
            "config import cycle detected at {}",
            canonical.display()
        )));
    }

    let data = fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("failed to read config {}: {e}", path.display())))?;
    let mut value: Value = toml::from_str(&data)
        .map_err(|e| Error::msg(format!("TOML parse error in {}: {e}", path.display())))?;

    // Root-level single-parent extends (optional).
    let mut out = Value::Table(Default::default());
    if let Some(ext) = value.get("extends").and_then(Value::as_str) {
        let base_path = resolve_ref_path(path, ext);
        out = load_value_inner(&base_path, stack)?;
    }
    if let Some(tbl) = value.as_table_mut() {
        tbl.remove("extends");
    }

    // Root + section-level imports.
    inline_imports_in_value(path, &mut value, stack)?;

    merge_values(&mut out, value);

    stack.remove(&canonical);
    Ok(out)
}

pub fn load(path: &Path) -> Result<ConfigDoc> {
    let mut stack = HashSet::<PathBuf>::new();
    let value = load_value_inner(path, &mut stack)?;
    Ok(ConfigDoc {
        path: path.to_path_buf(),
        value,
    })
}
