use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use gaia_image_builder::checkpoints::{
    self, CheckpointManifest, CheckpointManifestTarget, CheckpointTrustMode,
};
use gaia_image_builder::config::ConfigDoc;

struct DockerGuard {
    name: String,
}

impl Drop for DockerGuard {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .arg("rm")
            .arg("-f")
            .arg(&self.name)
            .status();
    }
}

fn have_bin(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_output(cmd: &mut Command) -> Output {
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run {:?}: {e}", cmd))
}

fn run_ok(cmd: &mut Command) {
    let out = run_output(cmd);
    if out.status.success() {
        return;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    panic!(
        "command failed {:?}\nstatus={}\nstdout={}\nstderr={}",
        cmd, out.status, stdout, stderr
    );
}

fn run_stdout(cmd: &mut Command) -> String {
    let out = run_output(cmd);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        panic!("command failed {:?}: {}", cmd, stderr.trim());
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind free port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn wait_for(timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        thread::sleep(Duration::from_millis(500));
    }
    panic!("timed out waiting for condition");
}

fn write_payload_archive(root: &Path) -> PathBuf {
    let payload_dir = root.join("payload").join("buildroot_out_dir");
    fs::create_dir_all(&payload_dir).expect("payload dir");
    fs::write(payload_dir.join("marker.txt"), "fixture").expect("payload marker");
    let archive = root.join("payload.tar");
    run_ok(
        Command::new("tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(root)
            .arg("payload"),
    );
    archive
}

fn write_manifest(root: &Path, id: &str, anchor_task: &str, fingerprint: &str) -> PathBuf {
    let manifest = CheckpointManifest {
        version: 1,
        id: id.to_string(),
        anchor_task: anchor_task.to_string(),
        fingerprint: fingerprint.to_string(),
        lineage: {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(anchor_task.as_bytes());
            h.update(b"\n");
            h.update(fingerprint.as_bytes());
            hex::encode(h.finalize())
        },
        created_at: chrono::Utc::now().to_rfc3339(),
        trust_mode: CheckpointTrustMode::Verify,
        fingerprint_inputs: Default::default(),
        targets: vec![CheckpointManifestTarget {
            name: "buildroot_out_dir".to_string(),
            payload_rel: "buildroot_out_dir".to_string(),
        }],
    };
    let path = root.join("manifest.json");
    let body = serde_json::to_string_pretty(&manifest).expect("manifest encode");
    fs::write(&path, body).expect("manifest write");
    path
}

fn doc_with_s3_backend(tmp: &Path, endpoint_url: &str, bucket: &str) -> ConfigDoc {
    let raw = format!(
        r#"
[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
fingerprint_from = ["buildroot.version"]
backend = "s3:cache"

[checkpoints.backends.s3.cache]
bucket = "{}"
endpoint_url = "{}"
prefix = "gaia"
aws_access_key_id_env = "GAIA_TEST_AWS_ACCESS_KEY_ID"
aws_secret_access_key_env = "GAIA_TEST_AWS_SECRET_ACCESS_KEY"
"#,
        tmp.display(),
        bucket,
        endpoint_url
    );
    let value: toml::Value = toml::from_str(&raw).expect("parse toml");
    ConfigDoc {
        path: tmp.join("build.toml"),
        value,
    }
}

fn doc_with_ssh_backend(tmp: &Path, target: &str, port: u16, identity_file: &Path) -> ConfigDoc {
    let raw = format!(
        r#"
[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"

[checkpoints]
enabled = true

[[checkpoints.points]]
id = "base"
anchor_task = "buildroot.build"
fingerprint_from = ["buildroot.version"]
backend = "ssh:cache"

[checkpoints.backends.ssh.cache]
target = "{}"
port = {}
identity_file = "{}"
strict_host_key_checking = false
"#,
        tmp.display(),
        target,
        port,
        identity_file.display()
    );
    let value: toml::Value = toml::from_str(&raw).expect("parse toml");
    ConfigDoc {
        path: tmp.join("build.toml"),
        value,
    }
}

#[test]
#[ignore = "requires docker, aws cli, and tar"]
fn checkpoints_remote_s3_minio_list_fixture() {
    if !have_bin("docker") || !have_bin("aws") || !have_bin("tar") {
        eprintln!("skip: missing docker/aws/tar");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let name = format!("gaia-minio-{}-{}", std::process::id(), free_port());
    let port = free_port();
    let _guard = DockerGuard { name: name.clone() };

    run_ok(
        Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--rm")
            .arg("--name")
            .arg(&name)
            .arg("-p")
            .arg(format!("{port}:9000"))
            .arg("-e")
            .arg("MINIO_ROOT_USER=minio")
            .arg("-e")
            .arg("MINIO_ROOT_PASSWORD=miniosecret")
            .arg("quay.io/minio/minio")
            .arg("server")
            .arg("/data"),
    );

    let endpoint = format!("http://127.0.0.1:{port}");
    wait_for(Duration::from_secs(30), || {
        let out = run_output(
            Command::new("aws")
                .arg("s3api")
                .arg("list-buckets")
                .arg("--endpoint-url")
                .arg(&endpoint)
                .env("AWS_ACCESS_KEY_ID", "minio")
                .env("AWS_SECRET_ACCESS_KEY", "miniosecret")
                .env("AWS_DEFAULT_REGION", "us-east-1"),
        );
        out.status.success()
    });

    let bucket = "gaia-checkpoints-test";
    run_ok(
        Command::new("aws")
            .arg("s3")
            .arg("mb")
            .arg(format!("s3://{bucket}"))
            .arg("--endpoint-url")
            .arg(&endpoint)
            .env("AWS_ACCESS_KEY_ID", "minio")
            .env("AWS_SECRET_ACCESS_KEY", "miniosecret")
            .env("AWS_DEFAULT_REGION", "us-east-1"),
    );

    unsafe {
        std::env::set_var("GAIA_TEST_AWS_ACCESS_KEY_ID", "minio");
        std::env::set_var("GAIA_TEST_AWS_SECRET_ACCESS_KEY", "miniosecret");
    }

    let doc = doc_with_s3_backend(tmp.path(), &endpoint, bucket);
    let status = checkpoints::status_for_doc(&doc).expect("status");
    let fingerprint = status.first().expect("checkpoint").fingerprint.clone();

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("fixture dir");
    let manifest = write_manifest(&fixture, "base", "buildroot.build", &fingerprint);
    let archive = write_payload_archive(&fixture);
    let remote_prefix = format!("s3://{bucket}/gaia/base/{fingerprint}");

    run_ok(
        Command::new("aws")
            .arg("s3")
            .arg("cp")
            .arg(&manifest)
            .arg(format!("{remote_prefix}/manifest.json"))
            .arg("--endpoint-url")
            .arg(&endpoint)
            .env("AWS_ACCESS_KEY_ID", "minio")
            .env("AWS_SECRET_ACCESS_KEY", "miniosecret")
            .env("AWS_DEFAULT_REGION", "us-east-1"),
    );
    run_ok(
        Command::new("aws")
            .arg("s3")
            .arg("cp")
            .arg(&archive)
            .arg(format!("{remote_prefix}/payload.tar"))
            .arg("--endpoint-url")
            .arg(&endpoint)
            .env("AWS_ACCESS_KEY_ID", "minio")
            .env("AWS_SECRET_ACCESS_KEY", "miniosecret")
            .env("AWS_DEFAULT_REGION", "us-east-1"),
    );

    let listed = checkpoints::list_for_doc(&doc, true, None).expect("list");
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0]
            .remote_fingerprints
            .iter()
            .any(|f| f == &fingerprint),
        "remote fingerprints missing current fingerprint: {:?}",
        listed[0].remote_fingerprints
    );
}

#[test]
#[ignore = "requires docker, ssh/scp/ssh-keygen, and tar"]
fn checkpoints_remote_ssh_list_fixture() {
    if !have_bin("docker")
        || !have_bin("ssh")
        || !have_bin("scp")
        || !have_bin("ssh-keygen")
        || !have_bin("tar")
    {
        eprintln!("skip: missing docker/ssh/scp/ssh-keygen/tar");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let key = tmp.path().join("id_ed25519");
    run_ok(
        Command::new("ssh-keygen")
            .arg("-q")
            .arg("-t")
            .arg("ed25519")
            .arg("-N")
            .arg("")
            .arg("-f")
            .arg(&key),
    );
    let pubkey = fs::read_to_string(key.with_extension("pub")).expect("read pubkey");

    let name = format!("gaia-ssh-{}-{}", std::process::id(), free_port());
    let port = free_port();
    let _guard = DockerGuard { name: name.clone() };
    run_ok(
        Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--rm")
            .arg("--name")
            .arg(&name)
            .arg("-p")
            .arg(format!("{port}:2222"))
            .arg("-e")
            .arg("PUID=1000")
            .arg("-e")
            .arg("PGID=1000")
            .arg("-e")
            .arg("TZ=UTC")
            .arg("-e")
            .arg("USER_NAME=gaia")
            .arg("-e")
            .arg(format!("PUBLIC_KEY={}", pubkey.trim()))
            .arg("-e")
            .arg("PASSWORD_ACCESS=false")
            .arg("lscr.io/linuxserver/openssh-server:latest"),
    );

    wait_for(Duration::from_secs(45), || {
        let out = run_output(
            Command::new("ssh")
                .arg("-p")
                .arg(port.to_string())
                .arg("-i")
                .arg(&key)
                .arg("-o")
                .arg("StrictHostKeyChecking=no")
                .arg("-o")
                .arg("UserKnownHostsFile=/dev/null")
                .arg("gaia@127.0.0.1")
                .arg("echo ok"),
        );
        out.status.success()
    });

    let home = run_stdout(
        Command::new("ssh")
            .arg("-p")
            .arg(port.to_string())
            .arg("-i")
            .arg(&key)
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("gaia@127.0.0.1")
            .arg("printf \"$HOME\""),
    );
    let remote_base = format!("{}/checkpoints", home.trim());
    let target = format!("gaia@127.0.0.1:{remote_base}");
    let doc = doc_with_ssh_backend(tmp.path(), &target, port, &key);
    let status = checkpoints::status_for_doc(&doc).expect("status");
    let fingerprint = status.first().expect("checkpoint").fingerprint.clone();

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("fixture dir");
    let manifest = write_manifest(&fixture, "base", "buildroot.build", &fingerprint);
    let archive = write_payload_archive(&fixture);

    let remote_dir = format!("{remote_base}/base/{fingerprint}");
    run_ok(
        Command::new("ssh")
            .arg("-p")
            .arg(port.to_string())
            .arg("-i")
            .arg(&key)
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("gaia@127.0.0.1")
            .arg(format!("mkdir -p '{}'", remote_dir)),
    );

    run_ok(
        Command::new("scp")
            .arg("-P")
            .arg(port.to_string())
            .arg("-i")
            .arg(&key)
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg(&manifest)
            .arg(format!("gaia@127.0.0.1:{remote_dir}/manifest.json")),
    );
    run_ok(
        Command::new("scp")
            .arg("-P")
            .arg(port.to_string())
            .arg("-i")
            .arg(&key)
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg(&archive)
            .arg(format!("gaia@127.0.0.1:{remote_dir}/payload.tar")),
    );

    let listed = checkpoints::list_for_doc(&doc, true, None).expect("list");
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0]
            .remote_fingerprints
            .iter()
            .any(|f| f == &fingerprint),
        "remote fingerprints missing current fingerprint: {:?}",
        listed[0].remote_fingerprints
    );
}
