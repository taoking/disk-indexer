use std::fs;

use disk_indexer::config::Config;
use disk_indexer::db::Database;
use disk_indexer::duplicate::{DuplicateFilter, duplicate_groups, lookup};
use disk_indexer::model::VolumeRole;
use disk_indexer::report::{create_cleanup_plan, duplicate_report, write_csv};
use disk_indexer::scanner::{ScanOptions, scan};
use disk_indexer::volume::{MarkerPolicy, register_volume};
use tempfile::tempdir;

#[test]
fn indexes_duplicates_incrementally_and_keeps_offline_history() {
    let temp = tempdir().expect("temporary root");
    let volume_a = temp.path().join("volume-a");
    let volume_b = temp.path().join("volume-b");
    let volume_c = temp.path().join("volume-c");
    fs::create_dir_all(&volume_a).expect("volume a");
    fs::create_dir_all(&volume_b).expect("volume b");
    fs::create_dir_all(&volume_c).expect("volume c");
    fs::write(volume_a.join("photo1.raw"), b"same bytes").expect("photo a");
    fs::write(volume_a.join("same-size-different"), b"different!").expect("different file");
    fs::write(volume_b.join("renamed.raw"), b"same bytes").expect("photo b");
    fs::write(volume_c.join("copy.raw"), b"same bytes").expect("photo c");

    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    assert_eq!(database.schema_version().expect("schema"), 1);
    let a = register_volume(
        &mut database,
        &volume_a,
        VolumeRole::Primary,
        MarkerPolicy::WriteIfPossible,
    )
    .expect("register a")
    .volume;
    let a_again = register_volume(
        &mut database,
        &volume_a,
        VolumeRole::Primary,
        MarkerPolicy::WriteIfPossible,
    )
    .expect("same volume identity")
    .volume;
    assert_eq!(a.id, a_again.id);
    let collision_root = temp.path().join("marker-collision");
    fs::create_dir_all(&collision_root).expect("collision root");
    fs::copy(
        volume_a.join(".disk-indexer-volume-id"),
        collision_root.join(".disk-indexer-volume-id"),
    )
    .expect("copy marker for collision test");
    assert!(
        register_volume(
            &mut database,
            &collision_root,
            VolumeRole::Unknown,
            MarkerPolicy::WriteIfPossible,
        )
        .is_err()
    );
    let b = register_volume(
        &mut database,
        &volume_b,
        VolumeRole::LegacyBackup,
        MarkerPolicy::WriteIfPossible,
    )
    .expect("register b")
    .volume;
    let c = register_volume(
        &mut database,
        &volume_c,
        VolumeRole::LocalBackup,
        MarkerPolicy::WriteIfPossible,
    )
    .expect("register c")
    .volume;

    assert_eq!(
        scan(&mut database, &config, &a, &ScanOptions::default())
            .expect("scan a")
            .full_hashed_count,
        0
    );
    assert!(
        duplicate_groups(&database, DuplicateFilter::default())
            .expect("sample-only groups")
            .is_empty()
    );
    assert_eq!(
        scan(&mut database, &config, &b, &ScanOptions::default())
            .expect("scan b")
            .full_hashed_count,
        2
    );
    assert_eq!(
        scan(&mut database, &config, &c, &ScanOptions::default())
            .expect("scan c")
            .full_hashed_count,
        1
    );

    let groups = duplicate_groups(&database, DuplicateFilter::default()).expect("duplicates");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].known_copies, 3);
    assert_eq!(groups[0].online_copies, 3);
    assert_eq!(groups[0].file_size, 10);
    assert_eq!(groups[0].theoretical_reclaimable_bytes, 20);
    let json = serde_json::to_value(duplicate_report(&database, &groups)).expect("json report");
    assert_eq!(json["schema_version"], 1);
    let csv_path = temp.path().join("duplicates.csv");
    write_csv(&csv_path, &groups).expect("csv report");
    assert!(
        fs::read_to_string(csv_path)
            .expect("read csv")
            .contains("full_hash")
    );

    let second_scan =
        scan(&mut database, &config, &a, &ScanOptions::default()).expect("second scan");
    assert_eq!(second_scan.metadata_reused_count, 2);
    assert_eq!(second_scan.full_hashed_count, 0);

    let result = lookup(&database, &config, &volume_b.join("renamed.raw"), false).expect("lookup");
    assert!(result.has_appeared_before);
    assert_eq!(result.known_copies, 3);
    assert!(result.full_hash.is_some());

    let blocked = create_cleanup_plan(&database, b.id, a.id, 3).expect("blocked plan");
    assert_eq!(blocked.items.len(), 1);
    assert_eq!(blocked.items[0].status, "blocked");

    let offline_path = temp.path().join("volume-c-offline");
    fs::rename(&volume_c, &offline_path).expect("temporarily detach c");
    database
        .refresh_volume_online_states()
        .expect("refresh state");
    let groups =
        duplicate_groups(&database, DuplicateFilter::default()).expect("offline duplicate report");
    assert_eq!(groups[0].known_copies, 3);
    assert_eq!(groups[0].offline_copies, 1);
    assert_eq!(groups[0].missing_copies, 0);
    fs::rename(offline_path, volume_c).expect("restore c");
    database
        .refresh_volume_online_states()
        .expect("restore online state");

    fs::write(volume_a.join("new-file"), b"new file").expect("new file");
    let added = scan(&mut database, &config, &a, &ScanOptions::default()).expect("scan new file");
    assert_eq!(added.discovered_count, 3);
    assert_eq!(added.metadata_reused_count, 2);
    fs::rename(volume_a.join("new-file"), volume_a.join("moved-file")).expect("move file");
    let moved = scan(&mut database, &config, &a, &ScanOptions::default()).expect("scan moved file");
    assert_eq!(moved.missing_count, 1);

    fs::write(volume_b.join("renamed.raw"), b"same-bytex").expect("same size content change");
    scan(&mut database, &config, &b, &ScanOptions::default()).expect("scan changed content");
    let changed = database
        .file_record_by_path(
            b.id,
            &disk_indexer::util::path_bytes(std::path::Path::new("renamed.raw")),
        )
        .expect("changed record")
        .expect("changed file exists");
    assert!(changed.full_hash.is_none());
    fs::remove_file(volume_b.join("renamed.raw")).expect("delete file");
    let deleted = scan(&mut database, &config, &b, &ScanOptions::default()).expect("scan deletion");
    assert_eq!(deleted.missing_count, 1);
}

