use super::*;
use repoorganizer_core::model::Status;
use std::process::Command;

#[test]
fn parse_status_full() {
    let out = "# branch.oid abc123\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n1 .M N... 100644 100644 100644 a b f.txt\n? new.txt\nu UU N... 1 1 1 1 a b c conflict.txt\n";
    let s = parse_status_v2(out);
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!((s.ahead, s.behind, s.changed, s.untracked, s.conflicts), (2, 3, 2, 1, 1));
    assert_eq!(s.head_sha.as_deref(), Some("abc123"));
}

#[test]
fn parse_detached_and_initial() {
    let s = parse_status_v2("# branch.oid (initial)\n# branch.head (detached)\n");
    assert!(s.branch.is_none() && s.head_sha.is_none());
}

#[test]
fn parse_counts_and_pull() {
    assert_eq!(parse::parse_left_right("4\t1\n"), Some((4, 1)));
    assert_eq!(parse::parse_left_right("x"), None);
    let log = "HEAD@{1730000500}|commit: x\nHEAD@{1730000100}|pull: Fast-forward\nHEAD@{1729000000}|pull origin: ff\n";
    assert_eq!(parse::parse_last_pull(log), Some(1730000100));
    assert_eq!(parse::parse_last_pull("HEAD@{1}|commit: x"), None);
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let st = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "protocol.file.allow=always"])
        .args(args)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

#[test]
fn snapshot_real_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    let clone = tmp.path().join("clone");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-b", "main"]);
    std::fs::write(origin.join("a.txt"), "1").unwrap();
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-m", "one"]);
    git(tmp.path(), &["clone", origin.to_str().unwrap(), clone.to_str().unwrap()]);

    let g = CliGit::default();
    let s = g.snapshot("c", &clone);
    assert_eq!(s.state, Status::UpToDate);
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert!(s.head_sha.is_some());
    assert!(s.last_fetch.is_none(), "frischer Clone hat noch kein FETCH_HEAD");

    // lokaler Commit -> Ahead
    std::fs::write(clone.join("b.txt"), "2").unwrap();
    git(&clone, &["add", "."]);
    git(&clone, &["commit", "-m", "two"]);
    assert_eq!(g.snapshot("c", &clone).state, Status::Ahead(1));

    // untracked -> Dirty
    std::fs::write(clone.join("u.txt"), "x").unwrap();
    let s = g.snapshot("c", &clone);
    assert_eq!(s.state, Status::Dirty);
    assert_eq!(s.untracked, 1);

    // Origin bekommt Commit, Clone fetcht -> Diverged (sobald sauber)
    std::fs::remove_file(clone.join("u.txt")).unwrap();
    std::fs::write(origin.join("c.txt"), "3").unwrap();
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-m", "three"]);
    git(&clone, &["fetch"]);
    assert!(g.snapshot("c", &clone).last_fetch.is_some());
    assert_eq!(g.snapshot("c", &clone).state, Status::Diverged);
    assert_eq!(g.left_right(&clone, "HEAD", "origin/main").unwrap(), (1, 1));

    // nicht vorhanden / kein Repo
    assert_eq!(g.snapshot("x", &tmp.path().join("nope")).state, Status::Unavailable);
    assert_eq!(g.snapshot("x", tmp.path()).state, Status::NotARepo);

    // parallel
    let items = vec![("a".to_string(), clone.clone()), ("b".to_string(), origin.clone()), ("c".to_string(), tmp.path().join("nope"))];
    let r = snapshots_parallel(&g, &items, 2);
    assert_eq!(r.iter().map(|s| s.location_id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
}



#[test]
fn guards() {
    assert!(guard_pull(&Status::Dirty).is_err());
    assert!(guard_pull(&Status::Diverged).is_err());
    assert!(guard_pull(&Status::Behind(2)).is_ok());
    assert!(guard_push(&Status::Dirty).is_ok());
    assert!(guard_push(&Status::Diverged).is_err());
    assert!(guard_push(&Status::Unavailable).is_err());
}

#[test]
fn actions_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "--bare", "-b", "main"]);
    git(tmp.path(), &["clone", origin.to_str().unwrap(), a.to_str().unwrap()]);
    let cfg = ["config", "user.name", "t"];
    git(&a, &cfg);
    git(&a, &["config", "user.email", "t@t"]);
    git(&a, &["config", "commit.gpgsign", "false"]);

    let g = CliGit::default();
    std::fs::write(a.join("f.txt"), "1").unwrap();
    assert!(g.commit(&a, "  ").is_err());
    g.stage_all(&a).unwrap();
    g.commit(&a, "first").unwrap();
    g.push(&a).unwrap_or_else(|_| g.run_all(&a, &["push", "-u", "origin", "HEAD"]).unwrap());
    git(tmp.path(), &["clone", origin.to_str().unwrap(), b.to_str().unwrap()]);
    git(&b, &["config", "user.name", "t"]);
    git(&b, &["config", "user.email", "t@t"]);
    git(&b, &["config", "commit.gpgsign", "false"]);

    // a committet + pusht, b fetcht und pullt ff-only
    std::fs::write(a.join("g.txt"), "2").unwrap();
    g.stage(&a, &["g.txt"]).unwrap();
    g.commit(&a, "second").unwrap();
    g.push(&a).unwrap();
    g.fetch(&b).unwrap();
    assert_eq!(g.snapshot("b", &b).state, Status::Behind(1));
    g.pull(&b, PullMode::FfOnly).unwrap();
    assert_eq!(g.snapshot("b", &b).state, Status::UpToDate);
    assert!(b.join("g.txt").exists());

    // Stash: untracked Datei verschwindet und kommt zurück
    std::fs::write(b.join("tmp.txt"), "x").unwrap();
    g.stash(&b).unwrap();
    assert!(!b.join("tmp.txt").exists());
    assert_eq!(g.stash_count(&b), 1);
    g.stash_pop(&b).unwrap();
    assert!(b.join("tmp.txt").exists());
    std::fs::remove_file(b.join("tmp.txt")).unwrap();

    // Divergenz: ff-only schlägt fehl
    std::fs::write(a.join("h.txt"), "3").unwrap();
    g.stage_all(&a).unwrap();
    g.commit(&a, "third").unwrap();
    g.push(&a).unwrap();
    std::fs::write(b.join("i.txt"), "4").unwrap();
    g.stage_all(&b).unwrap();
    g.commit(&b, "local").unwrap();
    g.fetch(&b).unwrap();
    assert_eq!(g.snapshot("b", &b).state, Status::Diverged);
    assert!(g.pull(&b, PullMode::FfOnly).is_err());
    g.pull(&b, PullMode::Rebase).unwrap();
    assert_eq!(g.snapshot("b", &b).state, Status::Ahead(1));
    g.push(&b).unwrap();
}

