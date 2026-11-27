//! A source as a recipe writes it.

use gaia_describe::{ImagePlatform, Source};

fn source(text: &str) -> Result<Source, toml::de::Error> {
    toml::from_str(text)
}

#[test]
fn an_image_is_for_the_target_unless_it_says_host() {
    let image = |platform| Source::Image {
        reference: "r".into(),
        platform,
    };
    assert_eq!(
        source("image = \"r\"").expect("parses"),
        image(ImagePlatform::Target)
    );
    assert_eq!(
        source("image = \"r\"\nplatform = \"target\"").expect("parses"),
        image(ImagePlatform::Target)
    );
    assert_eq!(
        source("image = \"r\"\nplatform = \"host\"").expect("parses"),
        image(ImagePlatform::Host)
    );
    let other = source("image = \"r\"\nplatform = \"arm64\"").expect_err("refused");
    assert!(other.to_string().contains("platform"), "{other}");
    let git = source("git = \"u\"\nref = \"main\"\nplatform = \"host\"").expect_err("refused");
    assert!(
        git.to_string()
            .contains("only an image source has a `platform`"),
        "{git}"
    );
}
