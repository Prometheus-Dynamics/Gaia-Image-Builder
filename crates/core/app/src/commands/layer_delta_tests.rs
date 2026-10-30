use super::*;
use gaia_process::sha256_hex_bytes;
use gaia_testkit::TempDir;

fn zstd_missing() -> bool {
    Command::new("zstd").arg("--version").output().is_err()
}

/// Pseudo-random bytes that compress poorly on their own, so a delta
/// against a similar base is clearly smaller than the whole layer.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

/// A release dir with one tar per layer and its `layers.json`.
fn release(dir: &Path, layers: &[(&str, &[u8])]) -> String {
    fs::create_dir_all(dir).expect("release dir");
    let records: Vec<Value> = layers
        .iter()
        .map(|(name, bytes)| {
            fs::write(dir.join(format!("{name}.tar")), bytes).expect("tar");
            let sha256 = sha256_hex_bytes(bytes);
            json!({
                "name": name,
                "id": format!("erofs-{}", short(&sha256)),
                "tar": format!("{name}.tar"),
                "tar_sha256": sha256,
                "tar_size": bytes.len(),
            })
        })
        .collect();
    let order: Vec<&str> = layers.iter().map(|(name, _)| *name).collect();
    fs::write(
        dir.join(LAYERS_JSON),
        json!({ "schema": 1, "order": order, "layers": records }).to_string(),
    )
    .expect("layers.json");
    dir.display().to_string()
}

#[test]
fn unchanged_layers_need_nothing_and_changed_ones_get_a_verified_delta() {
    if zstd_missing() {
        return;
    }
    let scratch = TempDir::new("gaia-layer-delta");
    let base = noise(1, 64 * 1024);
    let app_v1 = noise(2, 256 * 1024);
    let mut app_v2 = app_v1.clone();
    app_v2[1000..1100].copy_from_slice(&noise(3, 100));
    app_v2.extend(noise(4, 512));
    let tools = noise(5, 8 * 1024);
    let v1 = release(&scratch.join("v1"), &[("base", &base), ("app", &app_v1)]);
    let v2 = release(
        &scratch.join("v2"),
        &[("base", &base), ("app", &app_v2), ("tools", &tools)],
    );
    let out = scratch.join("out");

    let document = write_deltas(&v2, &[v1], &out).expect("deltas");

    let layers = document["layers"].as_array().expect("layers");
    assert_eq!(layers[0]["status"], "unchanged");
    assert!(layers[0].get("full").is_none());
    assert!(!out.join("base.tar.zst").exists());

    let app = &layers[1];
    assert_eq!(app["status"], "changed");
    let delta = &app["deltas"][0];
    assert_eq!(delta["from_tar_sha256"], sha256_hex_bytes(&app_v1));
    assert_eq!(delta["from_id"], layers_id(&app_v1));
    let delta_size = delta["size"].as_u64().expect("size");
    assert!(
        delta_size * 20 < app["full"]["size"].as_u64().expect("full"),
        "{app}"
    );
    // The board's side: the delta against the old tar gives the new tar.
    let rebuilt = scratch.join("app.tar");
    let status = Command::new("zstd")
        .args(["-q", "-d"])
        .arg(format!("--long={}", delta["window_log"]))
        .arg(format!(
            "--patch-from={}",
            scratch.join("v1/app.tar").display()
        ))
        .arg(out.join(delta["file"].as_str().expect("file")))
        .arg("-o")
        .arg(&rebuilt)
        .status()
        .expect("zstd");
    assert!(status.success());
    assert_eq!(fs::read(&rebuilt).expect("rebuilt"), app_v2);

    let tools = &layers[2];
    assert_eq!(tools["status"], "new");
    assert_eq!(tools["deltas"], json!([]));
    assert!(out.join("tools.tar.zst").is_file());

    assert!(out.join(DELTAS_JSON).is_file());
    assert_eq!(
        fs::read(out.join(LAYERS_JSON)).expect("copy"),
        fs::read(scratch.join("v2").join(LAYERS_JSON)).expect("layers.json")
    );
    let text = summary(&document, &out.display().to_string());
    assert!(text.contains("layer base: unchanged"), "{text}");
    assert!(text.contains("layer app: 1 delta(s), smallest"), "{text}");
    assert!(
        text.contains("layer tools: new, whole layer only"),
        "{text}"
    );
}

fn layers_id(bytes: &[u8]) -> String {
    format!("erofs-{}", short(&sha256_hex_bytes(bytes)))
}

#[test]
fn an_unrelated_base_gives_no_delta_and_a_missing_base_tar_is_skipped() {
    if zstd_missing() {
        return;
    }
    let scratch = TempDir::new("gaia-layer-delta-unrelated");
    let unrelated = release(&scratch.join("v0"), &[("app", &noise(7, 32 * 1024))]);
    let gone = release(&scratch.join("v1"), &[("app", &noise(8, 32 * 1024))]);
    fs::remove_file(scratch.join("v1/app.tar")).expect("remove tar");
    let new = release(&scratch.join("v2"), &[("app", &noise(9, 32 * 1024))]);

    let document = write_deltas(&new, &[unrelated, gone], &scratch.join("out")).expect("deltas");

    let app = &document["layers"][0];
    assert_eq!(app["status"], "changed");
    assert_eq!(app["deltas"], json!([]), "{app}");
    assert!(app["full"]["size"].as_u64().is_some());
}

#[test]
fn a_new_tar_that_does_not_match_layers_json_is_refused() {
    if zstd_missing() {
        return;
    }
    let scratch = TempDir::new("gaia-layer-delta-stale");
    let new = release(&scratch.join("v2"), &[("app", b"one")]);
    fs::write(scratch.join("v2/app.tar"), b"two").expect("rewrite");

    let error = write_deltas(&new, &[], &scratch.join("out")).expect_err("stale tar");

    assert!(error.contains("but layers.json records"), "{error}");
}

#[test]
fn the_output_dir_is_required() {
    let outcome = layer_delta_command("layers", &[], None, false);
    assert!(
        matches!(&outcome, CommandOutcome::Failed { message } if message.contains("--out")),
        "{outcome:?}"
    );
}
