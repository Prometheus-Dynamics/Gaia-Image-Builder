//! A recipe file turned into a Daedalus group, and the errors it gives.

use daedalus::layers::graph::{GraphBody, GroupDoc, Lit, NodeDoc, NodeKindDoc, StagePortDoc};
use daedalus::layers::planner::pipeline::solve;
use daedalus::prelude::*;
use gaia_recipe::{Recipe, Recipes};
use std::path::Path;

#[node(id = "test.scale", inputs(x, factor), outputs(y))]
fn scale(x: i64, factor: i64) -> i64 {
    x * factor
}

#[node(id = "test.name", inputs(value, prefix), outputs(text))]
fn name(value: i64, prefix: String) -> String {
    format!("{prefix}{value}")
}

const RECIPE: &str = r#"
version = 1
name = "two-step"
description = "Scales a number, then names it."

[stages]
scale = "test.scale"
name = "test.name"

[[connects]]
from = "scale.y"
to = "name.value"

[outputs]
named = "name.text"

[parameters.factor]
type = "int"
default = 3
sets = ["scale.factor"]

[parameters.prefix]
type = "string"
sets = ["name.prefix"]
description = "Goes in front of the number."

[consts]
"scale.x" = 7
"#;

fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("write");
    path
}

fn port(stage: &str, port: &str) -> StagePortDoc {
    StagePortDoc {
        stage: stage.into(),
        port: port.into(),
    }
}

#[test]
fn a_recipe_with_two_nodes_expands_into_the_expected_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let recipes = dir.path().join("recipes");
    std::fs::create_dir(&recipes).expect("mkdir");
    write(&recipes, "two-step.toml", RECIPE);
    let product = dir.path().join("product.toml");
    let loaded = Recipes::load_dir(&recipes).expect("loads");
    let recipe = loaded.get("two-step", &product).expect("found");

    let values: toml::Table = toml::from_str(r#"prefix = "n=""#).expect("toml");
    let group = recipe.group(&values, &product).expect("group");
    let mut expected = GroupDoc {
        id: "two-step".into(),
        stages: [
            ("name".into(), "test.name".into()),
            ("scale".into(), "test.scale".into()),
        ]
        .into(),
        connects: vec![(port("scale", "y"), port("name", "value"))],
        inputs: Default::default(),
        outputs: [("named".into(), port("name", "text"))].into(),
        consts: Vec::new(),
    };
    expected.consts = vec![
        daedalus::layers::graph::StageConstDoc {
            at: port("scale", "x"),
            value: Lit::Int(7),
        },
        daedalus::layers::graph::StageConstDoc {
            at: port("scale", "factor"),
            value: Lit::Int(3),
        },
        daedalus::layers::graph::StageConstDoc {
            at: port("name", "prefix"),
            value: Lit::Str("n=".into()),
        },
    ];
    assert_eq!(group, expected);
    println!("{}", serde_json::to_string_pretty(&group).expect("json"));

    // Daedalus expands it: two nodes, labelled by the group, the edge
    // between them, every constant in place.
    let mut registry = Registry::new();
    registry
        .register_all(&[&SCALE_NODE, &NAME_NODE])
        .expect("registers");
    let mut node = NodeDoc::new("two-step");
    node.kind = NodeKindDoc::Group;
    let body = GraphBody {
        nodes: vec![node],
        edges: Vec::new(),
        groups: vec![group],
    };
    let graph = solve(&body, &registry, &Config::default()).expect("solves");
    let mut seen: Vec<String> = graph
        .nodes
        .iter()
        .map(|n| {
            let consts: Vec<String> = n
                .doc
                .consts
                .iter()
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect();
            format!(
                "{} {} [{}]",
                n.doc.label.as_deref().unwrap_or("-"),
                n.doc.id,
                consts.join(", ")
            )
        })
        .collect();
    seen.sort();
    for line in &seen {
        println!("{line}");
    }
    assert_eq!(
        seen,
        [
            r#"two-step::name test.name [prefix=Str("n=")]"#,
            "two-step::scale test.scale [factor=Int(3), x=Int(7)]",
        ]
    );
    assert_eq!(graph.edges.len(), 1);
    let edge = &graph.edges[0];
    let id = |i: u32| graph.nodes[i as usize].doc.id.as_str();
    println!(
        "edge {}.{} -> {}.{}",
        id(edge.from.node),
        edge.from.port,
        id(edge.to.node),
        edge.to.port
    );
    assert_eq!(
        (id(edge.from.node), edge.from.port.as_str()),
        ("test.scale", "y")
    );
    assert_eq!(
        (id(edge.to.node), edge.to.port.as_str()),
        ("test.name", "value")
    );

    // A product's value overrides the default.
    let values: toml::Table = toml::from_str("prefix = \"n=\"\nfactor = 5").expect("toml");
    let group = recipe.group(&values, &product).expect("group");
    assert!(
        group
            .consts
            .iter()
            .any(|c| c.at == port("scale", "factor") && c.value == Lit::Int(5))
    );
}

#[test]
fn a_wrong_parameter_type_fails_with_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let recipe = Recipe::read(write(dir.path(), "two-step.toml", RECIPE)).expect("reads");
    let product = dir.path().join("products/demo.toml");
    let check = |values: &str, want: &str| {
        let values: toml::Table = toml::from_str(values).expect("toml");
        let error = recipe
            .group(&values, &product)
            .expect_err("fails")
            .to_string();
        println!("{error}");
        assert!(error.contains(want), "{error}");
        assert!(
            error.contains("products/demo.toml") && error.contains("recipe two-step"),
            "{error}"
        );
    };
    check(
        "prefix = \"n=\"\nfactor = \"fast\"",
        "parameters.factor: wants an int, got the string \"fast\"",
    );
    check(
        "prefix = 4",
        "parameters.prefix: wants a string, got the int 4",
    );
    check(
        "",
        "parameters.prefix: missing; it has no default and wants a string",
    );
    check(
        "prefix = \"n\"\nfacter = 2",
        "parameters.facter: not a parameter of this recipe; did you mean factor?",
    );
}

#[test]
fn a_bad_recipe_file_names_the_file_and_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        (
            RECIPE.replace("default = 3", "default = \"three\""),
            "recipe two-step: parameters.factor.default: wants an int, got the string \"three\"",
        ),
        (
            RECIPE.replace("to = \"name.value\"", "to = \"label.value\""),
            "connects[0].to: no stage \"label\"; the stages are name, scale",
        ),
        (
            RECIPE.replace("[outputs]", "[outptus]"),
            "unknown field `outptus`",
        ),
        (
            RECIPE.replace("version = 1", "version = 2"),
            "version: 2 is not a version of this format",
        ),
    ];
    for (text, want) in cases {
        let path = write(dir.path(), "bad.toml", &text);
        let error = Recipe::read(&path).expect_err("fails").to_string();
        println!("{error}");
        assert!(error.contains(want), "{error}");
        assert!(error.starts_with(&path.display().to_string()), "{error}");
    }
    // Two recipes with one name.
    let recipes = dir.path().join("recipes");
    std::fs::create_dir(&recipes).expect("mkdir");
    write(&recipes, "a.toml", RECIPE);
    write(&recipes, "b.toml", RECIPE);
    let error = Recipes::load_dir(&recipes)
        .expect_err("duplicate")
        .to_string();
    println!("{error}");
    assert!(
        error.contains("b.toml") && error.contains("also the name of") && error.contains("a.toml")
    );
}