#[test]
fn interrupted_scan_can_be_resumed() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let temp = tempdir().expect("temporary root");
    let volume_path = temp.path().join("volume");
    fs::create_dir_all(&volume_path).expect("volume");
    fs::write(volume_path.join("file"), b"data").expect("file");
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let volume = register_volume(
        &mut database,
        &volume_path,
        VolumeRole::Unknown,
        MarkerPolicy::WriteIfPossible,
    )
    .expect("registration")
    .volume;
    let cancelled = Arc::new(AtomicBool::new(true));
    let interrupted = scan(
        &mut database,
        &config,
        &volume,
        &ScanOptions {
            cancel_flag: Some(cancelled),
            ..ScanOptions::default()
        },
    )
    .expect("interrupted scan state");
    assert_eq!(interrupted.status, "interrupted");
    let resumed = scan(
        &mut database,
        &config,
        &volume,
        &ScanOptions {
            resume: true,
            ..ScanOptions::default()
        },
    )
    .expect("resume");
    assert_eq!(resumed.status, "completed");
    assert_eq!(resumed.discovered_count, 1);
}

#[cfg(unix)]
#[test]
fn scans_non_utf8_path_without_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = tempdir().expect("temporary root");
    let volume = temp.path().join("volume");
    fs::create_dir_all(&volume).expect("volume");
    let non_utf8_name = OsString::from_vec(vec![b'f', 0x80, b'o']);
    if let Err(error) = fs::write(volume.join(non_utf8_name), b"contents") {
        // APFS 的 POSIX 层会拒绝非法 UTF-8；Linux 等接受原始字节的文件系统继续覆盖。
        if error.raw_os_error() == Some(92) {
            return;
        }
        panic!("non utf8 file: {error}");
    }
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let registered = register_volume(
        &mut database,
        &volume,
        VolumeRole::Unknown,
        MarkerPolicy::WriteIfPossible,
    )
    .expect("registration")
    .volume;
    let summary = scan(&mut database, &config, &registered, &ScanOptions::default()).expect("scan");
    assert_eq!(summary.discovered_count, 1);
}
