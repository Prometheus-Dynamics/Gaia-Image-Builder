use super::*;

#[test]
fn package_reconcile_disabled_returns_no_messages() {
    let root = unique_dir("gaia-starting-point-packages-disabled");
    let packages = gaia_spec::StartingPointPackagesSpec::default();

    let messages = reconcile_packages(&root, &root, &packages).expect("disabled packages");

    assert!(messages.is_empty());
}

#[test]
fn package_reconcile_plans_apt_commands() {
    let root = unique_dir("gaia-starting-point-packages");
    fs::create_dir_all(root.join("etc")).expect("etc");
    fs::create_dir_all(root.join("usr/bin")).expect("usr bin");
    fs::write(root.join("etc/os-release"), "VERSION_ID=\"24.04\"\n").expect("os-release");
    fs::write(root.join("usr/bin/apt-get"), "").expect("apt-get");

    let packages = gaia_spec::StartingPointPackagesSpec {
        enabled: true,
        execute: false,
        manager: Some("apt".into()),
        release_version: Some("24.04".into()),
        allow_major_upgrade: false,
        update: true,
        dist_upgrade: true,
        install: vec!["curl".into(), "git".into()],
        remove: vec!["nano".into()],
        extra_args: vec!["--no-install-recommends".into()],
        os_release_path: Some("/etc/os-release".into()),
    };

    let commands = reconcile_packages(&root, &root, &packages).expect("planned commands");

    assert!(commands.iter().any(|line| line.ends_with("apt-get update")));
    assert!(commands.iter().any(|line| line.contains("apt-get")
        && line.contains("install")
        && line.contains("-o APT::Default-Release=24.04")
        && line.contains("--no-install-recommends")
        && line.contains("curl")
        && line.contains("git")));
    assert!(
        commands.iter().any(|line| line.contains("apt-get")
            && line.contains("remove")
            && line.contains("nano"))
    );
    assert!(commands.iter().any(|line| line.contains("apt-get")
        && line.contains("dist-upgrade")
        && line.contains("-y")
        && line.contains("--no-install-recommends")));
}

#[test]
fn package_reconcile_execute_true_plans_commands_without_dry_run_notice() {
    let root = unique_dir("gaia-starting-point-packages-execute");
    fs::create_dir_all(root.join("etc")).expect("etc");
    fs::create_dir_all(root.join("usr/bin")).expect("usr bin");
    fs::write(root.join("etc/os-release"), "VERSION_ID=\"24.04\"\n").expect("os-release");
    fs::write(root.join("usr/bin/apt-get"), "").expect("apt-get");

    let packages = gaia_spec::StartingPointPackagesSpec {
        enabled: true,
        execute: true,
        manager: Some("apt".into()),
        release_version: Some("24.04".into()),
        allow_major_upgrade: false,
        update: true,
        dist_upgrade: false,
        install: vec!["curl".into()],
        remove: Vec::new(),
        extra_args: Vec::new(),
        os_release_path: Some("/etc/os-release".into()),
    };

    let plan = plan_package_reconcile(&root, &packages).expect("package plan");

    assert_eq!(plan.commands.len(), 2);
    assert!(
        plan.messages
            .iter()
            .any(|line| line.ends_with("apt-get update"))
    );
    assert!(
        plan.messages
            .iter()
            .all(|line| !line.contains("execute=false"))
    );
}

#[test]
fn package_reconcile_quotes_shell_syntax_in_planned_commands() {
    let root = unique_dir("gaia-starting-point-packages-quote");
    fs::create_dir_all(root.join("etc")).expect("etc");
    fs::create_dir_all(root.join("usr/bin")).expect("usr bin");
    fs::write(root.join("etc/os-release"), "VERSION_ID=\"24.04\"\n").expect("os-release");
    fs::write(root.join("usr/bin/apt-get"), "").expect("apt-get");

    let packages = gaia_spec::StartingPointPackagesSpec {
        enabled: true,
        execute: false,
        manager: Some("apt".into()),
        release_version: Some("24.04".into()),
        allow_major_upgrade: false,
        update: false,
        dist_upgrade: false,
        install: vec!["curl;touch /tmp/pwned".into(), "git".into()],
        remove: Vec::new(),
        extra_args: vec!["--option=A B".into()],
        os_release_path: Some("/etc/os-release".into()),
    };

    let commands = reconcile_packages(&root, &root, &packages).expect("planned commands");
    let install = commands
        .iter()
        .find(|line| line.contains(" install "))
        .expect("install command");

    assert!(install.contains("'curl;touch /tmp/pwned'"));
    assert!(install.contains("'--option=A B'"));
}

#[test]
fn package_reconcile_blocks_major_release_change_without_opt_in() {
    let root = unique_dir("gaia-starting-point-packages-major");
    fs::create_dir_all(root.join("etc")).expect("etc");
    fs::create_dir_all(root.join("usr/bin")).expect("usr bin");
    fs::write(root.join("etc/os-release"), "VERSION_ID=\"22.04\"\n").expect("os-release");
    fs::write(root.join("usr/bin/apt-get"), "").expect("apt-get");

    let packages = gaia_spec::StartingPointPackagesSpec {
        enabled: true,
        execute: false,
        manager: Some("apt".into()),
        release_version: Some("24.04".into()),
        allow_major_upgrade: false,
        update: false,
        dist_upgrade: false,
        install: vec!["curl".into()],
        remove: Vec::new(),
        extra_args: Vec::new(),
        os_release_path: Some("/etc/os-release".into()),
    };

    let error = reconcile_packages(&root, &root, &packages).expect_err("major bump blocked");
    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("allow_major_upgrade=false"));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn privileged_package_execution_fails_early_on_non_linux_hosts() {
    let error = ensure_linux_root("requires root").expect_err("non-Linux hosts are unsupported");

    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("Linux hosts"));
}
