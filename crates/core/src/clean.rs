//! Erkennung und Bereinigung von Build-Artefakten und Caches (z. B. auf RAM-Laufwerken).
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Artifact {
    pub rel_path: String,
    pub kind: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoArtifacts {
    pub total_bytes: u64,
    pub items: Vec<Artifact>,
}

/// Rekursive Berechnung der Verzeichnisgröße in Bytes.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Findet typische Build-Ordner im gegebenen Repository.
pub fn find_artifacts(root: &Path) -> RepoArtifacts {
    let mut items = Vec::new();
    if !root.is_dir() {
        return RepoArtifacts::default();
    }

    // 1. Rust / Cargo: Hauptordner
    let target = root.join("target");
    if target.is_dir() && (root.join("Cargo.toml").exists() || target.join("debug").exists() || target.join("release").exists() || target.join("CACHEDIR.TAG").exists()) {
        let sz = dir_size(&target);
        if sz > 0 {
            items.push(Artifact {
                rel_path: "target".into(),
                kind: "Rust Cargo".into(),
                size_bytes: sz,
            });
        }
    }
    // Nested Cargo-Targets in Unterordnern
    find_nested_cargo(root, root, 0, &mut items);

    // 2. .NET / C# / F# (bin / obj / .vs)
    find_dotnet_artifacts(root, root, 0, &mut items);

    // 3. Node.js (dist, build, .next, out, node_modules)
    find_nodejs_artifacts(root, root, 0, &mut items);

    // 4. Python (__pycache__, .pytest_cache, .mypy_cache, .ruff_cache)
    find_python_artifacts(root, root, 0, &mut items);

    // 5. C/C++ CMake
    find_cmake_artifacts(root, root, 0, &mut items);

    let total_bytes = items.iter().map(|i| i.size_bytes).sum();
    RepoArtifacts { total_bytes, items }
}

fn find_nested_cargo(root: &Path, cur: &Path, depth: usize, out: &mut Vec<Artifact>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(cur) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" {
            continue;
        }
        if cur != root && name_str == "target" && (cur.join("Cargo.toml").exists() || p.join("debug").exists() || p.join("release").exists() || p.join("CACHEDIR.TAG").exists()) {
            let sz = dir_size(&p);
            if sz > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push(Artifact {
                    rel_path: rel,
                    kind: "Rust Cargo".into(),
                    size_bytes: sz,
                });
            }
            continue;
        }
        if name_str != "target" {
            find_nested_cargo(root, &p, depth + 1, out);
        }
    }
}

fn find_dotnet_artifacts(root: &Path, cur: &Path, depth: usize, out: &mut Vec<Artifact>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(cur) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        if name_str.eq_ignore_ascii_case("bin") || name_str.eq_ignore_ascii_case("obj") || name_str == ".vs" {
            let sz = dir_size(&p);
            if sz > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push(Artifact {
                    rel_path: rel,
                    kind: if name_str == ".vs" { "Visual Studio Cache".into() } else { ".NET bin/obj".into() },
                    size_bytes: sz,
                });
            }
            continue;
        }
        find_dotnet_artifacts(root, &p, depth + 1, out);
    }
}

fn find_nodejs_artifacts(root: &Path, cur: &Path, depth: usize, out: &mut Vec<Artifact>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(cur) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "target" {
            continue;
        }
        if name_str == "node_modules" || name_str == ".next" || name_str == ".nuxt" || name_str == ".output" {
            let sz = dir_size(&p);
            if sz > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push(Artifact {
                    rel_path: rel,
                    kind: if name_str == "node_modules" { "Node.js Modules" } else { "Next/Nuxt Build" }.into(),
                    size_bytes: sz,
                });
            }
            continue;
        }
        if (name_str == "dist" || name_str == "build" || name_str == "out") && cur.join("package.json").exists() {
            let sz = dir_size(&p);
            if sz > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push(Artifact {
                    rel_path: rel,
                    kind: "Node.js Build".into(),
                    size_bytes: sz,
                });
            }
            continue;
        }
        find_nodejs_artifacts(root, &p, depth + 1, out);
    }
}

