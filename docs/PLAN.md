# RepoOrganizer – Projektplan

Rust + Slint (Style **fluent**) · Git über **CLI** · Config als **TOML**, zwischen Windows/Linux **teilbar**.

## 1. Kernkonzept

- **Repository** = logische Einheit (Name, Remote-URL, Tags, Notizen).
- **Standort (Location)** = lokaler Klon mit **Pfaden je Betriebssystem**.
- Ein Repo hat beliebig viele Standorte. Nicht erreichbare Standorte (Laufwerk fehlt, Pfad eines anderen OS) sind ein normaler Zustand, kein Fehler.

## 2. Datenmodell

```text
Repo       { id, name, remote_url, tags[], notes, favorite }
Location   { id, repo_id, label, paths: { windows?, linux?, macos? }, role, auto_fetch }
             role = primary | backup | mirror | worktree
Snapshot   { location_id, last_checked, last_fetch, last_pull, branch, ahead, behind,
             dirty_files, untracked, stash_count, head_sha, state }   // lokaler Cache
Activity   { repo_id, date, commits, authors }                        // Statistik-Cache
```

**Pfad-Auflösung:** Zur Laufzeit wird der Pfad des aktuellen OS gewählt; Pfade anderer Systeme werden gespeichert und ausgegraut angezeigt („auf anderem System"). Optionale Pfad-Mappings (`R:\` ↔ `/mnt/r/`).

**Status:** `Unavailable` · `NotARepo` · `UpToDate` · `Behind(n)` · `Ahead(n)` · `Diverged` · `Dirty` · `Error`

## 3. Config teilen (TOML)

- `repos.toml` (**geteilt**, versionierbar): Repos, Standorte, Pfade je OS, Tags, Mappings. Ablageort wählbar (z. B. `R:\git\RepoOrganizer-config` oder eigenes Git-Repo); Pfad merkt sich eine kleine lokale Bootstrap-Datei im OS-Config-Verzeichnis.
- `local.toml` (**pro Rechner**, nicht geteilt): Fenster, Theme, Git-Pfad, Intervalle, Config-Ort.
- Cache (Snapshots/Statistik): pro Rechner, JSON, nicht geteilt.
- Stabile Reihenfolge der Schlüssel/Sortierung beim Schreiben (saubere Diffs), atomares Schreiben (Temp-Datei + Rename), `version`-Feld für Migrationen. Änderung durch anderen Rechner wird erkannt (mtime/Watcher) und neu geladen.

## 4. Technik

| Bereich | Wahl |
|---|---|
| UI | `slint` (Style `fluent`, Dark/Light) |
| Git | Git-CLI via `std::process::Command` (nutzt Credential Manager/SSH), Backend-Trait `GitBackend` |
| Async | `tokio` + `slint::invoke_from_event_loop` |
| Persistenz | `serde`, `toml` (bzw. `toml_edit` zum Erhalt von Kommentaren), `serde_json` Cache |
| Pfade | `directories`, `camino` |
| Scan | `walkdir`/`ignore` |
| Fehler/Log | `anyhow`, `thiserror`, `tracing` |
| Watch | `notify` |

> [!IMPORTANT]
> Erreichbarkeits-Checks mit Timeout in eigenem Thread (Netzlaufwerke, tote Mounts), sonst friert die UI ein.

## 5. Architektur

```text
RepoOrganizer/
├─ Cargo.toml          (Workspace)
├─ crates/
│  ├─ core/   Modelle, Pfad-Auflösung, Config, Status-/Drift-/Health-Logik (UI-frei, testbar)
│  ├─ git/    CLI-Wrapper, Parser für status/log/rev-list
│  └─ app/    Slint-UI, Controller, Worker
│     └─ ui/*.slint
└─ docs/
```

Schichten: UI ⇄ Controller ⇄ Services (Repo/Git/Scan/Stats/Health) ⇄ Store (Config/Cache).

## 6. Oberfläche (fluent, großzügig, detailreich)

Sidebar · Hauptbereich · Detailbereich · Task-Panel unten.

1. **Dashboard:** Kacheln (Repos, Standorte online/offline, Behind/Ahead/Dirty, Drift, Health-Warnungen). Repo-Karten mit Status-Badges pro Standort, Suche, Filter, Sortierung.
2. **Smarte Ansichten** (Sidebar, gespeicherte Filter): „Heute geändert", „Unpushed", „Behind", „Dirty", „Veraltet (> X Tage ohne Pull)", „Drift", „Offline", „Favoriten", plus eigene Filter (Tags/Status/Zeit).
3. **Repo-Detail:** Kopf (Name, Remote, Tags, Favorit, Notizen); Standorte mit OS-gerechtem Pfad, Online-Badge, Branch, ↑/↓, Dirty, „zuletzt gepullt vor …", Aktionen. Tabs: Änderungen (Stage/Commit) · Verlauf · Branches · Statistik · Health · Einstellungen.
4. **Statistik:** Commits/Woche/Monat, Heatmap, Top-Autoren, Zeilen +/-, Größe, letzte Aktivität (eigene Slint-Chart-Komponenten).
5. **Scan-Assistent:** Wurzelordner wählen → Fund-Liste (neu / bekannter Standort / Duplikat) → Auswahl übernehmen.
6. **Standort hinzufügen / Klonen:** vorhandenen Ordner verknüpfen oder `git clone` mit Fortschritt; Pfade je OS eintragen.
7. **Einstellungen:** Refresh, Git-Pfad, Theme, Config-Ort, Mappings, Sprache.

## 7. Git-Aktionen

Fetch · Pull (Default ff-only; Option rebase/merge) · Push · Commit · Stage/Unstage · Stash · Branch wechseln · Clone · Alle Standorte eines Repos syncen · Bulk-Aktionen. Bestätigung bei destruktiven Aktionen; Pull/Push bei `Diverged`/`Dirty` mit Hinweis blockiert.

Status via `git status --porcelain=v2 --branch`, `git rev-list --left-right --count`, `.git/FETCH_HEAD`-mtime, Reflog, `git log`.

## 8. Feature-Details v1

### Scanfunktion
- Rekursiv ab Wurzelordnern (konfigurierbar, Tiefe, Ausschlüsse wie `node_modules`, `target`), erkennt `.git` (Ordner/Datei für Worktrees/Submodule).
- Liest `origin`-URL, **normalisiert** (ssh/https, `.git`-Suffix, Groß-/Kleinschreibung) und ordnet automatisch dem passenden Repo als weiteren Standort zu; unbekannte → „neues Repo vorschlagen".
- Läuft im Hintergrund mit Fortschritt, abbrechbar, Timeout für Netzpfade.

### Drift-Warnung
- Vergleicht alle erreichbaren Standorte eines Repos pro Branch (HEAD-SHA, ahead/behind zueinander via lokalem `rev-list`/Fetch zwischen Standorten).
- Meldungen: „Standort B ist 12 Commits hinter A", „A und B divergiert", „Dirty-Standort verhindert Sync".
- Offline-Standorte werden mit letztem bekanntem Snapshot verglichen (als „veraltet" gekennzeichnet).

### Health-Checks (pro Repo/Standort, einzeln abschaltbar)
- Ungepushte Commits / Branches ohne Upstream
- Lange kein Fetch/Pull/Commit (Schwellwert einstellbar)
- Uncommittete Änderungen sehr alt
- Große Dateien (> Schwellwert), `.git`-Größe/`git count-objects`
- Fehlende `.gitignore`/README
- Detached HEAD, laufender Merge/Rebase, Konflikte
- Remote nicht erreichbar / fehlt
- Optional: Secret-Scan (gitleaks, falls installiert)
- Ergebnis: Ampel (OK/Warnung/Fehler) + Handlungsempfehlung.

### Smarte Ansichten
Regelbasierte Filter auf Snapshot/Health/Tags; Zähler-Badges in der Sidebar; eigene Ansichten speicherbar (in `repos.toml`, damit geteilt).

## 9. Meilensteine

1. **M0** Workspace, Slint-Hello-World (fluent), CI-Layout
2. **M1** Core: Modell, TOML-Config (geteilt/lokal), Pfadauflösung, Erreichbarkeit mit Timeout, Tests
3. **M2** Git lesen: Status, Ahead/Behind, Last-Fetch/Pull, parallel
4. **M3** UI: Dashboard + Detail + Standorte
5. **M4** Aktionen: Fetch/Pull/Push/Commit + Task-Panel
6. **M5** Scan-Assistent, Standort hinzufügen/Klonen
7. **M6** Drift-Erkennung, Health-Checks, Smarte Ansichten
8. **M7** Statistik/Charts
9. **M8** Politur: Shortcuts, Tray, Packaging, Linux-Test

## 10. Spätere Ideen

Offline-Queue/Hotplug-Sync · „Öffnen mit …" (Explorer/Terminal/VS Code) · GitHub/GitLab-Infos · Tray-Benachrichtigungen · CLI-Modus (`repoorganizer status`) · DE/EN via `@tr()`.

## 11. Entscheidungen

| Thema | Entscheidung |
|---|---|
| Git | CLI |
| Config | TOML, teilbar (`repos.toml`) + lokal (`local.toml`) |
| UI-Style | Slint `fluent` |
| Name | RepoOrganizer |
| v1-Features | Scan, Drift, Health, Smarte Ansichten, teilbare Config |
| Build-Cleanup | Automatische Erkennung & Bereinigung von Build-Artefakten (`target`, `bin`/`obj`, `node_modules`, `build` etc.) mit RAM-Disk-Freispeicher-Überwachung |