#[test]
fn scan_finds_repos_and_respects_limits() {
    use crate::scan::{scan, ScanOptions};
    use std::sync::atomic::AtomicBool;
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    let mk = |rel: &str| {
        let p = r.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        git(&p, &["init", "-b", "main"]);
        p
    };
    let a = mk("work/a");
    git(&a, &["remote", "add", "origin", "git@h:o/a.git"]);
    mk("work/nested/b");
    mk("work/node_modules/c");
    mk("deep/1/2/3/4/5/d");
    // Worktree-artige .git-Datei
    std::fs::create_dir_all(r.join("work/wt")).unwrap();
    std::fs::write(r.join("work/wt/.git"), "gitdir: /x").unwrap();
    // Repo in Repo wird nicht gefunden
    mk("work/a/inner");

    let opts = ScanOptions { roots: vec![r.to_path_buf()], ..Default::default() };
    let mut n = 0;
    let res = scan(&opts, &CliGit::default(), &AtomicBool::new(false), &mut |c, _| n = c);
    let names: Vec<_> = res.iter().map(|f| f.path.strip_prefix(r).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(names, ["work/a", "work/nested/b", "work/wt"]);
    assert_eq!(res[0].remote.as_deref(), Some("git@h:o/a.git"));
    assert!(res[1].remote.is_none());
    assert!(n > 3);

    let cancel = AtomicBool::new(true);
    assert!(scan(&opts, &CliGit::default(), &cancel, &mut |_, _| {}).is_empty());
}

#[test]
fn facts_and_compare() {
    use crate::facts::FactOptions;
    use repoorganizer_core::health::{evaluate, HealthSettings};
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "--bare", "-b", "main"]);
    git(tmp.path(), &["clone", origin.to_str().unwrap(), a.to_str().unwrap()]);
    for d in [&a] {
        git(d, &["config", "user.name", "t"]);
        git(d, &["config", "user.email", "t@t"]);
        git(d, &["config", "commit.gpgsign", "false"]);
    }
    let g = CliGit::default();
    std::fs::write(a.join("f.txt"), "1").unwrap();
    std::fs::write(a.join("big.bin"), vec![0u8; 2048]).unwrap();
    g.stage_all(&a).unwrap();
    g.commit(&a, "one").unwrap();
    g.run_all(&a, &["push", "-u", "origin", "HEAD"]).unwrap();

    let opts = FactOptions { check_remote: true, large_file_min_bytes: 1024, ..Default::default() };
    let f = g.collect_facts(&a, &opts);
    assert!(f.has_remote && f.has_upstream && !f.detached);
    assert_eq!(f.remote_ok, Some(true));
    assert_eq!(f.large_files.len(), 1);
    assert!(!f.has_gitignore && !f.has_readme);
    assert!(f.in_progress.is_none() && f.git_size_bytes > 0 && f.last_commit.is_some());
    let r = evaluate(&f, &HealthSettings::default(), f.last_commit.unwrap());
    let checks: Vec<_> = r.iter().map(|x| x.check).collect();
    assert!(checks.contains(&"no_gitignore") && checks.contains(&"no_readme"));

    // Branch ohne Upstream + uncommittete Datei
    git(&a, &["branch", "feature"]);
    std::fs::write(a.join("u.txt"), "x").unwrap();
    git(&a, &["add", "u.txt"]);
    let f = g.collect_facts(&a, &FactOptions::default());
    assert_eq!(f.branches_without_upstream, 1);
    assert!(f.oldest_dirty.is_some());

    // Remote verschwindet
    git(&a, &["remote", "set-url", "origin", tmp.path().join("nope.git").to_str().unwrap()]);
    let f = g.collect_facts(&a, &opts);
    assert_eq!(f.remote_ok, Some(false));

    // compare: b ist Klon von origin, a committet weiter
    git(&a, &["remote", "set-url", "origin", origin.to_str().unwrap()]);
    git(&a, &["reset", "--hard"]);
    git(tmp.path(), &["clone", origin.to_str().unwrap(), b.to_str().unwrap()]);
    for d in [&b] {
        git(d, &["config", "user.name", "t"]);
        git(d, &["config", "user.email", "t@t"]);
        git(d, &["config", "commit.gpgsign", "false"]);
    }
    assert_eq!(g.compare_locations(&a, &b), Some((0, 0)));
    std::fs::write(a.join("n.txt"), "n").unwrap();
    g.stage_all(&a).unwrap();
    g.commit(&a, "two").unwrap();
    assert_eq!(g.compare_locations(&a, &b), Some((1, 0)));
    assert_eq!(g.compare_locations(&b, &a), Some((0, 1)));
    std::fs::write(b.join("m.txt"), "m").unwrap();
    g.stage_all(&b).unwrap();
    g.commit(&b, "b-local").unwrap();
    assert_eq!(g.compare_locations(&a, &b), Some((1, 1)));
}