fn find_python_artifacts(root: &Path, cur: &Path, depth: usize, out: &mut Vec<Artifact>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(cur) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" || name_str == "target" || name_str == "venv" || name_str == ".venv" {
            continue;
        }
        if name_str == "__pycache__" || name_str == ".pytest_cache" || name_str == ".mypy_cache" || name_str == ".ruff_cache" {
            let sz = dir_size(&p);
            if sz > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push(Artifact {
                    rel_path: rel,
                    kind: "Python Cache".into(),
                    size_bytes: sz,
                });
            }
            continue;
        }
        find_python_artifacts(root, &p, depth + 1, out);
    }
}

fn find_cmake_artifacts(root: &Path, cur: &Path, depth: usize, out: &mut Vec<Artifact>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(cur) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        if (name_str == "build" || name_str == "cmake-build-debug" || name_str == "cmake-build-release") && cur.join("CMakeLists.txt").exists() {
            let sz = dir_size(&p);
            if sz > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push(Artifact {
                    rel_path: rel,
                    kind: "CMake Build".into(),
                    size_bytes: sz,
                });
            }
            continue;
        }
        find_cmake_artifacts(root, &p, depth + 1, out);
    }
}

/// Prüft, ob eine Datei ein ausführbares Rust-Binary (direkt in debug/ oder release/) ist.
pub fn is_rust_binary_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let Some(parent) = path.parent() else { return false };
    let parent_name = parent.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if parent_name != "debug" && parent_name != "release" {
        return false;
    }

    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if file_name.starts_with('.') || file_name.ends_with(".lock") || file_name == "CACHEDIR.TAG" {
        return false;
    }

    if file_name.starts_with("build_script_") || file_name.starts_with("build-script-") {
        return false;
    }

    #[cfg(windows)]
    {
        path.extension().map(|ext| ext.eq_ignore_ascii_case("exe")).unwrap_or(false)
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let has_excluded_ext = path.extension().map(|ext| {
            let e = ext.to_string_lossy();
            e == "d" || e == "rlib" || e == "rmeta" || e == "json" || e == "txt"
        }).unwrap_or(false);

        if has_excluded_ext {
            return false;
        }

        path.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
}

/// Löscht einen spezifischen Pfad (Datei oder Verzeichnisbaum) robust auf Windows und gibt die freigegebenen Bytes zurück.
pub fn clean_path(p: &Path) -> std::io::Result<u64> {
    clean_path_opt(p, false)
}

pub fn clean_path_opt(p: &Path, keep_rust_binaries: bool) -> std::io::Result<u64> {
    if !p.exists() {
        return Ok(0);
    }
    if p.is_file() {
        if keep_rust_binaries && is_rust_binary_file(p) {
            return Ok(0);
        }
        let sz = p.metadata().map(|m| m.len()).unwrap_or(0);
        if let Ok(m) = p.metadata() {
            let mut perm = m.permissions();
            if perm.readonly() {
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(p, perm);
            }
        }
        std::fs::remove_file(p)?;
        return Ok(sz);
    }
    let mut freed = 0u64;
    clean_dir_recursive(p, keep_rust_binaries, &mut freed)?;
    let _ = std::fs::remove_dir(p);
    Ok(freed)
}

fn clean_dir_recursive(dir: &Path, keep_rust_binaries: bool, freed: &mut u64) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                let _ = clean_dir_recursive(&path, keep_rust_binaries, freed);
                let _ = std::fs::remove_dir(&path);
            } else {
                if keep_rust_binaries && is_rust_binary_file(&path) {
                    continue;
                }
                let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if let Ok(m) = entry.metadata() {
                    let mut perm = m.permissions();
                    if perm.readonly() {
                        perm.set_readonly(false);
                        let _ = std::fs::set_permissions(&path, perm);
                    }
                }
                match std::fs::remove_file(&path) {
                    Ok(_) => *freed += sz,
                    Err(_) => {
                        // Bei gesperrter Datei (z. B. laufende EXE) ignorieren wir den Einzelfehler
                    }
                }
            }
        }
    }
    Ok(())
}

/// Löscht alle erkannten Build-Artefakte im Repository und gibt die Gesamtzahl freigegebener Bytes zurück.
pub fn clean_repo_artifacts(root: &Path) -> std::io::Result<u64> {
    clean_repo_artifacts_opt(root, false)
}

