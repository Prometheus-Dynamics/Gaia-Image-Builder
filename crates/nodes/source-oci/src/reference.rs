//! An image reference: `[registry/]repository[:tag][@digest]`.

/// Where an image is: its registry's base URL and its repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// `https://ghcr.io`, or `http://127.0.0.1:5000` for a registry on this
    /// machine.
    pub base: String,
    pub repository: String,
    /// The digest the reference itself names, when it names one.
    pub digest: Option<String>,
}

impl Reference {
    /// Parses a reference. Docker Hub's short names (`alpine`,
    /// `user/image`) are read as Docker Hub reads them.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (rest, digest) = match text.split_once('@') {
            Some((rest, digest)) => (rest, Some(digest.to_string())),
            None => (text, None),
        };
        let (host, path) = match rest.split_once('/') {
            Some((first, path))
                if first.contains('.') || first.contains(':') || first == "localhost" =>
            {
                (first.to_string(), path.to_string())
            }
            _ => ("docker.io".to_string(), rest.to_string()),
        };
        // A tag is after the last `:` of the last path part.
        let repository = match path.rsplit_once(':') {
            Some((repository, tag)) if !tag.contains('/') => repository.to_string(),
            _ => path,
        };
        let repository = if host == "docker.io" && !repository.contains('/') {
            format!("library/{repository}")
        } else {
            repository
        };
        let valid = !repository.is_empty()
            && repository
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && repository.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.' | '/')
            });
        if !valid {
            return Err(format!("`{text}` is not an image reference"));
        }
        let name = host
            .rsplit_once(':')
            .map_or(host.as_str(), |(name, _)| name);
        let base = match host.as_str() {
            "docker.io" => "https://registry-1.docker.io".to_string(),
            _ if matches!(name, "localhost" | "127.0.0.1" | "[::1]") => format!("http://{host}"),
            _ => format!("https://{host}"),
        };
        Ok(Self {
            base,
            repository,
            digest,
        })
    }
}
