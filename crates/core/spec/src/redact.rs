/// Replaces the userinfo of every `scheme://userinfo@host` URL in `text` with
/// `***`, so `https://user:token@host/repo.git` reads `https://***@host/repo.git`.
///
/// Works on any text (log lines, error messages, state files), not only on
/// whole URLs. The authority ends at the first `/`, `?`, `#`, whitespace or
/// quote; the userinfo is everything before the last `@` in it.
pub fn redact_url_userinfo(text: &str) -> String {
    const DELIMITERS: &[char] = &['/', '?', '#', '\'', '"', '<', '>', '`'];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("://") {
        let (head, tail) = rest.split_at(position + "://".len());
        out.push_str(head);
        let end = tail
            .find(|character: char| DELIMITERS.contains(&character) || character.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, remainder) = tail.split_at(end);
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = remainder;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::redact_url_userinfo;

    #[test]
    fn redacts_userinfo_in_whole_urls() {
        assert_eq!(
            redact_url_userinfo("https://user:ghp_secret@github.com/org/repo.git"),
            "https://***@github.com/org/repo.git"
        );
        assert_eq!(
            redact_url_userinfo("https://ghp_token@github.com/org/repo.git"),
            "https://***@github.com/org/repo.git"
        );
    }

    #[test]
    fn leaves_urls_without_userinfo_unchanged() {
        for url in [
            "https://github.com/org/repo.git",
            "file:///srv/git/repo",
            "git@github.com:org/repo.git",
            "/home/user/repo",
            "https://host:8443/path?x=y@z",
        ] {
            assert_eq!(redact_url_userinfo(url), url, "{url}");
        }
    }

    #[test]
    fn redacts_urls_embedded_in_messages() {
        let message = "fatal: unable to access 'https://user:pw@host.example/x.git/': denied; retry 'https://a:b@c.example' now";
        assert_eq!(
            redact_url_userinfo(message),
            "fatal: unable to access 'https://***@host.example/x.git/': denied; retry 'https://***@c.example' now"
        );
    }

    #[test]
    fn userinfo_ends_at_last_at_sign_of_the_authority() {
        assert_eq!(
            redact_url_userinfo("https://user:p@ss@host/path@tail"),
            "https://***@host/path@tail"
        );
    }

    #[test]
    fn handles_text_without_scheme_and_trailing_separator() {
        assert_eq!(redact_url_userinfo("no url here"), "no url here");
        assert_eq!(redact_url_userinfo("https://"), "https://");
        assert_eq!(redact_url_userinfo(""), "");
    }
}
