//! Every port type round-trips through Daedalus values (fed into a graph,
//! through a node, taken back out) and through its serialized value form.

mod common;

use common::*;
use daedalus::prelude::*;
use daedalus_cpu::CpuBackend;
use gaia_types::*;
use std::sync::Arc;

macro_rules! passthrough {
    ($($fn_name:ident, $record:ident, $ty:ty, $id:literal;)*) => {
        $(
            #[node(id = $id, inputs(x), outputs(y))]
            fn $fn_name(x: $ty) -> $ty {
                x
            }
        )*
        const NODES: &[&NodeRecord] = &[$(&$record),*];
        /// (node id, port type key)
        fn ports() -> Vec<(&'static str, String)> {
            vec![$(($id, <$ty as daedalus_types::Typed>::KEY.as_str().to_string())),*]
        }
    };
}

passthrough! {
    pass_object, PASS_OBJECT_NODE, StoreRef, "test.object";
    pass_artifact, PASS_ARTIFACT_NODE, Artifact, "test.artifact";
    pass_tree, PASS_TREE_NODE, Tree, "test.tree";
    pass_entry, PASS_ENTRY_NODE, FileEntry, "test.file-entry";
    pass_image, PASS_IMAGE_NODE, Image, "test.image";
    pass_partition, PASS_PARTITION_NODE, Partition, "test.partition";
    pass_target, PASS_TARGET_NODE, Target, "test.target";
    pass_size, PASS_SIZE_NODE, Size, "test.size";
}

/// One graph: for each type, host `<n>_in` -> passthrough -> host `<n>_out`.
fn load() -> LoadedHost {
    let mut port_types = Vec::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (index, (id, key)) in ports().iter().enumerate() {
        port_types.push(format!(
            r#""p{index}_in": "{key}", "p{index}_out": "{key}""#
        ));
        nodes.push(format!(r#"{{"id": "{id}"}}"#));
        let node = index + 1;
        edges.push(format!(
            r#"{{"from": {{"node": 0, "port": "p{index}_in"}}, "to": {{"node": {node}, "port": "x"}}}}"#
        ));
        edges.push(format!(
            r#"{{"from": {{"node": {node}, "port": "y"}}, "to": {{"node": 0, "port": "p{index}_out"}}}}"#
        ));
    }
    let text = format!(
        r#"{{"format": "daedalus.graph", "schema_version": 1, "graph": {{
            "nodes": [{{"id": "host", "kind": "host_bridge", "port_types": {{{ports}}}}}, {nodes}],
            "edges": [{edges}]}}}}"#,
        ports = port_types.join(", "),
        nodes = nodes.join(", "),
        edges = edges.join(", "),
    );
    let graph = GraphDoc::parse(&text).expect("the graph parses").graph;
    let mut registry = Registry::new();
    registry.register_all(NODES).expect("registers");
    let planned = plan(&graph, &registry, &[&CpuBackend], &Config::default()).expect("plans");
    LoadedHost::load(planned, LoadOptions::default()).expect("loads")
}

fn through<T: Clone + PartialEq + std::fmt::Debug + Send + Sync + 'static>(
    loaded: &mut LoadedHost,
    index: usize,
    value: T,
) {
    let host = loaded.host();
    host.feed_typed(&format!("p{index}_in"), Arc::new(value.clone()))
        .expect("feed");
    host.step().expect("tick");
    let back = host
        .take::<T>(&format!("p{index}_out"))
        .expect("take")
        .expect("a value");
    assert_eq!(back, value);
}

#[test]
fn every_type_round_trips_through_a_daedalus_graph() {
    let mut loaded = load();
    through(&mut loaded, 0, object(ObjectKind::Tree, 9));
    through(&mut loaded, 1, artifact());
    through(&mut loaded, 2, tree());
    through(&mut loaded, 3, entry());
    through(&mut loaded, 4, image());
    through(&mut loaded, 5, partition());
    through(&mut loaded, 6, target());
    // A `Copy` value of 16 bytes or less travels in place, and is taken as
    // one.
    let host = loaded.host();
    host.feed_typed("p7_in", Arc::new(Size::kib(4)))
        .expect("feed");
    host.step().expect("tick");
    let size = host
        .take_copy::<Size>("p7_out")
        .expect("take")
        .expect("a value");
    assert_eq!(size, Size::kib(4));
    assert_eq!(loaded.fault_reports().count(), 0);
}

