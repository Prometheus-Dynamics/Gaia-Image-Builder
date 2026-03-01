use std::collections::BTreeMap;

use serde::Deserialize;
use toml::Value;

use crate::config::ConfigDoc;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct InputsConfig {
    options: BTreeMap<String, InputOptionConfig>,
    values: BTreeMap<String, Value>,
    resolved: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct InputOptionConfig {
    description: Option<String>,
    #[serde(rename = "type")]
    value_type: Option<InputType>,
    default: Option<Value>,
    choices: Vec<Value>,
    env: Option<String>,
    required: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum InputType {
    String,
    Bool,
    Int,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleOp {
    Truthy,
    Falsey,
    Eq,
    Neq,
}

#[derive(Debug, Clone)]
struct ParsedRule {
    key: String,
    op: RuleOp,
    rhs: Option<String>,
}

fn load_inputs_cfg(doc: &ConfigDoc) -> Result<InputsConfig> {
    Ok(doc.deserialize_path("inputs")?.unwrap_or_default())
}

fn parse_cli_overrides(raw: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for item in raw {
        let trimmed = item.trim();
        let Some((k, v)) = trimmed.split_once('=') else {
            return Err(Error::msg(format!(
                "invalid --set value '{}'; expected KEY=VALUE",
                item
            )));
        };
        let key = k.trim();
        if key.is_empty() {
            return Err(Error::msg(format!(
                "invalid --set value '{}'; key is empty",
                item
            )));
        }
        out.insert(key.to_string(), v.trim().to_string());
    }
    Ok(out)
}

fn value_type_of(v: &Value) -> Option<InputType> {
    match v {
        Value::String(_) => Some(InputType::String),
        Value::Boolean(_) => Some(InputType::Bool),
        Value::Integer(_) => Some(InputType::Int),
        Value::Float(_) => Some(InputType::Float),
        _ => None,
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_raw_value(raw: &str, expected: Option<InputType>) -> Result<Value> {
    let raw = raw.trim();
    match expected {
        Some(InputType::String) => Ok(Value::String(raw.to_string())),
        Some(InputType::Bool) => parse_bool(raw)
            .map(Value::Boolean)
            .ok_or_else(|| Error::msg(format!("expected bool value, got '{}'", raw))),
        Some(InputType::Int) => raw
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|e| Error::msg(format!("expected int value, got '{}': {}", raw, e))),
        Some(InputType::Float) => raw
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|e| Error::msg(format!("expected float value, got '{}': {}", raw, e))),
        None => {
            if let Some(v) = parse_bool(raw) {
                return Ok(Value::Boolean(v));
            }
            if let Ok(v) = raw.parse::<i64>() {
                return Ok(Value::Integer(v));
            }
            if let Ok(v) = raw.parse::<f64>() {
                return Ok(Value::Float(v));
            }
            Ok(Value::String(raw.to_string()))
        }
    }
}

fn value_repr(v: &Value) -> Result<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Boolean(b) => Ok(if *b { "true" } else { "false" }.to_string()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Float(f) => Ok(f.to_string()),
        _ => Err(Error::msg(format!(
            "unsupported input value type (only string/bool/int/float are supported), got {:?}",
            v.type_str()
        ))),
    }
}

fn ensure_primitive_value(path: &str, v: &Value) -> Result<()> {
    if value_type_of(v).is_none() {
        return Err(Error::msg(format!(
            "inputs value '{}' must be string/bool/int/float",
            path
        )));
    }
    Ok(())
}

fn ensure_value_type(path: &str, v: &Value, expected: InputType) -> Result<()> {
    ensure_primitive_value(path, v)?;
    let Some(actual) = value_type_of(v) else {
        return Err(Error::msg(format!(
            "inputs value '{}' has unsupported type",
            path
        )));
    };
    if actual != expected {
        return Err(Error::msg(format!(
            "inputs value '{}' has type {:?}, expected {:?}",
            path, actual, expected
        )));
    }
    Ok(())
}

fn inferred_option_type(opt: &InputOptionConfig, cfg_values: Option<&Value>) -> Option<InputType> {
    if let Some(t) = opt.value_type {
        return Some(t);
    }
    if let Some(v) = opt.default.as_ref()
        && let Some(t) = value_type_of(v)
    {
        return Some(t);
    }
    if let Some(v) = cfg_values
        && let Some(t) = value_type_of(v)
    {
        return Some(t);
    }
    opt.choices.first().and_then(value_type_of)
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Integer(x), Value::Float(y)) => (*x as f64) == *y,
        (Value::Float(x), Value::Integer(y)) => *x == (*y as f64),
        _ => a == b,
    }
}

fn validate_option_constraints(key: &str, opt: &InputOptionConfig, value: &Value) -> Result<()> {
    ensure_primitive_value(&format!("inputs.options.{}", key), value)?;

    if !opt.choices.is_empty() {
        for (idx, choice) in opt.choices.iter().enumerate() {
            ensure_primitive_value(&format!("inputs.options.{}.choices[{}]", key, idx), choice)?;
        }
        let found = opt.choices.iter().any(|choice| values_equal(choice, value));
        if !found {
            let choices = opt
                .choices
                .iter()
                .map(value_repr)
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            return Err(Error::msg(format!(
                "input '{}' value '{}' is not in allowed choices [{}]",
                key,
                value_repr(value)?,
                choices
            )));
        }
    }

    Ok(())
}

