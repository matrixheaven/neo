use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer as _, SigningKey};
use neo_extensions::{
    PackageInstallKind, PackageInstaller, PackageKind, PackageValidationError, validate_package,
};
use sha2::{Digest as _, Sha256};
use tar::{Builder, EntryType, Header};

#[test]
fn validates_signed_package_and_installs_extension_archive() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::Extension,
        "echo",
        "0.1.0",
        "neo-extension.toml",
        &[PackageEntry::file(
            "neo-extension.toml",
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
        )],
    );

    let validated = validate_package(&package).unwrap();

    assert_eq!(validated.manifest.kind, PackageKind::Extension);
    assert_eq!(validated.manifest.id, "echo");
    assert_eq!(validated.archive_path, dir.path().join("echo-0.1.0.tar"));

    let install_root = dir.path().join(".neo/extensions");
    let installed = PackageInstaller::new(&install_root)
        .install(&validated, PackageInstallKind::Extension)
        .unwrap();

    assert_eq!(installed.id, "echo");
    assert!(install_root.join("echo/neo-extension.toml").exists());
}

#[test]
fn validates_prompt_pack_and_theme_package_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let prompt_package = write_signed_package(
        &dir.path().join("prompt"),
        PackageKind::PromptPack,
        "team-prompts",
        "1.2.3",
        "review.md",
        &[PackageEntry::file(
            "review.md",
            "---\ndescription: Review code\n---\nReview $ARGUMENTS\n",
        )],
    );
    let theme_package = write_signed_package(
        &dir.path().join("theme"),
        PackageKind::Theme,
        "night-owl",
        "2.0.0",
        "night-owl.json",
        &[PackageEntry::file(
            "night-owl.json",
            r##"{"name":"Night Owl","colors":{"prompt":"#82aaff"}}"##,
        )],
    );

    assert_eq!(
        validate_package(&prompt_package).unwrap().manifest.kind,
        PackageKind::PromptPack
    );
    assert_eq!(
        validate_package(&theme_package).unwrap().manifest.kind,
        PackageKind::Theme
    );
}

#[test]
fn rejects_archive_with_wrong_sha256_digest() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::Theme,
        "theme",
        "0.1.0",
        "theme.json",
        &[PackageEntry::file("theme.json", r#"{"name":"Theme"}"#)],
    );
    let mut manifest = fs::read_to_string(&package).unwrap();
    manifest = manifest.replace(
        "sha256 = \"",
        "sha256 = \"0000000000000000000000000000000000000000000000000000000000000000",
    );
    fs::write(&package, manifest).unwrap();

    let error = validate_package(&package).unwrap_err();

    assert!(matches!(
        error,
        PackageValidationError::DigestMismatch { .. }
    ));
}

#[test]
fn rejects_archive_with_wrong_ed25519_signature() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::Theme,
        "theme",
        "0.1.0",
        "theme.json",
        &[PackageEntry::file("theme.json", r#"{"name":"Theme"}"#)],
    );
    let manifest = fs::read_to_string(&package)
        .unwrap()
        .lines()
        .map(|line| {
            if line.starts_with("signature = ") {
                format!("signature = \"{}\"", STANDARD.encode([0_u8; 64]))
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&package, manifest).unwrap();

    let error = validate_package(&package).unwrap_err();

    assert!(matches!(
        error,
        PackageValidationError::InvalidSignature { .. }
    ));
}

#[test]
fn rejects_absolute_archive_entry_paths() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::Theme,
        "theme",
        "0.1.0",
        "theme.json",
        &[PackageEntry::unsafe_file("/tmp/escape.json", "{}")],
    );

    let error = validate_package(&package).unwrap_err();

    assert!(matches!(
        error,
        PackageValidationError::UnsafeArchivePath { .. }
    ));
}

#[test]
fn rejects_parent_directory_archive_entry_paths() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::Theme,
        "theme",
        "0.1.0",
        "theme.json",
        &[PackageEntry::unsafe_file("../escape.json", "{}")],
    );

    let error = validate_package(&package).unwrap_err();

    assert!(matches!(
        error,
        PackageValidationError::UnsafeArchivePath { .. }
    ));
}

#[test]
fn rejects_symlinks_that_escape_package_root() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::PromptPack,
        "prompts",
        "0.1.0",
        "links/outside",
        &[
            PackageEntry::dir("links"),
            PackageEntry::symlink("links/outside", "../../escape"),
            PackageEntry::file("review.md", "Review\n"),
        ],
    );

    let error = validate_package(&package).unwrap_err();

    assert!(matches!(
        error,
        PackageValidationError::UnsafeSymlink { .. }
    ));
}

