use gaia_types::*;

pub fn object(kind: ObjectKind, byte: u8) -> StoreRef {
    StoreRef {
        kind,
        digest: format!("sha256:{}", format!("{byte:02x}").repeat(32)),
    }
}

pub fn artifact() -> Artifact {
    Artifact {
        kind: ArtifactKind::Other("uf2".into()),
        object: object(ObjectKind::File, 1),
    }
}

pub fn entry() -> FileEntry {
    FileEntry {
        path: "usr/bin/sudo".into(),
        mode: 0o4755,
        uid: 0,
        gid: 0,
    }
}

pub fn tree() -> Tree {
    Tree {
        object: object(ObjectKind::Tree, 2),
        entries: vec![entry()],
    }
}

pub fn partition() -> Partition {
    Partition {
        name: "rootfs-a".into(),
        role: PartitionRole::Rootfs,
        table_type: "0fc63daf-8483-4772-8e79-3d69d8477de4".into(),
        start: Size::mib(16),
        size: Size::mib(512),
        content: Some(object(ObjectKind::File, 3)),
    }
}

pub fn image() -> Image {
    Image {
        kind: ImageKind::Disk,
        object: object(ObjectKind::File, 4),
        size: Size::mib(1024),
        partitions: vec![partition()],
    }
}

pub fn target() -> Target {
    Target {
        cpu: Some("cortex-a76".into()),
        ..Target::from_triple("aarch64-unknown-linux-musl")
    }
}
