// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use std::io::Write;

fn make_pack_zip(tmp: &Path, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = tmp.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = Default::default();
    for (filename, bytes) in entries {
        zip.start_file(*filename, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn detect_format_recognises_mrpack() {
    let tmp = tempfile::tempdir().unwrap();
    let path = make_pack_zip(tmp.path(), "pack.mrpack", &[("modrinth.index.json", b"{}")]);
    assert_eq!(detect_format(&path), Ok(PackFormat::Mrpack));
}

#[test]
fn detect_format_recognises_mmc_flat() {
    // mmc-pack.json at the zip root - the flat layout that some mmc
    // archives use.
    let tmp = tempfile::tempdir().unwrap();
    let path = make_pack_zip(tmp.path(), "pack.zip", &[("mmc-pack.json", b"{}")]);
    assert_eq!(detect_format(&path), Ok(PackFormat::Mmc));
}

#[test]
fn detect_format_recognises_mmc_nested() {
    // mmc-pack.json one directory deep - the more common layout where
    // the archive wraps everything in a named directory.
    let tmp = tempfile::tempdir().unwrap();
    let path = make_pack_zip(tmp.path(), "pack.zip", &[("MyPack/mmc-pack.json", b"{}")]);
    assert_eq!(detect_format(&path), Ok(PackFormat::Mmc));
}

#[test]
fn detect_format_prefers_mrpack_when_both_markers_present() {
    // a zip with both markers should resolve to Mrpack since the
    // detector checks modrinth.index.json first.
    let tmp = tempfile::tempdir().unwrap();
    let path = make_pack_zip(
        tmp.path(),
        "weird.zip",
        &[("modrinth.index.json", b"{}"), ("mmc-pack.json", b"{}")],
    );
    assert_eq!(detect_format(&path), Ok(PackFormat::Mrpack));
}

#[test]
fn detect_format_errors_on_unknown_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let path = make_pack_zip(tmp.path(), "random.zip", &[("readme.txt", b"hello")]);
    let err = detect_format(&path).unwrap_err();
    assert!(
        err.contains("Unknown pack format"),
        "expected unknown format error, got: {err}"
    );
}

#[test]
fn detect_format_errors_on_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let err = detect_format(&tmp.path().join("missing.zip")).unwrap_err();
    assert!(err.contains("Cannot open"), "got: {err}");
}

#[test]
fn unique_name_no_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let name = unique_instance_name("TestPack", tmp.path());
    assert_eq!(name, "TestPack");
}

#[test]
fn unique_name_with_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("TestPack");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("instance.json"), "{}").unwrap();
    let name = unique_instance_name("TestPack", tmp.path());
    assert_eq!(name, "TestPack (2)");
}

#[test]
fn unique_name_preserves_directory_without_config() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("TestPack")).unwrap();

    assert_eq!(unique_instance_name("TestPack", tmp.path()), "TestPack (2)");
    assert!(tmp.path().join("TestPack").exists());
}

#[test]
fn unique_name_multiple_collisions() {
    let tmp = tempfile::tempdir().unwrap();
    for suffix in ["", " (2)", " (3)"] {
        let dir = tmp.path().join(format!("TestPack{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("instance.json"), "{}").unwrap();
    }
    let name = unique_instance_name("TestPack", tmp.path());
    assert_eq!(name, "TestPack (4)");
}

#[test]
fn parse_project_url() {
    assert_eq!(
        parse_import_input("https://modrinth.com/modpack/fabulously-optimized"),
        ImportInput::ProjectSlug("fabulously-optimized".to_string())
    );
}

#[test]
fn parse_version_url() {
    assert_eq!(
        parse_import_input("https://modrinth.com/modpack/fabulously-optimized/version/abc123"),
        ImportInput::VersionId {
            slug: "fabulously-optimized".to_string(),
            version_id: "abc123".to_string(),
        }
    );
}

#[test]
fn parse_local_mrpack() {
    assert_eq!(
        parse_import_input("/home/user/pack.mrpack"),
        ImportInput::LocalFile("/home/user/pack.mrpack".to_string())
    );
}

#[test]
fn parse_local_zip() {
    assert_eq!(
        parse_import_input("GT_New_Horizons.zip"),
        ImportInput::LocalFile("GT_New_Horizons.zip".to_string())
    );
}

#[test]
fn parse_tilde_path() {
    assert_eq!(
        parse_import_input("~/Downloads/pack.mrpack"),
        ImportInput::LocalFile("~/Downloads/pack.mrpack".to_string())
    );
}

#[test]
fn parse_bare_slug() {
    assert_eq!(
        parse_import_input("fabulously-optimized"),
        ImportInput::ProjectSlug("fabulously-optimized".to_string())
    );
}

#[test]
fn parse_input_trims_whitespace() {
    assert_eq!(
        parse_import_input("  fabulously-optimized  "),
        ImportInput::ProjectSlug("fabulously-optimized".to_string())
    );
}

#[test]
fn failed_import_cleanup_removes_the_partial_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(tmp.path().join("instances"), tmp.path().join("meta"));
    let instance_dir = manager.instances_dir.join("Broken");
    std::fs::create_dir_all(&instance_dir).unwrap();
    std::fs::write(instance_dir.join("partial"), b"data").unwrap();

    cleanup_failed_import(&manager, "Broken");

    assert!(!instance_dir.exists());
}

#[tokio::test]
async fn failed_import_creation_clears_progress_without_removing_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(tmp.path().join("instances"), tmp.path().join("meta"));
    let summary = ImportSummary {
        name: "../invalid".to_owned(),
        pack_version: String::new(),
        game_version: "1.20.1".to_owned(),
        loader: ModLoader::Vanilla,
        loader_version: None,
        mod_count: 0,
        override_count: 0,
        format: PackFormat::Mmc,
        archive_path: tmp.path().join("missing.zip"),
        source: None,
    };
    let preserved = tmp.path().join("invalid");
    std::fs::write(&preserved, b"keep").unwrap();

    assert!(execute_import(&summary, &manager).await.is_err());
    assert_eq!(std::fs::read(preserved).unwrap(), b"keep");
    assert!(
        !crate::feedback::progress::PROGRESS
            .lock()
            .unwrap()
            .current_action
            .as_deref()
            .is_some_and(|action| action.contains("../invalid"))
    );
}