#[test]
fn parse_numstat_log() {
    let out = "\u{1}1700000000\u{1}Anna\n3\t1\ta.txt\n-\t-\timg.png\n\n\u{1}1700000100\u{1}Bob Builder\n2\t0\tb.txt\n";
    let c = parse::parse_log_numstat(out);
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].author.as_str(), c[0].added, c[0].deleted), ("Anna", 3, 1));
    assert_eq!((c[1].author.as_str(), c[1].added), ("Bob Builder", 2));
}

#[test]
fn activity_from_real_repo() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-b", "main"]);
    std::fs::write(tmp.path().join("a.txt"), "1\n2\n3\n").unwrap();
    git(tmp.path(), &["add", "."]);
    git(tmp.path(), &["commit", "-m", "one"]);
    std::fs::write(tmp.path().join("a.txt"), "1\n").unwrap();
    git(tmp.path(), &["commit", "-am", "two"]);
    let g = CliGit::default();
    let c = g.collect_activity(tmp.path(), 30);
    assert_eq!(c.len(), 2);
    assert_eq!(c.iter().map(|x| x.added).sum::<u64>(), 3);
    assert_eq!(c.iter().map(|x| x.deleted).sum::<u64>(), 2);
    assert_eq!(c[0].author, "t");
    assert!(g.git_size(tmp.path()) > 0);
    assert!(g.collect_activity(tmp.path().join("nope").as_path(), 30).is_empty());
}
