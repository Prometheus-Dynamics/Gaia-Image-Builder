//! Keys and type structures are stable: each is recorded here, so a change
//! to a key or to any field shows up as a failing test, and is made on
//! purpose.

use daedalus_types::Typed;
use gaia_types::*;
use sha2::{Digest as _, Sha256};

/// SHA-256 of the type's canonical structure, first 16 hex digits.
fn shape<T: Typed>() -> String {
    let mut hasher = Sha256::new();
    T::EXPR.encode(&mut |bytes| hasher.update(bytes));
    let digest = hasher.finalize();
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn row<T: Typed>() -> (String, String) {
    (T::KEY.as_str().to_string(), shape::<T>())
}

#[test]
fn keys_and_structures_are_recorded() {
    let rows = [
        row::<StoreRef>(),
        row::<Artifact>(),
        row::<Tree>(),
        row::<FileEntry>(),
        row::<Image>(),
        row::<Partition>(),
        row::<Target>(),
        row::<Size>(),
        row::<Layer>(),
        row::<Placement>(),
        row::<Account>(),
        row::<AccountGroup>(),
        row::<Control>(),
        row::<DiskLayout>(),
        row::<DiskTable>(),
        row::<DiskPartition>(),
        row::<RawBlob>(),
        row::<OsConfig>(),
        row::<ConfigFragment>(),
        row::<BuildrootHost>(),
        row::<SourceOverride>(),
        row::<ObjectKind>(),
        row::<ArtifactKind>(),
        row::<ImageKind>(),
        row::<PartitionRole>(),
        row::<TableScheme>(),
        row::<Arch>(),
        row::<Os>(),
    ];
    for (key, shape) in &rows {
        println!("{key:22} {shape}");
    }
    let keys: Vec<&str> = rows.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, KEYS);
    let recorded = RECORDED
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let actual = rows.iter().map(|(key, shape)| format!("{key} {shape}"));
    assert_eq!(recorded.collect::<Vec<_>>(), actual.collect::<Vec<_>>());
}

const RECORDED: &str = "
    gaia:object 24b7c35dd7df2754
    gaia:artifact 19553e03b8145932
    gaia:tree 6cf3d3a8bf4db83b
    gaia:file-entry 85e916f315564f6c
    gaia:image f89d924627d97a9a
    gaia:partition 5221ef72e84bd693
    gaia:target 6193f2e1b43ec840
    gaia:size 67ebbd370daa02ba
    gaia:layer 06bab8ddb968a9a7
    gaia:placement b31304eccf6cdec8
    gaia:account db1105887abe30dc
    gaia:account-group cfc99d936f919de5
    gaia:package-control 19dc0ba00ed8499e
    gaia:disk-layout aece9583c0839b54
    gaia:disk-table bdf819301870eb86
    gaia:disk-partition a9f281ead6a6e53c
    gaia:raw-blob a34597c3a0a074cb
    gaia:os-config ccd0f4212d2312c5
    gaia:buildroot-fragment 2847827726c91ae9
    gaia:buildroot-host 236dfb6e85f8d0d4
    gaia:source-override d72cbdcff1dd0ea1
    gaia:object-kind 8ea567075e428736
    gaia:artifact-kind 1ce777e4bb50c505
    gaia:image-kind a54f2ea6cb3fe288
    gaia:partition-role 99251c19cf0fd1aa
    gaia:table-scheme 1cd54b978d3a0915
    gaia:arch 324560ad7bc366a7
    gaia:os 24c133b1c2ecf3b9
";
