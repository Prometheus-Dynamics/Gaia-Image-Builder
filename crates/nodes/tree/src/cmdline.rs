//! `gaia.cmdline`: a slot's kernel command line, whole.
//!
//! The line a slot boots with is hashed into its signed manifest, and what
//! opens the slot's root reads its own words from it. So one step writes
//! the whole line, byte for byte, and nothing is appended later.
//!
//! In order, as words separated by single spaces, on one line ending in one
//! line feed:
//!
//! 1. the board's base words;
//! 2. the words a generator wrote from the board's description;
//! 3. the image's additions (its components', then its recipe's);
//! 4. the slot's words, a file the generator wrote for this slot, with one
//!    hole, `@roothash@`, that only the build can fill: the dm-verity root
//!    hash of the slot's image, from its verity table.
//!
//! The slot's words are last because the program that reads them takes the
//! last word for a key. An earlier source must therefore not set one of
//! their keys, or anything in their namespace (the first slot word's key up
//! to its first `.`): it would lose without a sign, so it is refused here
//! with the source and the word named. Nothing of those words is spelled in
//! this code: the keys and the namespace are read from the file.
//!
//! An image that has a slot id gives it: the first of the slot's words must
//! then have it as its value, so a generator run for another slot (or for
//! none) is caught here and not at boot.
//!
//! The line is bounded: a kernel keeps only so many bytes of it. Over the
//! limit, the build fails with the size, the limit and the source whose
//! words crossed it.

use crate::walk::inside;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::Path;

const NAME: &str = "gaia.cmdline";
/// The hole in the slot's words.
pub const HOLE: &str = "@roothash@";
/// The file the step makes.
const OUTPUT: &str = "cmdline";

/// A word's key: the text before its `=`, or the whole word.
fn key_of(word: &str) -> &str {
    word.split_once('=').map_or(word, |(key, _)| key)
}