#[test]
fn a_port_of_one_type_refuses_another() {
    let text = r#"{"format": "daedalus.graph", "schema_version": 1, "graph": {
        "nodes": [{"id": "host", "kind": "host_bridge", "port_types": {"in": "gaia:artifact", "out": "gaia:image"}},
                  {"id": "test.artifact"}],
        "edges": [{"from": {"node": 0, "port": "in"}, "to": {"node": 1, "port": "x"}},
                  {"from": {"node": 1, "port": "y"}, "to": {"node": 0, "port": "out"}}]}}"#;
    let graph = GraphDoc::parse(text).expect("parses").graph;
    let mut registry = Registry::new();
    registry.register_all(NODES).expect("registers");
    let error = match plan(&graph, &registry, &[&CpuBackend], &Config::default()) {
        Ok(_) => panic!("a gaia:artifact output must not feed a gaia:image port"),
        Err(error) => error,
    };
    println!("refused: {error}");
}

/// The serialized value form of each type, recorded.
#[test]
fn every_type_serializes_to_a_recorded_value_and_back() {
    fn check<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: T,
        recorded: &str,
    ) {
        let text = serde_json::to_string(&value).expect("serializes");
        assert_eq!(text, recorded);
        assert_eq!(serde_json::from_str::<T>(&text).expect("reads back"), value);
    }
    let d = |byte: &str| format!("sha256:{}", byte.repeat(32));
    check(
        artifact(),
        &format!(
            r#"{{"kind":"uf2","object":{{"kind":"file","digest":"{}"}}}}"#,
            d("01")
        ),
    );
    check(
        tree(),
        &format!(
            r#"{{"object":{{"kind":"tree","digest":"{}"}},"entries":[{{"path":"usr/bin/sudo","mode":2541,"uid":0,"gid":0}}]}}"#,
            d("02")
        ),
    );
    check(
        image(),
        &format!(
            r#"{{"kind":"disk","object":{{"kind":"file","digest":"{}"}},"size":1073741824,"partitions":[{{"name":"rootfs-a","role":"rootfs","table_type":"0fc63daf-8483-4772-8e79-3d69d8477de4","start":16777216,"size":536870912,"content":{{"kind":"file","digest":"{}"}}}}]}}"#,
            d("04"),
            d("03")
        ),
    );
    check(
        target(),
        r#"{"arch":"aarch64","cpu":"cortex-a76","os":"linux"}"#,
    );
    check(Size::mib(1), "1048576");
}

#[test]
fn open_kinds_take_any_name_and_known_names_stay_known() {
    assert_eq!(ArtifactKind::parse("binary"), ArtifactKind::Binary);
    assert_eq!(
        ArtifactKind::parse("ipk"),
        ArtifactKind::Other("ipk".into())
    );
    let read: ArtifactKind = serde_json::from_str(r#""firmware""#).expect("reads");
    assert_eq!(read, ArtifactKind::Firmware);
    assert_eq!(ImageKind::parse("uf2").to_string(), "uf2");
    assert_eq!(
        Target::from_triple("thumbv8m.main-none-eabihf"),
        Target {
            arch: Arch::Arm,
            cpu: None,
            os: Os::None
        }
    );
    assert_eq!(
        Target::from_triple("riscv64gc-unknown-linux-gnu").arch,
        Arch::RiscV64
    );
    assert_eq!(Size::mib(16).to_string(), "16 MiB");
    assert_eq!(Size::bytes(1000).to_string(), "1000 B");
}

#[test]
fn a_store_reference_maps_to_the_store_and_back() {
    let object = gaia_store::ObjectRef {
        kind: gaia_store::Kind::Tree,
        digest: gaia_store::Digest::of_bytes(b"x"),
    };
    let port = StoreRef::from(object);
    assert_eq!(port.object_ref(), Some(object));
    let bad = StoreRef {
        kind: ObjectKind::File,
        digest: "nope".into(),
    };
    assert_eq!(bad.object_ref(), None);
}
