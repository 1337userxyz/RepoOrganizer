use crate::config::*;
use crate::model::*;
use crate::paths::*;
use crate::reach::{check, Reachability};
use std::time::Duration;

fn loc(win: Option<&str>, lin: Option<&str>) -> Location {
    Location {
        id: "l1".into(),
        repo_id: "r1".into(),
        label: String::new(),
        paths: OsPaths { path: None, windows: win.map(Into::into), linux: lin.map(Into::into), macos: None },
        role: Role::Primary,
        auto_fetch: false,
    }
}

#[test]
fn resolve_direct() {
    let l = loc(Some(r"R:\git\x"), Some("/home/u/x"));
    assert_eq!(resolve(&l, Os::Windows, &[]), Resolved::Local(r"R:\git\x".into()));
    assert_eq!(resolve(&l, Os::Linux, &[]), Resolved::Local("/home/u/x".into()));
}

#[test]
fn resolve_other_os_without_mapping() {
    assert_eq!(resolve(&loc(Some(r"R:\git\x"), None), Os::Linux, &[]), Resolved::OtherOs);
}

#[test]
fn resolve_with_mapping_both_directions() {
    let m = [PathMapping { windows: r"R:\".into(), unix: "/mnt/r/".into() }];
    assert_eq!(resolve(&loc(Some(r"R:\git\x"), None), Os::Linux, &m), Resolved::Local("/mnt/r/git/x".into()));
    assert_eq!(resolve(&loc(None, Some("/mnt/r/git/x")), Os::Windows, &m), Resolved::Local(r"R:\git\x".into()));
}

#[test]
fn resolve_auto_map_existing_directory() {
    let l = loc(Some(r"R:\git\RepoOrganizer"), None);
    if std::path::Path::new("/r/git/RepoOrganizer").exists() {
        assert_eq!(resolve(&l, Os::Linux, &[]), Resolved::Local("/r/git/RepoOrganizer".into()));
    }
}

fn sample() -> SharedConfig {
    let mut c = SharedConfig::default();
    c.repos = vec![
        Repo { id: "b".into(), name: "B".into(), remote_url: "git@h:b.git".into(), tags: vec!["z".into(), "a".into()], notes: String::new(), favorite: true, clean_keep_binary: false },
        Repo { id: "a".into(), name: "A".into(), remote_url: String::new(), tags: vec![], notes: "n".into(), favorite: false, clean_keep_binary: false },
    ];
    c.locations = vec![loc(Some(r"R:\git\a"), Some("/mnt/r/git/a"))];
    c.locations[0].repo_id = "a".into();
    c.mappings = vec![PathMapping { windows: r"R:\".into(), unix: "/mnt/r/".into() }];
    c.views = vec![SavedView { name: "Alt".into(), tags: vec![], status: vec!["behind".into()], older_than_days: Some(30) }];
    c
}

#[test]
fn shared_roundtrip_sorted_and_stable() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repos.toml");
    sample().save(&p).unwrap();
    let first = std::fs::read_to_string(&p).unwrap();
    let loaded = SharedConfig::load(&p).unwrap();
    assert_eq!(loaded.repos[0].id, "a");
    assert_eq!(loaded.repos[1].tags, vec!["a", "z"]);
    assert_eq!(loaded.locations_of("a").count(), 1);
    loaded.save(&p).unwrap();
    assert_eq!(first, std::fs::read_to_string(&p).unwrap());
    assert!(!dir.path().join("repos.toml.tmp").exists());
}

#[test]
fn shared_too_new_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repos.toml");
    std::fs::write(&p, "version = 99\n").unwrap();
    assert!(matches!(SharedConfig::load(&p), Err(ConfigError::TooNew { .. })));
}

#[test]
fn local_defaults_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("local.toml");
    assert_eq!(LocalConfig::load(&p).unwrap(), LocalConfig::default());
    let mut c = LocalConfig::default();
    c.theme = "dark".into();
    c.save(&p).unwrap();
    assert_eq!(LocalConfig::load(&p).unwrap(), c);
    assert_eq!(c.shared_path(&p), p.with_file_name("repos.toml"));
}

#[test]
fn reachability() {
    let dir = tempfile::tempdir().unwrap();
    let t = Duration::from_secs(2);
    assert_eq!(check(dir.path(), t), Reachability::NotARepo);
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    assert_eq!(check(dir.path(), t), Reachability::Repo);
    assert_eq!(check(&dir.path().join("nope"), t), Reachability::Unavailable);
}

mod m6 {
    use crate::cache;
    use crate::config::SavedView;
    use crate::drift::*;
    use crate::health::*;
    use crate::level::Level;
    use crate::model::*;
    use crate::views::*;

    fn dl(label: &str, sha: &str, dirty: bool, reachable: bool) -> DriftLoc {
        DriftLoc { label: label.into(), reachable, stale_at: Some(0), branch: Some("main".into()), head_sha: Some(sha.into()), dirty }
    }

    #[test]
    fn drift_messages_cases() {
        let now = 3 * 86400;
        // gleich
        assert!(drift_messages(&[dl("A", "1", false, true), dl("B", "1", false, true)], &mut |_, _| None, now).is_empty());
        // B hinter A, B dirty
        let m = drift_messages(&[dl("A", "1", false, true), dl("B", "2", true, true)], &mut |_, _| Some((12, 0)), now);
        assert_eq!(m[0].text, "B ist 12 Commits hinter A");
        assert_eq!(m[1].text, "Dirty-Standort B verhindert Sync");
        // divergiert
        let m = drift_messages(&[dl("A", "1", false, true), dl("B", "2", false, true)], &mut |_, _| Some((1, 2)), now);
        assert_eq!((m[0].level, m[0].text.as_str()), (Level::Error, "A und B divergiert"));
        // offline
        let m = drift_messages(&[dl("A", "1", false, true), dl("B", "2", false, false)], &mut |_, _| None, now);
        assert!(m[0].text.contains("offline") && m[0].text.contains("3 Tagen"));
        // Branches
        let mut b = dl("B", "2", false, true);
        b.branch = Some("dev".into());
        let m = drift_messages(&[dl("A", "1", false, true), b], &mut |_, _| None, now);
        assert!(m[0].text.contains("verschiedenen Branches"));
    }

    fn healthy() -> Facts {
        Facts { branch: Some("main".into()), has_upstream: true, has_remote: true, last_fetch: Some(1_000_000), has_gitignore: true, has_readme: true, remote_ok: Some(true), ..Default::default() }
    }

    #[test]
    fn health_ok_and_findings() {
        let now = 1_000_100;
        let s = HealthSettings::default();
        assert!(evaluate(&healthy(), &s, now).is_empty());
        let mut f = healthy();
        f.ahead = 2;
        f.in_progress = Some("Rebase".into());
        f.large_files = vec![("big.bin".into(), 200 * 1024 * 1024)];
        f.last_fetch = Some(now - 40 * 86400);
        f.detached = true;
        let r = evaluate(&f, &s, now);
        let checks: Vec<_> = r.iter().map(|x| x.check).collect();
        for c in ["unpushed", "in_progress", "large_files", "stale_fetch", "detached"] {
            assert!(checks.contains(&c), "{c}");
        }
        assert_eq!(overall(&r), Level::Error);
        // abschaltbar
        let mut s2 = HealthSettings::default();
        s2.disabled = vec!["unpushed".into(), "in_progress".into()];
        assert!(!evaluate(&f, &s2, now).iter().any(|x| x.check == "unpushed" || x.check == "in_progress"));
    }

    fn ctx<'a>(r: &'a Repo, statuses: Vec<Status>) -> RepoCtx<'a> {
        RepoCtx { repo: r, statuses, last_sync: Some(0), last_commit: None, drift: false, offline: false, health: Level::Ok, has_build_artifacts: false, now: 100 * 86400 }
    }

    #[test]
    fn views_match() {
        let r = Repo { id: "r".into(), name: "r".into(), remote_url: String::new(), tags: vec!["Work".into(), "rust".into()], notes: String::new(), favorite: true, clean_keep_binary: false };
        let c = ctx(&r, vec![Status::Behind(2), Status::UpToDate]);
        assert!(keyword_matches("behind", &c, 30));
        assert!(!keyword_matches("dirty", &c, 30));
        assert!(keyword_matches("stale", &c, 30));
        assert!(keyword_matches("favorite", &c, 30));
        let v = |tags: &[&str], st: &[&str], d| SavedView { name: "v".into(), tags: tags.iter().map(|s| s.to_string()).collect(), status: st.iter().map(|s| s.to_string()).collect(), older_than_days: d };
        assert!(view_matches(&v(&["work"], &["dirty", "behind"], None), &c, 30));
        assert!(!view_matches(&v(&["work", "go"], &[], None), &c, 30));
        assert!(!view_matches(&v(&[], &[], Some(200)), &c, 30));
        assert!(view_matches(&v(&[], &[], Some(50)), &c, 30));
    }

    #[test]
    fn cache_roundtrip_and_merge() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c").join("snapshots.json");
        assert!(cache::load(&p).is_empty());
        let mut s = Snapshot::empty("l1", Status::UpToDate);
        s.head_sha = Some("abc".into());
        s.build_size_bytes = 1024 * 1024;
        s.dirty_files = 3;
        cache::save(&p, &[s.clone()]).unwrap();
        assert_eq!(cache::load(&p), vec![s.clone()]);
        let merged = cache::merge(Some(&s), Snapshot::empty("l1", Status::Unavailable));
        assert_eq!(merged.head_sha.as_deref(), Some("abc"));
        assert_eq!(merged.state, Status::Unavailable);
        assert_eq!(merged.build_size_bytes, 0);
        assert_eq!(merged.dirty_files, 0);
        let fresh = cache::merge(Some(&s), Snapshot::empty("l1", Status::NotARepo));
        assert_eq!(fresh.state, Status::NotARepo);
    }
}