/// The whole line: `sources` in order (each a name for errors and its
/// text), then `slot` with its hole filled by `roothash`; at most `limit`
/// bytes without the line feed (0 for no limit).
///
/// # Errors
///
/// A root hash that is not lowercase hex of 64 digits or more; slot words
/// with no hole or more than one; an earlier word in the slot's keys or
/// namespace; a line over the limit.
pub fn assemble(
    sources: &[(&str, &str)],
    slot: Option<&str>,
    roothash: &str,
    limit: usize,
) -> Result<String, String> {
    let slot_words: Vec<String> = match slot {
        None => Vec::new(),
        Some(text) => {
            let holes = text.matches(HOLE).count();
            if holes != 1 {
                return Err(format!(
                    "the slot's words have {holes} `{HOLE}` holes; exactly one is filled"
                ));
            }
            let hex = roothash.len() >= 64
                && roothash.len().is_multiple_of(2)
                && roothash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !hex {
                return Err(format!(
                    "the root hash {roothash:?} is not lowercase hex of 64 digits or more"
                ));
            }
            text.replace(HOLE, roothash)
                .split_whitespace()
                .map(str::to_string)
                .collect()
        }
    };
    let slot_keys: Vec<&str> = slot_words.iter().map(|word| key_of(word)).collect();
    // The namespace: the first slot word's key up to and including its
    // first `.`, when it has one.
    let namespace: Option<&str> = slot_keys
        .first()
        .and_then(|key| key.find('.').map(|dot| &key[..=dot]));

    let mut line = String::new();
    let push = |line: &mut String, word: &str, from: &str| -> Result<(), String> {
        let added = usize::from(!line.is_empty()) + word.len();
        if limit != 0 && line.len() + added > limit {
            return Err(format!(
                "the command line is over {limit} bytes at `{word}`, a word of {from} ({} bytes before it)",
                line.len()
            ));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        Ok(())
    };
    for (from, text) in sources {
        for word in text.split_whitespace() {
            let key = key_of(word);
            if slot_keys.contains(&key) || namespace.is_some_and(|ns| key.starts_with(ns)) {
                return Err(format!(
                    "`{word}`, a word of {from}, sets what the slot's own words set (`{}`); the slot's words are last and would win without a sign, so remove it from {from}",
                    slot_words
                        .iter()
                        .find(|slot| key_of(slot) == key)
                        .map_or_else(
                            || format!("everything under `{}`", namespace.unwrap_or_default()),
                            Clone::clone
                        )
                ));
            }
            push(&mut line, word, from)?;
        }
    }
    for word in &slot_words {
        push(&mut line, word, "the slot")?;
    }
    line.push('\n');
    Ok(line)
}

/// Whether the first of the slot's words `text` has `slot` as its value.
pub fn names_slot(text: &str, slot: &str) -> bool {
    text.split_whitespace()
        .next()
        .and_then(|word| word.split_once('='))
        .is_some_and(|(_, value)| value == slot)
}

/// The `root_hash` of a verity table artifact's text.
pub fn root_hash_of(table: &str) -> Option<&str> {
    table
        .lines()
        .find_map(|line| line.strip_prefix("root_hash "))
        .map(str::trim)
}

/// The slot's command line as a file. `base` and `extra` are words;
/// `generated` is a tree with the generator's words at `words_path` and the
/// slot's at `slot_path` (either path empty for none); `table` is the
/// slot's verity table; `limit` is the most bytes the line may have (0 for
/// no limit); `slot` is the image's slot id (empty for none), which the
/// first of the slot's words must name.
#[node(
    id = "gaia.cmdline",
    inputs(
        store, base, generated, words_path, slot_path, slot, extra, table, limit
    ),
    outputs(cmdline)
)]
#[allow(clippy::too_many_arguments)]
pub fn cmdline(
    store: &str,
    base: &str,
    generated: Tree,
    words_path: &str,
    slot_path: &str,
    slot: &str,
    extra: &str,
    table: &Artifact,
    limit: i64,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let generated_object = generated
        .object
        .object_ref()
        .ok_or_else(|| fail("the generator's output is not in the store".into()))?;
    let read = |path: &str, what: &str| -> Result<Option<String>, StepError> {
        if path.is_empty() {
            return Ok(None);
        }
        let relative = inside(path).map_err(fail)?;
        std::fs::read_to_string(store.path(&generated_object).join(&relative))
            .map(Some)
            .map_err(|e| fail(format!("{what}, {relative}: {e}")))
    };
    let words = read(words_path, "the generator's words")?;
    let slot_id = slot;
    let slot = read(slot_path, "the slot's words")?;
    if let Some(text) = &slot
        && !slot_id.is_empty()
        && !names_slot(text, slot_id)
    {
        return Err(fail(format!(
            "the slot's words begin `{}`, and the image's slot is `{slot_id}`: the generator was run for another",
            text.split_whitespace().next().unwrap_or_default()
        )));
    }
    let roothash = match &slot {
        None => String::new(),
        Some(_) => {
            let object = table
                .object
                .object_ref()
                .ok_or_else(|| fail("the verity table is not in the store".into()))?;
            let text = std::fs::read_to_string(store.path(&object))
                .map_err(|e| fail(format!("the verity table: {e}")))?;
            root_hash_of(&text)
                .ok_or_else(|| fail("the verity table has no root_hash".into()))?
                .to_string()
        }
    };
    let limit = usize::try_from(limit).map_err(|_| fail("a limit below 0".into()))?;
    let line = assemble(
        &[
            ("the board's base", base),
            (
                "the generator's words",
                words.as_deref().unwrap_or_default(),
            ),
            ("the image's additions", extra),
        ],
        slot.as_deref(),
        &roothash,
        limit,
    )
    .map_err(fail)?;
    // The line is the whole of what is made, so it is the key.
    let key = KeyBuilder::new(NAME, 1).config("line", &line).finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("cmdline", Path::new(OUTPUT))],
        |work| {
            std::fs::write(work.dir().join(OUTPUT), &line)
                .map_err(|e| StepError::body(NAME, e.to_string()))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("cmdline"),
        object: made.outputs["cmdline"].into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "d49facf3f0db6b2a180af10a6685f8c75b94e3436ba2fb64c654f36b3b5df407";
    const SLOT: &str = "os.slot=base os.roothash=@roothash@ os.src.rpi-boot-mode-1=/soc@107c000000/mmc@fff000 panic=5\n";

    #[test]
    fn the_line_is_the_sources_in_order_then_the_slots_words() {
        let line = assemble(
            &[
                ("the board's base", "console=ttyAMA0,115200  rootwait\n"),
                ("the generator's words", "mod.owned=a,b mod.x=1\n"),
                ("the image's additions", "quiet"),
            ],
            Some(SLOT),
            HASH,
            0,
        )
        .expect("assembles");
        assert_eq!(
            line,
            format!(
                "console=ttyAMA0,115200 rootwait mod.owned=a,b mod.x=1 quiet os.slot=base os.roothash={HASH} os.src.rpi-boot-mode-1=/soc@107c000000/mmc@fff000 panic=5\n"
            )
        );
        // No slot: the sources alone, still one line and one line feed.
        assert_eq!(
            assemble(&[("the board's base", " a=1\n\n b ")], None, "", 0),
            Ok("a=1 b\n".into())
        );
    }

    #[test]
    fn an_earlier_word_in_the_slots_keys_or_namespace_is_refused_by_source() {
        let run = |source: &str, text: &str| {
            assemble(&[(source, text)], Some(SLOT), HASH, 0).expect_err("refused")
        };
        let why = run("the board's base", "console=x panic=10");
        assert!(
            why.contains("`panic=10`, a word of the board's base"),
            "{why}"
        );
        assert!(why.contains("(`panic=5`)"), "{why}");
        let why = run("the image's additions", "os.slot=other");
        assert!(why.contains("a word of the image's additions"), "{why}");
        // Anything under the namespace, even a key the slot's words lack.
        let why = run("the generator's words", "os.data=PARTUUID=1234");
        assert!(why.contains("everything under `os.`"), "{why}");
        // A key that only begins with the same letters is no part of it.
        assert!(assemble(&[("x", "osx.a=1 panicked=1")], Some(SLOT), HASH, 0).is_ok());
    }

    #[test]
    fn the_hole_and_the_root_hash_are_checked() {
        let none = SLOT.replace(HOLE, "abc");
        assert!(
            assemble(&[], Some(&none), HASH, 0)
                .expect_err("no hole")
                .contains("have 0 `@roothash@` holes")
        );
        let two = format!("{SLOT} x=@roothash@");
        assert!(assemble(&[], Some(&two), HASH, 0).is_err());
        for bad in [
            "",
            "D49F",
            &HASH.to_uppercase(),
            &HASH[..63],
            &format!("0x{HASH}"),
        ] {
            let why = assemble(&[], Some(SLOT), bad, 0).expect_err("bad hash");
            assert!(why.contains("is not lowercase hex"), "{bad:?}: {why}");
        }
    }

    #[test]
    fn a_line_over_the_limit_names_the_source_that_crossed_it() {
        let ok = assemble(&[("the board's base", "a=1 b=2")], None, "", 7).expect("fits");
        assert_eq!(ok, "a=1 b=2\n", "the line feed is not counted");
        let why = assemble(
            &[
                ("the board's base", "a=1"),
                ("the image's additions", "b=2"),
            ],
            None,
            "",
            6,
        )
        .expect_err("over");
        assert_eq!(
            why,
            "the command line is over 6 bytes at `b=2`, a word of the image's additions (3 bytes before it)"
        );
        // The slot's words count, device-tree paths and all.
        let why = assemble(&[], Some(SLOT), HASH, 100).expect_err("over");
        assert!(why.contains("a word of the slot"), "{why}");
        assert_eq!(
            root_hash_of(&format!("hash_type 1\nroot_hash {HASH}\nsalt 00\n")),
            Some(HASH)
        );
        assert_eq!(root_hash_of("salt 00\n"), None);
    }
}