/// Löscht Build-Artefakte mit optionalem Erhalt von Rust-Binaries (`clean_keep_binary`).
pub fn clean_repo_artifacts_opt(root: &Path, keep_binary: bool) -> std::io::Result<u64> {
    let is_rust = root.join("Cargo.toml").exists();
    let keep_rust_binaries = keep_binary && is_rust;

    let artifacts = find_artifacts(root);
    let mut freed = 0u64;
    for item in artifacts.items {
        let target_path = root.join(&item.rel_path);
        if target_path.exists() {
            let should_keep = keep_rust_binaries && item.kind == "Rust Cargo";
            if let Ok(sz) = clean_path_opt(&target_path, should_keep) {
                freed += sz;
            }
        }
    }
    Ok(freed)
}

/// Formatierung von Byte-Größen (z. B. "4.5 GB", "120 MB", "450 KB").
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        "–".into()
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Gibt freien und gesamten Speicherplatz für den Pfad zurück: `(free_bytes, total_bytes)`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut free_avail: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        extern "system" {
            fn GetDiskFreeSpaceExW(
                lpDirectoryName: *const u16,
                lpFreeBytesAvailableToCaller: *mut u64,
                lpTotalNumberOfBytes: *mut u64,
                lpTotalNumberOfFreeBytes: *mut u64,
            ) -> i32;
        }
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_avail,
                &mut total,
                &mut total_free,
            )
        };
        if ok != 0 {
            Some((free_avail, total))
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn format_bytes_works() {
        assert_eq!(format_bytes(0), "–");
        assert_eq!(format_bytes(500 * 1024), "500 KB");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50.0 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    #[test]
    fn find_and_clean_cargo_artifacts() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("output.bin"), vec![0u8; 10000]).unwrap();

        let arts = find_artifacts(root);
        assert_eq!(arts.items.len(), 1);
        assert_eq!(arts.items[0].rel_path, "target");
        assert_eq!(arts.items[0].size_bytes, 10000);
        assert_eq!(arts.total_bytes, 10000);

        let freed = clean_repo_artifacts(root).unwrap();
        assert_eq!(freed, 10000);
        assert!(!target.exists());
        assert!(root.join("Cargo.toml").exists());
    }

    #[test]
    fn clean_cargo_artifacts_keeping_binary() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let target_debug = root.join("target").join("debug");
        let deps = target_debug.join("deps");
        std::fs::create_dir_all(&deps).unwrap();

        let bin_name = if cfg!(windows) { "demo.exe" } else { "demo" };
        let bin_path = target_debug.join(bin_name);
        std::fs::write(&bin_path, vec![1u8; 8000]).unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&bin_path).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&bin_path, p).unwrap();
        }

        std::fs::write(deps.join("libdemo.rlib"), vec![2u8; 12000]).unwrap();

        // 1. Clean with keep_binary = true
        let freed = clean_repo_artifacts_opt(root, true).unwrap();
        assert_eq!(freed, 12000); // rlib freed, binary preserved!
        assert!(bin_path.exists());
        assert!(!deps.exists());

        // 2. Clean with keep_binary = false
        let freed_full = clean_repo_artifacts_opt(root, false).unwrap();
        assert_eq!(freed_full, 8000);
        assert!(!bin_path.exists());
        assert!(!root.join("target").exists());
    }

    #[test]
    fn find_and_clean_dotnet_artifacts() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let proj = root.join("src").join("App");
        let bin = proj.join("bin");
        let obj = proj.join("obj");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&obj).unwrap();
        std::fs::write(bin.join("app.exe"), vec![0u8; 5000]).unwrap();
        std::fs::write(obj.join("cache.dat"), vec![0u8; 2000]).unwrap();

        let arts = find_artifacts(root);
        assert_eq!(arts.items.len(), 2);
        assert_eq!(arts.total_bytes, 7000);

        let freed = clean_repo_artifacts(root).unwrap();
        assert_eq!(freed, 7000);
        assert!(!bin.exists());
        assert!(!obj.exists());
    }
}
