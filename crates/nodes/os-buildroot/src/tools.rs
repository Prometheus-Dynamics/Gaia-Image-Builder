//! Tools every make of a Buildroot output directory finds first on its
//! `PATH`, so what the build writes is the same on every run.
//!
//! GNU ar's default (binutils 2.46, as most distributions build it) is to
//! stamp an archive's symbol table with the time it was written, and ranlib
//! does the same when it rewrites one; a member's own date follows
//! `SOURCE_DATE_EPOCH`, the table's does not. So two builds of the same
//! inputs make different `libz.a`, `libcap.a`, ..., and a host tools seed
//! made in one store differed from the same key's seed made in another.
//! The wrappers here, `ar` and `ranlib` in [`DIR`] of the output directory,
//! run the machine's programs in deterministic mode (`D`, `-D`: dates, owners
//! and modes 0, 0, 644), unless the caller asked for real ones (`U`, `-U`).
//! Host packages take `ar` by name (Buildroot's `HOSTAR`, autoconf's `AR`,
//! libtool, CMake's ranlib), so the wrapper is what they run.
//!
//! What no wrapper reaches (a package that runs an archiver by an absolute
//! path, a cross ar) is put right when a tree is stored:
//! [`deterministic_archives`] sets the same header fields, which is what `D`
//! writes, and says which archives it rewrote.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The directory of the wrappers, in the output directory.
pub(crate) const DIR: &str = ".gaia-tools";

/// The machine's `name`, from the build's `PATH` ([`crate::PATH`]).
fn machine(name: &str) -> Option<PathBuf> {
    crate::PATH
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

/// The `ar` wrapper: `D` added to the operation (the first argument that is
/// not a long option or an option's value), unless it has `U` or `D`.
fn ar_script(ar: &Path) -> String {
    format!(
        r#"#!/bin/sh
# Written by Gaia (os-buildroot `tools`): GNU ar in deterministic mode.
done=0 prev=
for arg; do
    shift
    if [ $done = 0 ]; then
        case $prev in
            --plugin|--target|--output) ;;
            *)
                case $arg in
                    --*|-V|-v|-h) ;;
                    *[UD]*) done=1 ;;
                    *) arg="${{arg}}D"; done=1 ;;
                esac
                ;;
        esac
    fi
    set -- "$@" "$arg"
    prev=$arg
done
exec {} "$@"
"#,
        ar.display()
    )
}

/// The `ranlib` wrapper: `-D` first, unless `-U` or `-D` is given.
fn ranlib_script(ranlib: &Path) -> String {
    format!(
        r#"#!/bin/sh
# Written by Gaia (os-buildroot `tools`): ranlib in deterministic mode.
for arg; do
    case $arg in -U|-D|--version|-v|-V|--help|-h) exec {ranlib} "$@" ;; esac
done
exec {ranlib} -D "$@"
"#,
        ranlib = ranlib.display()
    )
}