fn resolve_values_with(
    doc: &ConfigDoc,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Value>> {
    let cfg = load_inputs_cfg(doc)?;
    let mut out = BTreeMap::<String, Value>::new();

    for (k, v) in &cfg.values {
        ensure_primitive_value(&format!("inputs.values.{}", k), v)?;
        out.insert(k.clone(), v.clone());
    }

    for (key, opt) in &cfg.options {
        if let Some(v) = opt.default.as_ref() {
            ensure_primitive_value(&format!("inputs.options.{}.default", key), v)?;
        }

        let expected_type = inferred_option_type(opt, out.get(key));
        let mut chosen: Option<Value> = None;

        if let Some(raw) = overrides.get(key) {
            chosen = Some(parse_raw_value(raw, expected_type)?);
        } else if let Some(env_name) = opt.env.as_deref().map(str::trim).filter(|s| !s.is_empty())
            && let Ok(raw) = std::env::var(env_name)
        {
            chosen = Some(parse_raw_value(&raw, expected_type)?);
        } else if let Some(v) = out.get(key) {
            chosen = Some(v.clone());
        } else if let Some(v) = opt.default.clone() {
            chosen = Some(v);
        }

        if let Some(v) = chosen {
            if let Some(t) = expected_type {
                ensure_value_type(&format!("inputs.options.{}", key), &v, t)?;
            } else {
                ensure_primitive_value(&format!("inputs.options.{}", key), &v)?;
            }
            validate_option_constraints(key, opt, &v)?;
            out.insert(key.clone(), v);
        } else if opt.required {
            return Err(Error::msg(format!(
                "required input '{}' is not set (no --set override, env, inputs.values, or default)",
                key
            )));
        }
    }

    for (key, raw) in overrides {
        if out.contains_key(key) {
            continue;
        }
        out.insert(key.clone(), parse_raw_value(raw, None)?);
    }

    Ok(out)
}

fn write_resolved(doc: &mut ConfigDoc, values: BTreeMap<String, Value>) -> Result<()> {
    let Some(root) = doc.value.as_table_mut() else {
        return Err(Error::msg("config root must be a table"));
    };

    let inputs_value = root
        .entry("inputs".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let Some(inputs_tbl) = inputs_value.as_table_mut() else {
        return Err(Error::msg("inputs must be a table"));
    };

    let mut resolved_tbl = toml::value::Table::new();
    for (k, v) in values {
        resolved_tbl.insert(k, v);
    }
    inputs_tbl.insert("resolved".to_string(), Value::Table(resolved_tbl));
    Ok(())
}

pub fn apply_cli_overrides(doc: &mut ConfigDoc, raw_overrides: &[String]) -> Result<()> {
    let overrides = parse_cli_overrides(raw_overrides)?;
    let resolved = resolve_values_with(doc, &overrides)?;
    write_resolved(doc, resolved)
}

pub fn resolved_values(doc: &ConfigDoc) -> Result<BTreeMap<String, Value>> {
    let cfg = load_inputs_cfg(doc)?;
    if !cfg.resolved.is_empty() {
        for (k, v) in &cfg.resolved {
            ensure_primitive_value(&format!("inputs.resolved.{}", k), v)?;
        }
        return Ok(cfg.resolved);
    }
    resolve_values_with(doc, &BTreeMap::new())
}

pub fn value_as_env_string(v: &Value) -> Result<String> {
    value_repr(v)
}

pub fn env_key_for_input(key: &str) -> String {
    let mut out = String::from("GAIA_INPUT_");
    for ch in key.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    out
}

pub fn inject_env_vars(
    doc: &ConfigDoc,
    envs: &mut BTreeMap<String, String>,
    prefix: Option<&str>,
) -> Result<()> {
    let resolved = resolved_values(doc)?;
    let prefix = prefix.unwrap_or("GAIA_INPUT_");
    for (k, v) in resolved {
        let mut env_key = String::from(prefix);
        for ch in k.chars() {
            if ch.is_ascii_alphanumeric() {
                env_key.push(ch.to_ascii_uppercase());
            } else {
                env_key.push('_');
            }
        }
        envs.insert(env_key, value_as_env_string(&v)?);
    }
    Ok(())
}

fn parse_rule(raw: &str) -> Result<ParsedRule> {
    let expr = raw.trim();
    if expr.is_empty() {
        return Err(Error::msg("empty condition rule"));
    }

    if let Some((k, v)) = expr.split_once("!=") {
        let key = k.trim();
        let rhs = v.trim();
        if key.is_empty() || rhs.is_empty() {
            return Err(Error::msg(format!(
                "invalid condition '{}'; expected key!=value",
                raw
            )));
        }
        return Ok(ParsedRule {
            key: key.to_string(),
            op: RuleOp::Neq,
            rhs: Some(rhs.to_string()),
        });
    }

    if let Some((k, v)) = expr.split_once('=') {
        let key = k.trim();
        let rhs = v.trim();
        if key.is_empty() || rhs.is_empty() {
            return Err(Error::msg(format!(
                "invalid condition '{}'; expected key=value",
                raw
            )));
        }
        return Ok(ParsedRule {
            key: key.to_string(),
            op: RuleOp::Eq,
            rhs: Some(rhs.to_string()),
        });
    }

    if let Some(rest) = expr.strip_prefix('!') {
        let key = rest.trim();
        if key.is_empty() {
            return Err(Error::msg(format!(
                "invalid condition '{}'; expected !key",
                raw
            )));
        }
        return Ok(ParsedRule {
            key: key.to_string(),
            op: RuleOp::Falsey,
            rhs: None,
        });
    }

    Ok(ParsedRule {
        key: expr.to_string(),
        op: RuleOp::Truthy,
        rhs: None,
    })
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => {
            let s = s.trim().to_ascii_lowercase();
            !s.is_empty() && !matches!(s.as_str(), "0" | "false" | "no" | "off")
        }
        _ => false,
    }
}

fn parse_rule_rhs(rhs: &str, expected: Option<InputType>) -> Result<Value> {
    parse_raw_value(rhs, expected)
}

fn eval_rule(
    rule: &ParsedRule,
    values: &BTreeMap<String, Value>,
    options: &BTreeMap<String, InputOptionConfig>,
) -> Result<bool> {
    let actual = values.get(&rule.key);
    match rule.op {
        RuleOp::Truthy => Ok(actual.is_some_and(is_truthy)),
        RuleOp::Falsey => Ok(!actual.is_some_and(is_truthy)),
        RuleOp::Eq | RuleOp::Neq => {
            let Some(rhs) = rule.rhs.as_deref() else {
                return Err(Error::msg("condition comparator missing rhs value"));
            };
            let expected_type = options
                .get(&rule.key)
                .and_then(|o| o.value_type)
                .or_else(|| actual.and_then(value_type_of));
            let expected = parse_rule_rhs(rhs, expected_type)?;
            let eq = actual.is_some_and(|a| values_equal(a, &expected));
            if rule.op == RuleOp::Eq {
                Ok(eq)
            } else {
                Ok(!eq)
            }
        }
    }
}

pub fn conditions_match(
    doc: &ConfigDoc,
    enabled_if: &[String],
    disabled_if: &[String],
) -> Result<bool> {
    let values = resolved_values(doc)?;
    let options = load_inputs_cfg(doc)?.options;

    for cond in enabled_if {
        let cond = cond.trim();
        if cond.is_empty() {
            continue;
        }
        let rule = parse_rule(cond)?;
        if !eval_rule(&rule, &values, &options)? {
            return Ok(false);
        }
    }

    for cond in disabled_if {
        let cond = cond.trim();
        if cond.is_empty() {
            continue;
        }
        let rule = parse_rule(cond)?;
        if eval_rule(&rule, &values, &options)? {
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(src: &str) -> ConfigDoc {
        ConfigDoc {
            path: "inline.toml".into(),
            value: toml::from_str(src).expect("valid toml"),
        }
    }

    #[test]
    fn apply_cli_overrides_sets_resolved_values() {
        let mut doc = make_doc(
            r#"
[inputs.options.pv_mode]
type = "string"
default = "release"
choices = ["release", "repo", "local"]

[inputs.options.use_driver]
type = "bool"
default = false
"#,
        );

        apply_cli_overrides(
            &mut doc,
            &[
                "pv_mode=repo".to_string(),
                "use_driver=true".to_string(),
                "workers=4".to_string(),
            ],
        )
        .expect("apply overrides");

        let resolved = resolved_values(&doc).expect("resolved");
        assert_eq!(
            resolved.get("pv_mode").and_then(Value::as_str),
            Some("repo")
        );
        assert_eq!(
            resolved.get("use_driver").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(resolved.get("workers").and_then(Value::as_integer), Some(4));
    }

    #[test]
    fn conditions_match_supports_truthy_eq_and_neq() {
        let mut doc = make_doc(
            r#"
[inputs.values]
pv_mode = "release"
use_driver = true
"#,
        );
        apply_cli_overrides(&mut doc, &[]).expect("resolve defaults");

        assert!(conditions_match(&doc, &["pv_mode=release".into()], &[]).expect("cond"));
        assert!(conditions_match(&doc, &["use_driver".into()], &[]).expect("cond"));
        assert!(conditions_match(&doc, &["pv_mode!=repo".into()], &[]).expect("cond"));
        assert!(!conditions_match(&doc, &["!use_driver".into()], &[]).expect("cond"));
    }

    #[test]
    fn apply_cli_overrides_rejects_bad_set_syntax() {
        let mut doc = make_doc("");
        let err = apply_cli_overrides(&mut doc, &["badvalue".into()]).expect_err("must fail");
        assert!(err.to_string().contains("expected KEY=VALUE"));
    }
}
