use tracing_subscriber::EnvFilter;

fn main() {
    bootstrap_logging();
    std::process::exit(gaia_app::run());
}

fn bootstrap_logging() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_else(|_| EnvFilter::default());

    if std::env::args().nth(1).as_deref() == Some("tui") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .without_time()
            .with_writer(std::io::sink)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .without_time()
            .try_init();
    }

    tracing::debug!("gaia bootstrap logging initialized");
}