#[test]
fn rejects_hardlinks_that_escape_package_root() {
    let dir = tempfile::tempdir().unwrap();
    let package = write_signed_package(
        dir.path(),
        PackageKind::PromptPack,
        "prompts",
        "0.1.0",
        "links/outside",
        &[
            PackageEntry::dir("links"),
            PackageEntry::hardlink("links/outside", "../../escape"),
            PackageEntry::file("review.md", "Review\n"),
        ],
    );

    let error = validate_package(&package).unwrap_err();

    assert!(matches!(
        error,
        PackageValidationError::UnsafeArchiveLink { .. }
    ));
}

#[derive(Clone)]
struct PackageEntry {
    path: PathBuf,
    kind: PackageEntryKind,
}

#[derive(Clone)]
enum PackageEntryKind {
    File(String),
    Dir,
    Hardlink(PathBuf),
    Symlink(PathBuf),
    UnsafeFile(String),
}

impl PackageEntry {
    fn file(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: PackageEntryKind::File(content.into()),
        }
    }

    fn unsafe_file(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: PackageEntryKind::UnsafeFile(content.into()),
        }
    }

    fn dir(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: PackageEntryKind::Dir,
        }
    }

    fn symlink(path: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: PackageEntryKind::Symlink(target.into()),
        }
    }

    fn hardlink(path: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: PackageEntryKind::Hardlink(target.into()),
        }
    }
}

fn write_signed_package(
    root: &Path,
    kind: PackageKind,
    id: &str,
    version: &str,
    entry: &str,
    entries: &[PackageEntry],
) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let archive_path = root.join(format!("{id}-{version}.tar"));
    write_archive(&archive_path, entries);
    let archive_bytes = fs::read(&archive_path).unwrap();
    let digest = hex_sha256(&archive_bytes);
    let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(&archive_bytes);

    let manifest_path = root.join(".neo-package.toml");
    fs::write(
        &manifest_path,
        format!(
            r#"
kind = "{kind}"
id = "{id}"
version = "{version}"
entry = "{entry}"

[archive]
path = "{id}-{version}.tar"
sha256 = "{digest}"

[signature]
algorithm = "ed25519"
public_key = "{}"
signature = "{}"
"#,
            STANDARD.encode(verifying_key.to_bytes()),
            STANDARD.encode(signature.to_bytes()),
        ),
    )
    .unwrap();
    manifest_path
}

fn write_archive(path: &Path, entries: &[PackageEntry]) {
    let file = fs::File::create(path).unwrap();
    let mut builder = Builder::new(file);
    for entry in entries {
        match &entry.kind {
            PackageEntryKind::File(content) => {
                let bytes = content.as_bytes();
                let mut header = Header::new_gnu();
                header.set_size(bytes.len().try_into().unwrap());
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, &entry.path, bytes)
                    .unwrap();
            }
            PackageEntryKind::UnsafeFile(content) => {
                let bytes = content.as_bytes();
                let mut header = Header::new_old();
                header.as_old_mut().name[..entry.path.as_os_str().len()]
                    .copy_from_slice(entry.path.as_os_str().as_encoded_bytes());
                header.set_size(bytes.len().try_into().unwrap());
                header.set_mode(0o644);
                header.set_cksum();
                builder.append(&header, bytes).unwrap();
            }
            PackageEntryKind::Dir => {
                let mut header = Header::new_gnu();
                header.set_size(0);
                header.set_entry_type(EntryType::Directory);
                header.set_mode(0o755);
                header.set_cksum();
                builder
                    .append_data(&mut header, &entry.path, std::io::empty())
                    .unwrap();
            }
            PackageEntryKind::Hardlink(target) => {
                let mut header = Header::new_gnu();
                header.set_size(0);
                header.set_entry_type(EntryType::Link);
                header.set_link_name(target).unwrap();
                header.set_mode(0o777);
                header.set_cksum();
                builder
                    .append_data(&mut header, &entry.path, std::io::empty())
                    .unwrap();
            }
            PackageEntryKind::Symlink(target) => {
                let mut header = Header::new_gnu();
                header.set_size(0);
                header.set_entry_type(EntryType::Symlink);
                header.set_link_name(target).unwrap();
                header.set_mode(0o777);
                header.set_cksum();
                builder
                    .append_data(&mut header, &entry.path, std::io::empty())
                    .unwrap();
            }
        }
    }
    builder.finish().unwrap();
    builder.into_inner().unwrap().flush().unwrap();
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}
