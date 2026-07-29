use std::fs;

use disk_indexer::config::Config;
use disk_indexer::db::Database;
use disk_indexer::duplicate::{DuplicateFilter, duplicate_groups, duplicate_groups_page, lookup};
use disk_indexer::model::{Volume, VolumeRole};
use disk_indexer::report::{
    CleanupVerificationOptions, create_cleanup_plan, create_cleanup_plan_with_verification,
    duplicate_report, write_csv,
};
use disk_indexer::scanner::{ScanOptions, scan};
use disk_indexer::volume::{
    MarkerPolicy, VolumeIdentityInput, register_volume, register_volume_with_identity,
    resolve_conflict_as_new_volume,
};
use tempfile::tempdir;

fn registered(registration: disk_indexer::volume::VolumeRegistration) -> Volume {
    registration
        .volume
        .expect("注册结果应包含一个可安全使用的卷")
}

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
    assert_eq!(database.schema_version().expect("schema"), 4);
    let a = registered(
        register_volume(
            &mut database,
            &volume_a,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register a"),
    );
    let a_again = registered(
        register_volume(
            &mut database,
            &volume_a,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("same volume identity"),
    );
    assert_eq!(a.id, a_again.id);
    let b = registered(
        register_volume(
            &mut database,
            &volume_b,
            VolumeRole::LegacyBackup,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register b"),
    );
    let c = registered(
        register_volume(
            &mut database,
            &volume_c,
            VolumeRole::LocalBackup,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register c"),
    );

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
    let first_page = duplicate_groups_page(&database, DuplicateFilter::default(), 0, 1)
        .expect("first duplicate page");
    assert_eq!(first_page.groups.len(), 1);
    let empty_page = duplicate_groups_page(
        &database,
        DuplicateFilter::default(),
        first_page.next_after_content_id.expect("next cursor"),
        1,
    )
    .expect("second duplicate page");
    assert!(empty_page.groups.is_empty());
    assert!(empty_page.next_after_content_id.is_none());
    let stats = database.dashboard_stats().expect("dashboard stats");
    assert_eq!(stats["schema_version"], 4);
    assert_eq!(stats["trusted_duplicate_groups"], 1);
    assert_eq!(stats["theoretical_reclaimable_bytes"], 20);
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
    assert!(
        groups[0]
            .copies
            .iter()
            .any(|copy| copy.status == "offline_unverified")
    );
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
fn offline_marker_clone_is_never_merged_and_can_be_resolved_as_new_volume() {
    let temp = tempdir().expect("temporary root");
    let original_path = temp.path().join("original");
    let clone_path = temp.path().join("clone");
    fs::create_dir_all(&original_path).expect("original directory");
    fs::create_dir_all(&clone_path).expect("clone directory");
    let original_canonical = original_path.canonicalize().expect("canonical original");
    let clone_canonical = clone_path.canonicalize().expect("canonical clone");
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let original_identity = VolumeIdentityInput {
        filesystem: "apfs".to_owned(),
        system_volume_uuid: Some("volume-a".to_owned()),
        partition_uuid: Some("partition-a".to_owned()),
        media_uuid: Some("media-a".to_owned()),
        device_serial: Some("serial-a".to_owned()),
        model: Some("Test Disk".to_owned()),
        transport: Some("USB".to_owned()),
        total_size: Some(1_000),
    };
    let original = registered(
        register_volume_with_identity(
            &mut database,
            &original_path,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
            Some(original_identity),
        )
        .expect("register original"),
    );
    fs::copy(
        original_path.join(".disk-indexer-volume-id"),
        clone_path.join(".disk-indexer-volume-id"),
    )
    .expect("copy marker like a disk clone");
    let offline_path = temp.path().join("original-offline");
    fs::rename(&original_path, &offline_path).expect("detach original");
    database
        .refresh_volume_online_states()
        .expect("refresh offline state");

    let clone_registration = register_volume_with_identity(
        &mut database,
        &clone_path,
        VolumeRole::LocalBackup,
        MarkerPolicy::DoNotWrite,
        Some(VolumeIdentityInput {
            filesystem: "apfs".to_owned(),
            system_volume_uuid: Some("volume-b".to_owned()),
            partition_uuid: Some("partition-b".to_owned()),
            media_uuid: Some("media-b".to_owned()),
            device_serial: Some("serial-b".to_owned()),
            model: Some("Clone Disk".to_owned()),
            transport: Some("USB".to_owned()),
            total_size: Some(1_000),
        }),
    )
    .expect("clone registration returns a safe status");
    assert!(clone_registration.volume.is_none());
    let conflict = clone_registration
        .conflict
        .expect("possible clone conflict");
    assert_eq!(conflict.existing_volume_id, original.id);
    assert_eq!(
        database
            .volume_by_id(original.id)
            .expect("original volume")
            .mount_path,
        original_canonical
    );
    assert!(
        !database
            .volume_by_id(original.id)
            .expect("original volume")
            .is_online
    );
    assert_eq!(
        database
            .open_volume_conflicts()
            .expect("open conflicts")
            .len(),
        1
    );

    let resolved =
        resolve_conflict_as_new_volume(&mut database, conflict.id, VolumeRole::LocalBackup)
            .expect("resolve as a distinct volume");
    assert_ne!(resolved.id, original.id);
    assert_ne!(resolved.volume_uid, original.volume_uid);
    assert_eq!(resolved.marker_uid, original.marker_uid);
    assert_eq!(resolved.mount_path, clone_canonical);
    assert!(
        database
            .open_volume_conflicts()
            .expect("resolved conflicts")
            .is_empty()
    );
}

#[test]
fn matching_stable_identity_allows_an_offline_volume_to_relink() {
    let temp = tempdir().expect("temporary root");
    let first_path = temp.path().join("first-mount");
    let second_path = temp.path().join("second-mount");
    fs::create_dir_all(&first_path).expect("first mount");
    fs::create_dir_all(&second_path).expect("second mount");
    let second_canonical = second_path.canonicalize().expect("canonical second mount");
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let stable_identity = || VolumeIdentityInput {
        filesystem: "apfs".to_owned(),
        system_volume_uuid: Some("stable-volume".to_owned()),
        partition_uuid: Some("stable-partition".to_owned()),
        media_uuid: Some("stable-media".to_owned()),
        device_serial: Some("stable-serial".to_owned()),
        model: None,
        transport: None,
        total_size: Some(2_000),
    };
    let first = registered(
        register_volume_with_identity(
            &mut database,
            &first_path,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
            Some(stable_identity()),
        )
        .expect("first mount"),
    );
    fs::copy(
        first_path.join(".disk-indexer-volume-id"),
        second_path.join(".disk-indexer-volume-id"),
    )
    .expect("same logical volume marker");
    fs::rename(&first_path, temp.path().join("first-offline")).expect("offline first mount");
    let relinked = registered(
        register_volume_with_identity(
            &mut database,
            &second_path,
            VolumeRole::Primary,
            MarkerPolicy::DoNotWrite,
            Some(stable_identity()),
        )
        .expect("stable re-registration"),
    );
    assert_eq!(relinked.id, first.id);
    assert_eq!(relinked.mount_path, second_canonical);
    assert!(
        database
            .open_volume_conflicts()
            .expect("no conflict")
            .is_empty()
    );
}

#[test]
fn migration_backfills_physical_device_for_existing_volume_history() {
    let temp = tempdir().expect("temporary root");
    let database_path = temp.path().join("legacy.db");
    let connection = rusqlite::Connection::open(&database_path).expect("legacy database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("initial schema");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations(version TEXT PRIMARY KEY, applied_at TEXT NOT NULL);
             INSERT INTO schema_migrations(version, applied_at) VALUES ('0001_initial', '2026-01-01T00:00:00Z');",
        )
        .expect("legacy migration marker");
    connection
        .execute(
            "INSERT INTO volumes (
                volume_uid, marker_uid, volume_name, filesystem, mount_path, mount_path_display,
                system_volume_uuid, device_serial, partition_uuid, total_size, role, is_online,
                first_seen_at, last_seen_at, created_at, updated_at
             ) VALUES (?1, NULL, 'legacy', 'apfs', ?2, '/legacy', 'volume-legacy', 'media-legacy',
                       'partition-legacy', 4096, 'primary', 0, 't', 't', 't', 't')",
            rusqlite::params!["legacy:volume", b"/legacy".to_vec()],
        )
        .expect("legacy volume");
    drop(connection);

    let config = Config::new(Some(database_path)).expect("config");
    let database = Database::open(&config).expect("migrate legacy database");
    assert_eq!(database.schema_version().expect("schema version"), 4);
    let volume = database
        .volume_by_uid("legacy:volume")
        .expect("volume lookup")
        .expect("legacy volume");
    assert!(volume.physical_device_id.is_some());
    assert_eq!(volume.identity_state.as_str(), "verified");
}

#[test]
fn keyset_paging_reads_large_volume_without_offset_or_duplicates() {
    let temp = tempdir().expect("temporary root");
    let database_path = temp.path().join("paged.db");
    let config = Config::new(Some(database_path.clone())).expect("config");
    drop(Database::open(&config).expect("initialize schema"));
    let mut connection = rusqlite::Connection::open(&database_path).expect("open raw database");
    let transaction = connection.transaction().expect("bulk transaction");
    transaction
        .execute(
            "INSERT INTO volumes (
                volume_uid, volume_name, filesystem, mount_path, mount_path_display, role, is_online,
                physical_device_id, identity_state, first_seen_at, last_seen_at, created_at, updated_at
             ) VALUES ('page-volume', 'page-volume', 'test', ?1, '/page-volume', 'unknown', 1,
                       NULL, 'fallback', 't', 't', 't', 't')",
            rusqlite::params![b"/page-volume".to_vec()],
        )
        .expect("insert volume");
    let volume_id = transaction.last_insert_rowid();
    let mut insert = transaction
        .prepare(
            "INSERT INTO file_copies (
                volume_id, relative_path, relative_path_display, filename, filename_display, file_size, status,
                first_seen_at, last_seen_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?2, ?3, 1, 'present', 't', 't', 't', 't')",
        )
        .expect("prepare files");
    for index in 0..100_000_u32 {
        let name = format!("file-{index:06}");
        insert
            .execute(rusqlite::params![volume_id, name.as_bytes(), name])
            .expect("insert simulated file");
    }
    drop(insert);
    transaction.commit().expect("commit simulated files");
    drop(connection);

    let database = Database::open(&config).expect("open paged database");
    let mut after_id = 0;
    let mut count = 0usize;
    loop {
        let page = database
            .files_for_volume_page(volume_id, after_id, 1_000)
            .expect("page");
        let Some(last) = page.last() else {
            break;
        };
        assert!(page.windows(2).all(|window| window[0].id < window[1].id));
        assert!(last.id > after_id);
        after_id = last.id;
        count = count.saturating_add(page.len());
    }
    assert_eq!(count, 100_000);
}

#[test]
fn jsonl_scan_protocol_has_only_valid_events_and_persists_task_history() {
    use assert_cmd::Command;

    let temp = tempdir().expect("temporary root");
    let volume_path = temp.path().join("volume");
    fs::create_dir_all(&volume_path).expect("volume directory");
    fs::write(volume_path.join("file"), b"contents").expect("file");
    let database_path = temp.path().join("index.db");
    let output = Command::cargo_bin("disk-indexer")
        .expect("binary")
        .args([
            "--db",
            database_path.to_string_lossy().as_ref(),
            "scan",
            volume_path.to_string_lossy().as_ref(),
            "--metadata-only",
            "--jsonl-progress",
        ])
        .output()
        .expect("run JSONL scan");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout)
        .expect("utf8 JSONL")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid JSON line"))
        .collect::<Vec<_>>();
    assert!(events.len() >= 2);
    assert_eq!(events.first().expect("start")["type"], "task_started");
    assert_eq!(events.last().expect("end")["type"], "task_completed");
    let task_id = events.first().expect("start")["task_id"].clone();
    assert!(events.iter().all(|event| {
        event["protocol_version"] == 1
            && event["task_id"] == task_id
            && event["timestamp"].is_string()
    }));
    let config = Config::new(Some(database_path)).expect("config");
    let database = Database::open(&config).expect("open database");
    let tasks = database.task_runs_page(0, 10).expect("task history");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["status"], "completed");
}