/// Writes the wrappers into `out`'s [`DIR`]. A machine without `ar` gets
/// none: Buildroot's own dependency check names the missing program.
pub(crate) fn write(out: &Path) -> io::Result<()> {
    let dir = out.join(DIR);
    fs::create_dir_all(&dir)?;
    for (name, script) in [
        ("ar", machine("ar").map(|ar| ar_script(&ar))),
        (
            "ranlib",
            machine("ranlib").map(|ranlib| ranlib_script(&ranlib)),
        ),
    ] {
        if let Some(script) = script {
            let path = dir.join(name);
            fs::write(&path, script)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

/// The `PATH` of a make whose output directory it sees at `seen`.
pub(crate) fn path(seen: &Path) -> String {
    format!("{}:{}", seen.join(DIR).display(), crate::PATH)
}

/// An archive's member headers as `D` writes them: each header's date, owner
/// and group 0 and mode 644, in place (the fields are fixed-width, so no
/// offset moves). Regular files under `root` that start with `!<arch>\n`;
/// thin archives name their members and are left. Returns the archives
/// rewritten, relative to `root`, sorted.
pub(crate) fn deterministic_archives(root: &Path) -> io::Result<Vec<String>> {
    let mut rewritten = Vec::new();
    for relative in crate::host::files(root)? {
        let path = root.join(&relative);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_file() || meta.len() < 8 {
            continue;
        }
        let mut magic = [0u8; 8];
        io::Read::read_exact(&mut fs::File::open(&path)?, &mut magic)?;
        if &magic != b"!<arch>\n" {
            continue;
        }
        let mut bytes = fs::read(&path)?;
        if zero_headers(&mut bytes) {
            crate::host::write_keeping_mode(&path, &meta, &bytes)?;
            rewritten.push(relative.display().to_string());
        }
    }
    Ok(rewritten)
}

/// Sets each member header's date, owner, group and mode as `D` writes
/// them (see the comment inside); whether anything changed. A header that
/// does not end as one does stops the walk where it is.
fn zero_headers(bytes: &mut [u8]) -> bool {
    /// `slot` set to `value`, padded with spaces; whether it changed.
    fn set(slot: &mut [u8], value: &str) -> bool {
        let mut want = value.as_bytes().to_vec();
        want.resize(slot.len(), b' ');
        let changed = slot != want.as_slice();
        slot.copy_from_slice(&want);
        changed
    }
    let mut changed = false;
    let mut at = 8;
    while at + 60 <= bytes.len() {
        let header = &mut bytes[at..at + 60];
        if &header[58..60] != b"`\n" {
            break;
        }
        let size: usize = match std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            Some(size) => size,
            None => break,
        };
        // As `D` writes them: the long names' table (`//`) has no date,
        // owner or mode at all; the symbol table (`/`, `/SYM64/`) mode 0;
        // a member mode 644.
        let name = &header[..16];
        let mode = if name.starts_with(b"// ") {
            None
        } else if name.starts_with(b"/ ") || name.starts_with(b"/SYM64/ ") {
            Some("0")
        } else {
            Some("644")
        };
        if let Some(mode) = mode {
            changed |= set(&mut header[16..28], "0");
            changed |= set(&mut header[28..34], "0");
            changed |= set(&mut header[34..40], "0");
            changed |= set(&mut header[40..48], mode);
        }
        at += 60 + size + size % 2;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// An archive of one object made by `ar` with `op`, the wrapper or the
    /// machine's, then `ranlib` on it (with `-U` when `op` has `U`).
    fn archive(dir: &Path, name: &str, op: &str, wrapped: bool) -> Vec<u8> {
        let path = if wrapped {
            path(dir)
        } else {
            crate::PATH.to_string()
        };
        let c = dir.join("a.c");
        fs::write(&c, "int f(void) { return 1; }\n").expect("source");
        let status = Command::new("cc")
            .args(["-c", "-o"])
            .arg(dir.join("a.o"))
            .arg(&c)
            .env("PATH", &path)
            .status()
            .expect("cc");
        assert!(status.success());
        let out = dir.join(name);
        // By its path: a command is looked up on the test's own `PATH`, not
        // the one it is given (make looks up on its own).
        let run = |program: &str, args: &[&str]| {
            let found = if wrapped {
                dir.join(DIR).join(program)
            } else {
                PathBuf::from(program)
            };
            let status = Command::new(&found)
                .args(args)
                .arg(&out)
                .args(if program == "ar" {
                    vec![dir.join("a.o")]
                } else {
                    vec![]
                })
                .env("PATH", &path)
                .status()
                .expect(program);
            assert!(status.success(), "{program} {args:?}");
        };
        run("ar", &[op]);
        // Real dates asked of ar are asked of ranlib too.
        run("ranlib", if op.contains('U') { &["-U"] } else { &[] });
        fs::read(&out).expect("archive")
    }

    /// The symbol table's date, owner and group fields, as written.
    fn table_fields(bytes: &[u8]) -> String {
        String::from_utf8_lossy(&bytes[8 + 16..8 + 40]).into_owned()
    }

    /// The machine's ar and ranlib stamp the symbol table with the time; the
    /// wrappers make the same archive with 0 there, and the same bytes a
    /// second later. An operation that asks for real dates keeps them.
    #[test]
    fn the_wrapped_archivers_write_the_same_archive_every_time() {
        if machine("ar").is_none() || machine("cc").is_none() {
            eprintln!("skipped: no ar or cc");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path()).expect("wrappers");
        let tools = dir.path();
        // `path(dir)` puts `dir/.gaia-tools` first. The machine's own ar
        // asked for real dates: some hosts' ar (Debian's, Ubuntu's) writes
        // 0 by default, others (Fedora's) the time, so the sample of an
        // archive with dates asks for them.
        let plain = archive(tools, "plain.a", "rcsU", false);
        assert!(
            !table_fields(&plain).starts_with("0 "),
            "the machine's ar wrote no date when asked for one: {:?}",
            table_fields(&plain)
        );
        let first = archive(tools, "one.a", "rcs", true);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = archive(tools, "two.a", "rcs", true);
        assert_eq!(first, second);
        assert!(
            table_fields(&first).starts_with("0 "),
            "{:?}",
            table_fields(&first)
        );
        let real = archive(tools, "real.a", "rcsU", true);
        assert!(
            !table_fields(&real).starts_with("0 "),
            "{:?}",
            table_fields(&real)
        );
    }

    /// What no wrapper reached is rewritten as `D` writes it: the machine's
    /// archive becomes the wrapped one's bytes, is named once, and is left
    /// alone the second time.
    #[test]
    fn an_archive_with_real_dates_is_rewritten_once() {
        if machine("ar").is_none() || machine("cc").is_none() {
            eprintln!("skipped: no ar or cc");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path()).expect("wrappers");
        let wrapped = archive(dir.path(), "wrapped.a", "rcs", true);
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("lib")).expect("tree");
        fs::write(
            tree.join("lib/libplain.a"),
            // Real dates, as an unwrapped ar writes on some hosts.
            archive(dir.path(), "plain.a", "rcsU", false),
        )
        .expect("plain");
        fs::write(tree.join("lib/libwrapped.a"), &wrapped).expect("wrapped");
        fs::write(tree.join("lib/notes.a"), "not an archive").expect("text");
        assert_eq!(
            deterministic_archives(&tree).expect("rewrite"),
            ["lib/libplain.a"]
        );
        assert_eq!(
            fs::read(tree.join("lib/libplain.a")).expect("read"),
            wrapped
        );
        assert!(deterministic_archives(&tree).expect("again").is_empty());
    }
}