#[test]
fn browser_ui_command_is_not_available() {
    use assert_cmd::Command;

    let output = Command::cargo_bin("disk-indexer")
        .expect("binary")
        .arg("ui")
        .output()
        .expect("run CLI");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
}

#[test]
fn lookup_marks_changed_cached_hash_as_stale_until_explicit_rehash() {
    let temp = tempdir().expect("temporary root");
    let volume_path = temp.path().join("volume");
    fs::create_dir_all(&volume_path).expect("volume directory");
    let file_path = volume_path.join("photo.raw");
    fs::write(&file_path, b"old-data").expect("initial file");
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let volume = registered(
        register_volume(
            &mut database,
            &volume_path,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register volume"),
    );
    scan(
        &mut database,
        &config,
        &volume,
        &ScanOptions {
            full_hash: true,
            ..ScanOptions::default()
        },
    )
    .expect("full scan");
    let valid = lookup(&database, &config, &file_path, false).expect("valid lookup");
    assert!(valid.exact);
    assert_eq!(valid.cache_state, "cache_valid");
    assert!(valid.metadata_matches_index);
    let previous_hash = valid.full_hash.expect("cached complete hash");

    let replacement = volume_path.join("replacement");
    fs::write(&replacement, b"new-data").expect("same-size replacement");
    fs::rename(&replacement, &file_path).expect("replace indexed path");
    let stale = lookup(&database, &config, &file_path, false).expect("stale lookup");
    assert!(!stale.exact);
    assert_eq!(stale.cache_state, "cache_stale");
    assert_eq!(stale.hash_state, "stale");
    assert!(stale.requires_rehash);
    assert!(stale.full_hash.is_none());

    let rehashed = lookup(&database, &config, &file_path, true).expect("rehash lookup");
    assert!(rehashed.exact);
    assert!(!rehashed.requires_rehash);
    assert_ne!(rehashed.full_hash.expect("current hash"), previous_hash);
}

#[cfg(unix)]
#[test]
fn hard_links_are_one_storage_object_and_never_cleanup_candidates() {
    let temp = tempdir().expect("temporary root");
    let volume_path = temp.path().join("volume");
    fs::create_dir_all(&volume_path).expect("volume directory");
    let original = volume_path.join("original.bin");
    let linked = volume_path.join("linked.bin");
    fs::write(&original, b"same storage object").expect("original file");
    fs::hard_link(&original, &linked).expect("hard link");
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let volume = registered(
        register_volume(
            &mut database,
            &volume_path,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register volume"),
    );
    scan(
        &mut database,
        &config,
        &volume,
        &ScanOptions {
            full_hash: true,
            ..ScanOptions::default()
        },
    )
    .expect("scan hard links");
    let groups = duplicate_groups(&database, DuplicateFilter::default()).expect("duplicate groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].path_count, 2);
    assert_eq!(groups[0].storage_object_count, 1);
    assert_eq!(groups[0].theoretical_reclaimable_bytes, 0);
    assert_eq!(groups[0].physical_device_count, 1);
    let plan = create_cleanup_plan(&database, volume.id, volume.id, 1).expect("cleanup plan");
    assert!(plan.items.iter().all(|item| item.status == "blocked"));
    assert!(plan.items.iter().all(|item| {
        item.blocked_reasons
            .iter()
            .any(|reason| reason.contains("硬链接"))
    }));
}

#[test]
fn duplicate_reports_exclude_nonpresent_statuses_and_cleanup_verifies_files() {
    let temp = tempdir().expect("temporary root");
    let volume_a = temp.path().join("volume-a");
    let volume_b = temp.path().join("volume-b");
    fs::create_dir_all(&volume_a).expect("volume a");
    fs::create_dir_all(&volume_b).expect("volume b");
    for name in ["one", "two", "three", "four"] {
        fs::write(volume_a.join(name), b"same bytes").expect("copy on volume a");
    }
    fs::write(volume_b.join("keeper"), b"same bytes").expect("keeper copy");
    let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
    let mut database = Database::open(&config).expect("database");
    let a = registered(
        register_volume(
            &mut database,
            &volume_a,
            VolumeRole::LocalBackup,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register a"),
    );
    let b = registered(
        register_volume(
            &mut database,
            &volume_b,
            VolumeRole::Primary,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("register b"),
    );
    for volume in [&a, &b] {
        scan(
            &mut database,
            &config,
            volume,
            &ScanOptions {
                full_hash: true,
                ..ScanOptions::default()
            },
        )
        .expect("full scan");
    }
    let changed = database
        .file_record_by_path(
            a.id,
            &disk_indexer::util::path_bytes(std::path::Path::new("two")),
        )
        .expect("changed record lookup")
        .expect("changed record");
    database
        .mark_hash_problem(changed.id, "simulated changed file", true)
        .expect("mark changed");
    let unreadable = database
        .file_record_by_path(
            a.id,
            &disk_indexer::util::path_bytes(std::path::Path::new("three")),
        )
        .expect("unreadable record lookup")
        .expect("unreadable record");
    database
        .mark_hash_problem(unreadable.id, "simulated unreadable file", false)
        .expect("mark unreadable");
    let groups = duplicate_groups(&database, DuplicateFilter::default()).expect("filtered report");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].known_copies, 3);
    assert!(groups[0].copies.iter().all(|copy| copy.status == "present"));

    let default_plan = create_cleanup_plan(&database, a.id, b.id, 1).expect("default plan");
    assert!(
        default_plan
            .items
            .iter()
            .all(|item| item.status == "candidate_unverified")
    );
    let verified_plan = create_cleanup_plan_with_verification(
        &mut database,
        &config,
        a.id,
        b.id,
        1,
        CleanupVerificationOptions {
            min_remaining_physical_devices: 1,
            verify_metadata: true,
            verify_full_hash: true,
        },
    )
    .expect("verified plan");
    assert!(
        verified_plan
            .items
            .iter()
            .all(|item| item.status == "verified_candidate")
    );

    fs::write(volume_b.join("keeper"), b"changed-ke").expect("change keeper without scan");
    let blocked_plan = create_cleanup_plan_with_verification(
        &mut database,
        &config,
        a.id,
        b.id,
        1,
        CleanupVerificationOptions {
            min_remaining_physical_devices: 1,
            verify_metadata: true,
            verify_full_hash: true,
        },
    )
    .expect("blocked verified plan");
    assert!(
        blocked_plan
            .items
            .iter()
            .all(|item| item.status == "blocked")
    );
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
    let volume = registered(
        register_volume(
            &mut database,
            &volume_path,
            VolumeRole::Unknown,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("registration"),
    );
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
    let registered = registered(
        register_volume(
            &mut database,
            &volume,
            VolumeRole::Unknown,
            MarkerPolicy::WriteIfPossible,
        )
        .expect("registration"),
    );
    let summary = scan(&mut database, &config, &registered, &ScanOptions::default()).expect("scan");
    assert_eq!(summary.discovered_count, 1);
}
