//! Central application state and the TUI event loop.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::config::Config;
use crate::distro::SystemProfile;
use crate::packages::{list_installed, Package, PkgSort};
use crate::scanner::mounts::{list_mounts, MountInfo};
use crate::scanner::{start_scan, ScanEntry, ScanEvent, ScanHandle};
use crate::targets::{build_registry, Category, CleanupTarget, RiskTier};
use crate::ui;
use crate::ui::theme::{self, Theme};

const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Cache,
    Packages,
    SysInfo,
    Log,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Overview, Tab::Cache, Tab::Packages, Tab::SysInfo, Tab::Log];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Cache => "Clean",
            Tab::Packages => "Packages",
            Tab::SysInfo => "System",
            Tab::Log => "Log",
        }
    }

    fn next(self) -> Tab {
        match self {
            Tab::Overview => Tab::Cache,
            Tab::Cache => Tab::Packages,
            Tab::Packages => Tab::SysInfo,
            Tab::SysInfo => Tab::Log,
            Tab::Log => Tab::Overview,
        }
    }

    fn prev(self) -> Tab {
        match self {
            Tab::Overview => Tab::Log,
            Tab::Cache => Tab::Overview,
            Tab::Packages => Tab::Cache,
            Tab::SysInfo => Tab::Packages,
            Tab::Log => Tab::SysInfo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Idle,
    Running,
    Done,
}

/// Colors the status line in the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Busy,
    Success,
    Warn,
    Error,
}

/// One level of drill-down: the directory being viewed and its entries.
pub struct ScanLevel {
    pub root: PathBuf,
    pub entries: Vec<ScanEntry>,
    pub total_size: u64,
    pub cursor: usize,
    /// False while scanning, or if the user drilled away before the scan
    /// finished (the level then holds partial results).
    pub complete: bool,
}

/// A folder or file the user flagged in the Overview for bulk deletion.
/// Marks persist across drill-down levels until deleted or unmarked.
#[derive(Debug, Clone)]
pub struct Mark {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

/// (target id, computed size). Id "\0done" marks end of sizing.
type SizingMsg = (String, u64);

pub enum CleanupMsg {
    Line(String),
    /// `reclaimed` = bytes actually freed (measured for path-based targets,
    /// estimated for pure-command ones). `new_size` = remeasured size after
    /// deletion, or None when the target has no walkable paths.
    TargetDone { id: String, ok: bool, reclaimed: u64, new_size: Option<u64> },
    AllDone,
}

pub struct App {
    pub profile: SystemProfile,
    pub config: Config,
    pub theme: Theme,
    /// Index into [`theme::ORDER`] for the `t` / `T` live theme switch.
    pub theme_idx: usize,
    pub tab: Tab,
    pub mounts: Vec<MountInfo>,
    pub started: Instant,

    // Scanning / drill-down navigation stack. Last element = current view.
    pub scan_stack: Vec<ScanLevel>,
    pub scan_state: ScanState,
    pub entries_visited: u64,
    scan_handle: Option<ScanHandle>,
    /// Completed scans keyed by directory, so drilling back into a folder
    /// serves instantly instead of re-walking it. Invalidated by `r` (per
    /// dir) and after any cleanup (sizes changed).
    scan_cache: HashMap<PathBuf, (Vec<ScanEntry>, u64)>,

    // Cleanup targets.
    pub targets: Vec<CleanupTarget>,
    pub selected: HashSet<String>,
    pub target_cursor: usize,
    sizing_rx: Option<Receiver<(String, u64)>>,

    // Dry-run / confirm / execution.
    pub confirm_pending: bool,
    pending_root_auth: bool,

    // Packages tab: installed-package inventory, loaded lazily on first view.
    pub packages: Vec<Package>,
    /// Indices into `packages` after filter + sort; the list the tab renders.
    pub pkg_view: Vec<usize>,
    /// Cursor position within `pkg_view`.
    pub pkg_cursor: usize,
    pub pkg_sort: PkgSort,
    pub pkg_filter: String,
    /// True while `/` filter typing captures the keyboard.
    pub pkg_filter_input: bool,
    /// Package ids marked for uninstall.
    pub pkg_marked: HashSet<String>,
    pub pkg_confirm: bool,
    pub pkg_loading: bool,
    pkg_rx: Option<Receiver<(Vec<Package>, Vec<String>)>>,
    pending_pkg_auth: bool,

    // Overview mark-and-delete: arbitrary paths flagged for bulk deletion,
    // separate from the curated Cache-tab targets.
    pub marked: Vec<Mark>,
    pub overview_confirm: bool,
    pending_marked_auth: bool,
    pub log: Vec<String>,
    pub log_scroll: usize,
    cleanup_rx: Option<Receiver<CleanupMsg>>,
    pub cleanup_running: bool,
    reclaimed_this_run: u64,
    /// Bytes reclaimed across every cleanup run this session (survives past
    /// `reclaimed_this_run`, which resets per run) — feeds the farewell screen.
    pub session_reclaimed: u64,
    /// (category, label, bytes) for every target actually cleared this
    /// session, in completion order.
    pub session_freed: Vec<(Category, String, u64)>,

    pub show_help: bool,
    pub status: String,
    pub status_kind: StatusKind,
    should_quit: bool,
}

impl App {
    pub fn new(profile: SystemProfile, config: Config) -> Self {
        let mounts = list_mounts();
        let targets = build_registry(&profile, &config);
        let theme = Theme::from_name(&config.ui.theme);
        let theme_idx = crate::ui::theme::index_of(&config.ui.theme);
        let mut app = App {
            profile,
            config,
            theme,
            theme_idx,
            tab: Tab::Overview,
            mounts,
            started: Instant::now(),
            scan_stack: Vec::new(),
            scan_state: ScanState::Idle,
            entries_visited: 0,
            scan_handle: None,
            scan_cache: HashMap::new(),
            targets,
            selected: HashSet::new(),
            target_cursor: 0,
            sizing_rx: None,
            confirm_pending: false,
            pending_root_auth: false,
            packages: Vec::new(),
            pkg_view: Vec::new(),
            pkg_cursor: 0,
            pkg_sort: PkgSort::Size,
            pkg_filter: String::new(),
            pkg_filter_input: false,
            pkg_marked: HashSet::new(),
            pkg_confirm: false,
            pkg_loading: false,
            pkg_rx: None,
            pending_pkg_auth: false,
            marked: Vec::new(),
            overview_confirm: false,
            pending_marked_auth: false,
            log: vec!["Silt started — nothing gets deleted without your say-so.".into()],
            log_scroll: 0,
            cleanup_rx: None,
            cleanup_running: false,
            reclaimed_this_run: 0,
            session_reclaimed: 0,
            session_freed: Vec::new(),
            show_help: false,
            status: String::new(),
            status_kind: StatusKind::Info,
            should_quit: false,
        };
        app.start_target_sizing();
        let root = app.config.default_root();
        app.begin_scan(root);
        app
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut last_render = Instant::now() - TICK;
        loop {
            self.drain_channels();

            if self.pending_root_auth {
                self.pending_root_auth = false;
                self.sudo_gate_and_execute(terminal)?;
            }

            if self.pending_marked_auth {
                self.pending_marked_auth = false;
                self.sudo_gate_and_delete_marked(terminal)?;
            }

            if self.pending_pkg_auth {
                self.pending_pkg_auth = false;
                self.sudo_gate_and_uninstall_pkgs(terminal)?;
            }

            if last_render.elapsed() >= TICK {
                terminal.draw(|frame| ui::render(frame, &self))?;
                last_render = Instant::now();
            }

            if event::poll(TICK)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key.code, key.modifiers);
                        // Redraw immediately after input for responsiveness.
                        terminal.draw(|frame| ui::render(frame, &self))?;
                        last_render = Instant::now();
                    }
                }
            }

            if self.should_quit {
                if let Some(handle) = &self.scan_handle {
                    handle.cancel();
                }
                return Ok(());
            }
        }
    }

    fn set_status(&mut self, kind: StatusKind, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_kind = kind;
    }

    /// Append to the log; if the view was following the tail, keep following.
    fn push_log(&mut self, line: impl Into<String>) {
        let at_bottom = self.log_scroll + 1 >= self.log.len();
        self.log.push(line.into());
        if at_bottom {
            self.log_scroll = self.log.len() - 1;
        }
    }

    // ---- scanning ----

    fn begin_scan(&mut self, root: PathBuf) {
        if let Some(handle) = &self.scan_handle {
            handle.cancel();
        }
        // Serve a previously completed scan without re-walking the tree.
        if let Some((entries, total)) = self.scan_cache.get(&root) {
            let total = *total;
            self.scan_stack.push(ScanLevel {
                root: root.clone(),
                entries: entries.clone(),
                total_size: total,
                cursor: 0,
                complete: true,
            });
            self.scan_handle = None;
            self.scan_state = ScanState::Done;
            self.set_status(
                StatusKind::Success,
                format!(
                    "Settled: {} holds {}. (cached — r rescans)",
                    root.display(),
                    crate::ui::human(total)
                ),
            );
            return;
        }
        // Remote (cloud/network) mounts are shown as entries but never
        // walked: sizing a FUSE cloud mount forces every file to download.
        // The mount itself reports its remote usage, so surface that instead.
        // Exception: scanning the mount point itself (explicit user choice).
        let mut exclude = self.config.scan.exclude_paths.clone();
        let mut seed: Vec<ScanEntry> = Vec::new();
        if !self.config.scan.include_remote_mounts {
            for m in self.mounts.iter().filter(|m| m.is_remote()) {
                if m.mount_point != root {
                    exclude.push(m.mount_point.clone());
                }
                if m.mount_point.parent() == Some(root.as_path()) {
                    seed.push(ScanEntry {
                        path: m.mount_point.clone(),
                        size: m.used_bytes,
                        is_dir: true,
                        remote: true,
                    });
                }
            }
        }
        self.scan_stack.push(ScanLevel {
            root: root.clone(),
            entries: seed,
            total_size: 0,
            cursor: 0,
            complete: false,
        });
        self.entries_visited = 0;
        self.scan_state = ScanState::Running;
        self.set_status(StatusKind::Busy, format!("Sifting through {}…", root.display()));
        self.scan_handle = Some(start_scan(
            root,
            exclude,
            self.config.scan.follow_symlinks,
        ));
    }

    fn drain_channels(&mut self) {
        // Scan events.
        if let Some(handle) = &self.scan_handle {
            let mut done = false;
            let mut warnings: Vec<String> = Vec::new();
            while let Ok(event) = handle.receiver.try_recv() {
                match event {
                    ScanEvent::DirScanned { path, size, is_dir } => {
                        if let Some(level) = self.scan_stack.last_mut() {
                            level.entries.push(ScanEntry { path, size, is_dir, remote: false });
                            level.entries.sort_by_key(|e| std::cmp::Reverse(e.size));
                            level.total_size += size;
                        }
                    }
                    ScanEvent::Progress { entries_visited } => {
                        self.entries_visited = entries_visited;
                    }
                    ScanEvent::Done { total_size } => {
                        let cache_entry = if let Some(level) = self.scan_stack.last_mut() {
                            level.total_size = total_size;
                            level.complete = true;
                            Some((level.root.clone(), level.entries.clone()))
                        } else {
                            None
                        };
                        if let Some((root, entries)) = cache_entry {
                            self.scan_cache.insert(root, (entries, total_size));
                        }
                        done = true;
                    }
                    ScanEvent::Warning(w) => warnings.push(w),
                }
            }
            // Keep only a few warnings in the log; permission noise is common.
            for w in warnings.into_iter().take(3) {
                self.push_log(format!("scan: {w}"));
            }
            if done {
                self.scan_state = ScanState::Done;
                self.scan_handle = None;
                if let Some(level) = self.scan_stack.last() {
                    self.set_status(
                        StatusKind::Success,
                        format!(
                            "Settled: {} holds {}.",
                            level.root.display(),
                            crate::ui::human(level.total_size)
                        ),
                    );
                }
            }
        }

        // Target sizing results.
        if let Some(rx) = &self.sizing_rx {
            let mut finished = false;
            while let Ok(msg) = rx.try_recv() {
                let (id, size) = msg;
                if id == "\0done" {
                    finished = true;
                    continue;
                }
                if let Some(t) = self.targets.iter_mut().find(|t| t.id == id) {
                    t.size_bytes = Some(size);
                }
            }
            if finished {
                self.sizing_rx = None;
            }
        }

        // Package inventory results.
        if let Some(rx) = &self.pkg_rx {
            let mut loaded: Option<(Vec<Package>, Vec<String>)> = None;
            if let Ok(msg) = rx.try_recv() {
                loaded = Some(msg);
            }
            if let Some((pkgs, warnings)) = loaded {
                for w in warnings {
                    self.push_log(format!("packages: {w}"));
                }
                self.packages = pkgs;
                self.pkg_loading = false;
                self.pkg_rx = None;
                self.recompute_pkg_view();
                let total: u64 = self.packages.iter().map(|p| p.size).sum();
                self.set_status(
                    StatusKind::Success,
                    format!(
                        "Found {} installed packages ({} on disk).",
                        self.packages.len(),
                        crate::ui::human(total)
                    ),
                );
            }
        }

        // Cleanup execution log.
        if let Some(rx) = &self.cleanup_rx {
            let mut all_done = false;
            let mut done_ids: Vec<(String, bool, u64, Option<u64>)> = Vec::new();
            let mut lines: Vec<String> = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    CleanupMsg::Line(l) => lines.push(l),
                    CleanupMsg::TargetDone { id, ok, reclaimed, new_size } => {
                        done_ids.push((id, ok, reclaimed, new_size))
                    }
                    CleanupMsg::AllDone => all_done = true,
                }
            }
            for l in lines {
                self.push_log(l);
            }
            for (id, ok, reclaimed, new_size) in done_ids {
                if ok {
                    self.selected.remove(&id);
                    self.reclaimed_this_run += reclaimed;
                    self.session_reclaimed += reclaimed;
                    if let Some(pkg_id) = id.strip_prefix("\0pkg:") {
                        // Package uninstall: drop it from the inventory and
                        // credit the freed bytes to its own category.
                        if let Some(pos) = self.packages.iter().position(|p| p.id == pkg_id) {
                            let p = self.packages.remove(pos);
                            if reclaimed > 0 {
                                self.session_freed.push((
                                    Category::Packages,
                                    p.name.clone(),
                                    reclaimed,
                                ));
                            }
                            self.recompute_pkg_view();
                        }
                    } else if let Some(display) = id.strip_prefix("\0mark:") {
                        // Overview mark-and-delete: a hand-picked path, not a
                        // registry target, so there's no CleanupTarget to
                        // look up — file it under its own category instead.
                        if reclaimed > 0 {
                            self.session_freed.push((
                                Category::Marked,
                                display.to_string(),
                                reclaimed,
                            ));
                        }
                    } else if let Some(t) = self.targets.iter_mut().find(|t| t.id == id) {
                        // Size was remeasured after deletion (None = no
                        // walkable paths, so treat as fully cleared).
                        t.size_bytes = Some(new_size.unwrap_or(0));
                        if reclaimed > 0 {
                            self.session_freed.push((t.category, t.label.clone(), reclaimed));
                        }
                    }
                }
            }
            if all_done {
                self.cleanup_running = false;
                self.cleanup_rx = None;
                // Deletions changed on-disk sizes; drop cached scans so the
                // Overview re-walks fresh, and prune any now-gone entries from
                // the level on screen so marked deletions vanish immediately.
                self.scan_cache.clear();
                if let Some(level) = self.scan_stack.last_mut() {
                    level.entries.retain(|e| e.path.exists());
                    level.cursor = level.cursor.min(level.entries.len().saturating_sub(1));
                }
                let reclaimed = self.reclaimed_this_run;
                if reclaimed > 0 {
                    let msg = format!("✦ Reclaimed {}. Nice and tidy.", crate::ui::human(reclaimed));
                    self.push_log(msg.clone());
                    self.set_status(StatusKind::Success, msg);
                } else {
                    self.set_status(StatusKind::Warn, "Cleanup finished — see Log tab for details.");
                }
                self.push_log("--- cleanup finished ---");
            }
        }
    }

    fn start_target_sizing(&mut self) {
        let (tx, rx): (Sender<SizingMsg>, Receiver<SizingMsg>) = std::sync::mpsc::channel();
        let work: Vec<(String, Vec<PathBuf>)> = self
            .targets
            .iter()
            .filter(|t| t.size_bytes.is_none() && !t.paths.is_empty())
            .map(|t| (t.id.clone(), t.paths.clone()))
            .collect();
        std::thread::Builder::new()
            .name("silt-sizer".into())
            .spawn(move || {
                for (id, paths) in work {
                    let size: u64 = paths
                        .iter()
                        .map(|p| crate::scanner::walker::path_size(p))
                        .sum();
                    if tx.send((id, size)).is_err() {
                        return;
                    }
                }
                let _ = tx.send(("\0done".into(), 0));
            })
            .expect("failed to spawn sizer thread");
        self.sizing_rx = Some(rx);
    }

    // ---- input ----

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // The confirm overlay owns the keyboard: no tab switching, no quit,
        // nothing but an explicit yes/no.
        if self.confirm_pending {
            self.handle_confirm_key(code);
            return;
        }

        // The Overview delete-marked overlay likewise owns the keyboard.
        if self.overview_confirm {
            self.handle_overview_confirm_key(code);
            return;
        }

        // The Packages uninstall overlay likewise owns the keyboard.
        if self.pkg_confirm {
            self.handle_pkg_confirm_key(code);
            return;
        }

        if self.show_help {
            self.show_help = false;
            return;
        }

        // Filter typing captures everything (including q / Tab / digits).
        if self.tab == Tab::Packages && self.pkg_filter_input {
            self.handle_pkg_filter_key(code);
            return;
        }

        match code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                return;
            }
            KeyCode::Tab => {
                self.switch_tab(self.tab.next());
                return;
            }
            KeyCode::BackTab => {
                self.switch_tab(self.tab.prev());
                return;
            }
            KeyCode::Char('1') => { self.switch_tab(Tab::Overview); return; }
            KeyCode::Char('2') => { self.switch_tab(Tab::Cache); return; }
            KeyCode::Char('3') => { self.switch_tab(Tab::Packages); return; }
            KeyCode::Char('4') => { self.switch_tab(Tab::SysInfo); return; }
            KeyCode::Char('5') => { self.switch_tab(Tab::Log); return; }
            KeyCode::Char('t') => { self.cycle_theme(1); return; }
            KeyCode::Char('T') => { self.cycle_theme(-1); return; }
            _ => {}
        }

        match self.tab {
            Tab::Overview => self.handle_overview_key(code),
            Tab::Cache => self.handle_cache_key(code),
            Tab::Packages => self.handle_pkg_key(code),
            Tab::Log => self.handle_log_key(code),
            Tab::SysInfo => {}
        }
    }

    /// Step the live theme by `dir` (+1 next, -1 previous), wrapping around the
    /// gallery, and announce it in the status line so the switch is legible even
    /// on a color no one recognizes.
    fn cycle_theme(&mut self, dir: i32) {
        let n = theme::ORDER.len() as i32;
        self.theme_idx = (((self.theme_idx as i32 + dir) % n + n) % n) as usize;
        let name = theme::ORDER[self.theme_idx];
        self.theme = Theme::from_name(name);
        self.config.ui.theme = name.to_string();
        match Config::save_theme(name) {
            Ok(()) => self.set_status(
                StatusKind::Info,
                format!("Theme: {}  (saved · t / T to keep browsing)", theme::label(name)),
            ),
            Err(e) => self.set_status(
                StatusKind::Warn,
                format!("Theme: {} — couldn't save: {e}", theme::label(name)),
            ),
        }
    }

    /// Switch tab; entering Packages for the first time kicks off the
    /// (potentially slow) inventory in the background.
    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Packages && self.packages.is_empty() && !self.pkg_loading {
            self.start_pkg_load();
        }
    }

    fn handle_overview_key(&mut self, code: KeyCode) {
        // Mark / delete / reveal act on the current entry and touch `self`
        // beyond the level, so handle them before the `last_mut` borrow.
        match code {
            KeyCode::Char(' ') => {
                self.toggle_mark_current();
                return;
            }
            KeyCode::Char('d') => {
                if self.marked.is_empty() {
                    self.set_status(StatusKind::Warn, "No folders marked. Space marks one.");
                } else {
                    self.overview_confirm = true;
                    self.set_status(
                        StatusKind::Warn,
                        "Confirm delete: y wipes marked folders, n/Esc cancels.",
                    );
                }
                return;
            }
            KeyCode::Char('o') => {
                self.reveal_current();
                return;
            }
            _ => {}
        }

        let Some(level) = self.scan_stack.last_mut() else {
            return;
        };
        match code {
            KeyCode::Down | KeyCode::Char('j')
                if level.cursor + 1 < level.entries.len() => {
                    level.cursor += 1;
                }
            KeyCode::Up | KeyCode::Char('k') => {
                level.cursor = level.cursor.saturating_sub(1);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                level.cursor = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                level.cursor = level.entries.len().saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                // Drilling is allowed mid-scan, ncdu-style: the running scan
                // is cancelled (the level keeps its partial results) and the
                // chosen directory is scanned instead.
                let entry = level.entries.get(level.cursor).cloned();
                match entry {
                    Some(entry) if entry.remote => self.set_status(
                        StatusKind::Warn,
                        format!(
                            "☁ {} is cloud storage — scanning it would download every file. o opens it instead.",
                            entry.path.display()
                        ),
                    ),
                    Some(entry) if entry.is_dir => self.begin_scan(entry.path),
                    Some(_) => {
                        self.set_status(StatusKind::Info, "That's a file — only directories open.")
                    }
                    None => {}
                }
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h')
                if self.scan_stack.len() > 1 => {
                    if let Some(handle) = &self.scan_handle {
                        handle.cancel();
                        self.scan_handle = None;
                    }
                    self.scan_stack.pop();
                    self.scan_state = ScanState::Done;
                    if let Some(level) = self.scan_stack.last() {
                        let note = if level.complete { "" } else { " (partial — r rescans)" };
                        self.set_status(
                            StatusKind::Info,
                            format!("Back in {}{note}", level.root.display()),
                        );
                    }
                }
            KeyCode::Char('r') => {
                let root = self
                    .scan_stack
                    .pop()
                    .map(|l| l.root)
                    .unwrap_or_else(|| self.config.default_root());
                self.scan_cache.remove(&root);
                self.begin_scan(root);
            }
            _ => {}
        }
    }

    /// Toggle the mark on the entry under the Overview cursor.
    fn toggle_mark_current(&mut self) {
        let Some(entry) = self
            .scan_stack
            .last()
            .and_then(|l| l.entries.get(l.cursor).cloned())
        else {
            return;
        };
        if entry.remote {
            self.set_status(
                StatusKind::Warn,
                "☁ That's cloud storage — Silt won't bulk-delete a remote mount.",
            );
            return;
        }
        if let Some(pos) = self.marked.iter().position(|m| m.path == entry.path) {
            self.marked.remove(pos);
            self.set_status(StatusKind::Info, format!("Unmarked {}", entry.path.display()));
        } else {
            self.set_status(
                StatusKind::Info,
                format!("Marked {} ({})", entry.path.display(), crate::ui::human(entry.size)),
            );
            self.marked.push(Mark {
                path: entry.path,
                size: entry.size,
                is_dir: entry.is_dir,
            });
        }
    }

    /// Open the current entry in the system file manager (`xdg-open`). Files
    /// reveal their parent directory.
    fn reveal_current(&mut self) {
        let Some(entry) = self
            .scan_stack
            .last()
            .and_then(|l| l.entries.get(l.cursor).cloned())
        else {
            return;
        };
        let target = if entry.is_dir {
            entry.path.clone()
        } else {
            entry
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| entry.path.clone())
        };
        match std::process::Command::new("xdg-open")
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.set_status(
                StatusKind::Info,
                format!("Opening {} in file manager…", target.display()),
            ),
            Err(e) => self.set_status(
                StatusKind::Error,
                format!("Couldn't open file manager (xdg-open): {e}"),
            ),
        }
    }

    fn handle_overview_confirm_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.overview_confirm = false;
                if self.marked_needs_root() {
                    // Same rule as target cleanup: sudo prompts outside raw mode.
                    self.pending_marked_auth = true;
                } else {
                    self.delete_marked(false);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.overview_confirm = false;
                self.set_status(StatusKind::Info, "Cancelled — marked folders untouched.");
            }
            _ => {}
        }
    }

    /// Total size of everything currently marked.
    pub fn marked_total(&self) -> u64 {
        self.marked.iter().map(|m| m.size).sum()
    }

    /// True when a path with this exact location is marked.
    /// True while any background job is running: a scan, target sizing, package
    /// inventory, or a cleanup/uninstall. Drives the header ramp animation.
    pub fn is_working(&self) -> bool {
        self.scan_state == ScanState::Running
            || self.cleanup_running
            || self.pkg_loading
            || self.sizing_rx.is_some()
    }

    pub fn is_marked(&self, path: &std::path::Path) -> bool {
        self.marked.iter().any(|m| m.path == path)
    }

    /// True when any marked path is owned by another user (deletion needs sudo).
    pub fn marked_needs_root(&self) -> bool {
        !crate::targets::is_root() && self.marked.iter().any(|m| path_needs_root(&m.path))
    }

    fn handle_cache_key(&mut self, code: KeyCode) {
        if self.cleanup_running {
            self.set_status(StatusKind::Busy, "Cleanup in progress — see Log tab.");
            return;
        }
        match code {
            KeyCode::Down | KeyCode::Char('j')
                if self.target_cursor + 1 < self.targets.len() => {
                    self.target_cursor += 1;
                }
            KeyCode::Up | KeyCode::Char('k') => {
                self.target_cursor = self.target_cursor.saturating_sub(1);
            }
            KeyCode::Char(' ') => {
                if let Some(t) = self.targets.get(self.target_cursor) {
                    let id = t.id.clone();
                    if self.selected.contains(&id) {
                        self.selected.remove(&id);
                    } else {
                        self.selected.insert(id);
                    }
                }
            }
            KeyCode::Char('a') => {
                // Select all Safe targets (never bulk-select Caution).
                let mut added = 0;
                for t in &self.targets {
                    if t.risk == RiskTier::Safe && self.selected.insert(t.id.clone()) {
                        added += 1;
                    }
                }
                self.set_status(
                    StatusKind::Info,
                    format!("Grabbed every Safe target ({added} new)."),
                );
            }
            KeyCode::Char('A') => {
                self.selected.clear();
                self.set_status(StatusKind::Info, "Selection cleared.");
            }
            KeyCode::Enter => {
                if self.selected.is_empty() {
                    self.set_status(
                        StatusKind::Warn,
                        "Nothing picked. Space marks a target; 'a' grabs all Safe.",
                    );
                    return;
                }
                self.show_dry_run();
                self.confirm_pending = true;
            }
            _ => {}
        }
    }

    fn handle_log_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Char('j')
                if self.log_scroll + 1 < self.log.len() => {
                    self.log_scroll += 1;
                }
            KeyCode::Up | KeyCode::Char('k') => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.log_scroll = self.log.len().saturating_sub(1);
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.log_scroll = 0;
            }
            _ => {}
        }
    }

    // ---- packages tab ----

    /// Take package inventory on a background thread (listing a few thousand
    /// packages can take a second or two).
    fn start_pkg_load(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let profile = self.profile.clone();
        self.pkg_loading = true;
        self.pkg_rx = Some(rx);
        self.set_status(StatusKind::Busy, "Taking inventory of installed packages…");
        std::thread::Builder::new()
            .name("silt-packages".into())
            .spawn(move || {
                let _ = tx.send(list_installed(&profile));
            })
            .expect("failed to spawn package inventory thread");
    }

    /// Rebuild the filtered + sorted view over `packages`.
    pub fn recompute_pkg_view(&mut self) {
        let filter = self.pkg_filter.to_lowercase();
        let mut view: Vec<usize> = self
            .packages
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || p.uninstall_ref.to_lowercase().contains(&filter)
            })
            .map(|(i, _)| i)
            .collect();
        match self.pkg_sort {
            PkgSort::Size => {
                view.sort_by_key(|&i| std::cmp::Reverse(self.packages[i].size));
            }
            PkgSort::Name => {
                view.sort_by(|&a, &b| self.packages[a].name.cmp(&self.packages[b].name));
            }
            PkgSort::Source => {
                // Group by source, biggest first within each group.
                view.sort_by(|&a, &b| {
                    let (pa, pb) = (&self.packages[a], &self.packages[b]);
                    pa.source
                        .to_string()
                        .cmp(&pb.source.to_string())
                        .then(pb.size.cmp(&pa.size))
                });
            }
        }
        self.pkg_view = view;
        self.pkg_cursor = self.pkg_cursor.min(self.pkg_view.len().saturating_sub(1));
    }

    /// Package under the Packages-tab cursor, honoring filter + sort.
    pub fn pkg_at_cursor(&self) -> Option<&Package> {
        self.pkg_view
            .get(self.pkg_cursor)
            .map(|&i| &self.packages[i])
    }

    /// All packages currently marked for uninstall.
    pub fn marked_pkgs(&self) -> Vec<&Package> {
        self.packages
            .iter()
            .filter(|p| self.pkg_marked.contains(&p.id))
            .collect()
    }

    /// True when any marked package's uninstall would invoke sudo.
    pub fn pkg_marked_needs_root(&self) -> bool {
        !crate::targets::is_root() && self.marked_pkgs().iter().any(|p| p.needs_root())
    }

    fn handle_pkg_key(&mut self, code: KeyCode) {
        if self.cleanup_running {
            self.set_status(StatusKind::Busy, "Uninstall in progress — see Log tab.");
            return;
        }
        match code {
            KeyCode::Down | KeyCode::Char('j')
                if self.pkg_cursor + 1 < self.pkg_view.len() => {
                    self.pkg_cursor += 1;
                }
            KeyCode::Up | KeyCode::Char('k') => {
                self.pkg_cursor = self.pkg_cursor.saturating_sub(1);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.pkg_cursor = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.pkg_cursor = self.pkg_view.len().saturating_sub(1);
            }
            KeyCode::PageDown => {
                self.pkg_cursor =
                    (self.pkg_cursor + 20).min(self.pkg_view.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                self.pkg_cursor = self.pkg_cursor.saturating_sub(20);
            }
            KeyCode::Char(' ') => self.toggle_mark_pkg(),
            KeyCode::Char('s') => {
                self.pkg_sort = self.pkg_sort.next();
                self.recompute_pkg_view();
                self.set_status(
                    StatusKind::Info,
                    format!("Sorted by {}.", self.pkg_sort.label()),
                );
            }
            KeyCode::Char('/') => {
                self.pkg_filter_input = true;
                self.set_status(
                    StatusKind::Info,
                    "Filter: type to narrow, Enter keeps it, Esc clears it.",
                );
            }
            KeyCode::Esc if !self.pkg_filter.is_empty() => {
                self.pkg_filter.clear();
                self.recompute_pkg_view();
                self.set_status(StatusKind::Info, "Filter cleared.");
            }
            KeyCode::Char('r') => {
                if !self.pkg_loading {
                    self.pkg_marked.clear();
                    self.packages.clear();
                    self.pkg_view.clear();
                    self.start_pkg_load();
                }
            }
            KeyCode::Char('d') | KeyCode::Enter => {
                if self.pkg_marked.is_empty() {
                    self.set_status(StatusKind::Warn, "No packages marked. Space marks one.");
                } else {
                    self.pkg_confirm = true;
                    self.set_status(
                        StatusKind::Warn,
                        "Confirm uninstall: y removes marked packages, n/Esc cancels.",
                    );
                }
            }
            _ => {}
        }
    }

    fn handle_pkg_filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                self.pkg_filter_input = false;
                self.set_status(
                    StatusKind::Info,
                    format!("{} packages match.", self.pkg_view.len()),
                );
            }
            KeyCode::Esc => {
                self.pkg_filter_input = false;
                self.pkg_filter.clear();
                self.recompute_pkg_view();
                self.set_status(StatusKind::Info, "Filter cleared.");
            }
            KeyCode::Backspace => {
                self.pkg_filter.pop();
                self.recompute_pkg_view();
            }
            KeyCode::Char(c) => {
                self.pkg_filter.push(c);
                self.pkg_cursor = 0;
                self.recompute_pkg_view();
            }
            _ => {}
        }
    }

    /// Toggle the uninstall mark on the package under the cursor.
    fn toggle_mark_pkg(&mut self) {
        let Some(p) = self.pkg_at_cursor() else {
            return;
        };
        if p.essential {
            self.set_status(
                StatusKind::Warn,
                format!("▲ {} is a core system package — Silt won't remove it.", p.name),
            );
            return;
        }
        if p.uninstall_command().is_none() {
            self.set_status(
                StatusKind::Warn,
                format!("Silt can't uninstall {} (unsupported source).", p.name),
            );
            return;
        }
        let (id, name, size) = (p.id.clone(), p.name.clone(), p.size);
        if self.pkg_marked.remove(&id) {
            self.set_status(StatusKind::Info, format!("Unmarked {name}"));
        } else {
            self.pkg_marked.insert(id);
            self.set_status(
                StatusKind::Info,
                format!("Marked {name} ({}) for uninstall", crate::ui::human(size)),
            );
        }
    }

    fn handle_pkg_confirm_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.pkg_confirm = false;
                if self.pkg_marked_needs_root() {
                    // Same rule as everywhere: sudo prompts outside raw mode.
                    self.pending_pkg_auth = true;
                } else {
                    self.uninstall_marked_pkgs(false);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pkg_confirm = false;
                self.set_status(StatusKind::Info, "Cancelled — nothing uninstalled.");
            }
            _ => {}
        }
    }

    /// Suspend the TUI for a `sudo -v` prompt, then run the uninstalls.
    /// Mirrors `sudo_gate_and_execute`.
    fn sudo_gate_and_uninstall_pkgs(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let cached = std::process::Command::new("sudo")
            .args(["-n", "-v"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if cached {
            self.uninstall_marked_pkgs(false);
            return Ok(());
        }

        ratatui::restore();
        println!("Silt needs sudo to uninstall the marked packages.");
        println!("(your password goes directly to sudo; Silt never sees it)\n");
        let authed = std::process::Command::new("sudo")
            .arg("-v")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !authed {
            *terminal = ratatui::init();
            terminal.clear()?;
            self.push_log("ERROR: sudo authentication failed; nothing was uninstalled.");
            self.set_status(StatusKind::Error, "sudo authentication failed — nothing removed.");
            return Ok(());
        }

        // See `sudo_gate_and_execute`: if credentials didn't cache, run inline
        // with a live prompt rather than the non-blocking `sudo -n` path.
        let caches = std::process::Command::new("sudo")
            .args(["-n", "-v"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if caches {
            *terminal = ratatui::init();
            terminal.clear()?;
            self.uninstall_marked_pkgs(false);
        } else {
            println!("(sudo won't cache credentials here — you may be prompted per step)\n");
            let handle = self.uninstall_marked_pkgs(true);
            let _ = handle.join();
            *terminal = ratatui::init();
            terminal.clear()?;
        }
        Ok(())
    }

    /// Uninstall every marked package on a worker thread, reusing the cleanup
    /// log channel. Marks are drained up front. After a successful uninstall
    /// of a system package, exact-name leftovers under ~/.cache, ~/.config and
    /// ~/.local/share are purged too.
    fn uninstall_marked_pkgs(&mut self, interactive: bool) -> std::thread::JoinHandle<()> {
        let victims: Vec<Package> = self
            .marked_pkgs()
            .into_iter()
            .cloned()
            .collect();
        self.pkg_marked.clear();
        let am_root = crate::targets::is_root();
        let (tx, rx) = std::sync::mpsc::channel();
        self.cleanup_rx = Some(rx);
        self.cleanup_running = true;
        self.reclaimed_this_run = 0;
        self.push_log("--- uninstalling packages ---");
        self.set_status(StatusKind::Busy, "Uninstalling packages…");
        self.tab = Tab::Log;
        std::thread::Builder::new()
            .name("silt-uninstall".into())
            .spawn(move || {
                for pkg in victims {
                    let _ = tx.send(CleanupMsg::Line(format!(
                        ">> {} ({}, {})",
                        pkg.name,
                        pkg.source,
                        crate::ui::human(pkg.size)
                    )));
                    let Some((cmd, args, needs_root)) = pkg.uninstall_command() else {
                        let _ = tx.send(CleanupMsg::Line(
                            "   ERROR: unsupported source".into(),
                        ));
                        continue;
                    };
                    let (program, full_args) = if needs_root && !am_root {
                        let mut a = if interactive {
                            vec![cmd]
                        } else {
                            vec!["-n".to_string(), cmd]
                        };
                        a.extend(args);
                        ("sudo".to_string(), a)
                    } else {
                        (cmd, args)
                    };
                    let _ = tx.send(CleanupMsg::Line(format!(
                        "   $ {program} {}",
                        full_args.join(" ")
                    )));
                    let output = std::process::Command::new(&program)
                        .args(&full_args)
                        .output();
                    let ok = match output {
                        Ok(out) => {
                            for line in String::from_utf8_lossy(&out.stdout).lines().take(20) {
                                let _ = tx.send(CleanupMsg::Line(format!("   {line}")));
                            }
                            for line in String::from_utf8_lossy(&out.stderr).lines().take(10) {
                                let _ = tx.send(CleanupMsg::Line(format!("   ! {line}")));
                            }
                            if !out.status.success() {
                                let _ = tx.send(CleanupMsg::Line(format!(
                                    "   ERROR: {program} exited with {}",
                                    out.status
                                )));
                            }
                            out.status.success()
                        }
                        Err(e) => {
                            let _ = tx.send(CleanupMsg::Line(format!(
                                "   ERROR: running {program}: {e}"
                            )));
                            false
                        }
                    };
                    let mut reclaimed = if ok { pkg.size } else { 0 };
                    if ok {
                        // Purge leftover per-user data (system packages only;
                        // flatpak/snap purge their own via the flags above).
                        for dir in pkg.leftover_dirs() {
                            let bytes = crate::scanner::walker::path_size(&dir);
                            match std::fs::remove_dir_all(&dir) {
                                Ok(()) => {
                                    reclaimed += bytes;
                                    let _ = tx.send(CleanupMsg::Line(format!(
                                        "   purged leftover data {} ({})",
                                        dir.display(),
                                        crate::ui::human(bytes)
                                    )));
                                }
                                Err(e) => {
                                    let _ = tx.send(CleanupMsg::Line(format!(
                                        "   ! couldn't purge {}: {e}",
                                        dir.display()
                                    )));
                                }
                            }
                        }
                    }
                    let _ = tx.send(CleanupMsg::TargetDone {
                        id: format!("\0pkg:{}", pkg.id),
                        ok,
                        reclaimed,
                        new_size: None,
                    });
                }
                let _ = tx.send(CleanupMsg::AllDone);
            })
            .expect("failed to spawn uninstall thread")
    }

    fn handle_confirm_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.confirm_pending = false;
                let needs_sudo = self.selected_targets().iter().any(|t| t.needs_sudo());
                if needs_sudo {
                    // Sudo must prompt on a real terminal, not inside raw
                    // mode; the run loop suspends the TUI first.
                    self.pending_root_auth = true;
                } else {
                    self.execute_selected(false);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.confirm_pending = false;
                self.push_log("Cancelled. Nothing was deleted.");
                self.set_status(StatusKind::Info, "Cancelled — everything stays put.");
            }
            _ => {}
        }
    }

    // ---- dry run + execution ----

    pub fn selected_targets(&self) -> Vec<&CleanupTarget> {
        self.targets
            .iter()
            .filter(|t| self.selected.contains(&t.id))
            .collect()
    }

    fn show_dry_run(&mut self) {
        let mut lines = vec![String::new(), "=== DRY RUN PREVIEW ===".to_string()];
        let mut total: u64 = 0;
        let mut has_caution = false;
        for t in self.selected_targets() {
            lines.push(format!(
                "[{}] {} — {}",
                t.risk,
                t.label,
                t.size_bytes.map(crate::ui::human).unwrap_or_else(|| "size unknown".into())
            ));
            lines.extend(t.dry_run_preview());
            total += t.size_bytes.unwrap_or(0);
            if t.risk == RiskTier::Caution {
                has_caution = true;
            }
        }
        lines.push(format!("Estimated reclaim: {}", crate::ui::human(total)));
        if has_caution {
            lines.push(
                "WARNING: selection includes Caution-tier targets that may contain real data."
                    .into(),
            );
        }
        for l in lines {
            self.push_log(l);
        }
        self.set_status(StatusKind::Warn, "Confirm cleanup: y executes, n/Esc cancels.");
    }

    /// Leave the TUI, let sudo prompt for a password on the real terminal,
    /// then come back and run the cleanup. Keystrokes go straight to sudo;
    /// the event loop never sees them.
    fn sudo_gate_and_execute(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // Cached credentials (or NOPASSWD)? Then no prompt is needed and the
        // screen doesn't have to blink at all.
        let cached = std::process::Command::new("sudo")
            .args(["-n", "-v"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if cached {
            self.execute_selected(false);
            return Ok(());
        }

        let root_labels: Vec<String> = self
            .selected_targets()
            .iter()
            .filter(|t| t.needs_sudo())
            .map(|t| t.label.clone())
            .collect();

        ratatui::restore();
        println!("Silt needs sudo for: {}", root_labels.join(", "));
        println!("(your password goes directly to sudo; Silt never sees it)\n");
        let authed = std::process::Command::new("sudo")
            .arg("-v")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !authed {
            *terminal = ratatui::init();
            terminal.clear()?;
            self.push_log("ERROR: sudo authentication failed; nothing was run.");
            self.set_status(StatusKind::Error, "sudo authentication failed — nothing was run.");
            return Ok(());
        }

        // Did the credentials actually cache? On systems with
        // `timestamp_timeout=0` sudo authenticates but stores no timestamp, so
        // the threaded `sudo -n` path can never work. Detect that and run the
        // cleanup inline instead, letting each sudo command prompt on the
        // terminal that's still suspended below us.
        let caches = std::process::Command::new("sudo")
            .args(["-n", "-v"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if caches {
            *terminal = ratatui::init();
            terminal.clear()?;
            self.execute_selected(false);
        } else {
            println!("(sudo won't cache credentials here — you may be prompted per step)\n");
            let handle = self.execute_selected(true);
            let _ = handle.join();
            *terminal = ratatui::init();
            terminal.clear()?;
        }
        Ok(())
    }

    /// Spawn the cleanup worker. `interactive` picks the sudo mode: `false`
    /// uses `sudo -n` (needs cached credentials, non-blocking — the normal
    /// path); `true` lets sudo prompt on the terminal, for systems that refuse
    /// to cache. The join handle lets the interactive caller block while the
    /// TUI is suspended so the prompt is visible.
    fn execute_selected(&mut self, interactive: bool) -> std::thread::JoinHandle<()> {
        let targets: Vec<CleanupTarget> =
            self.selected_targets().into_iter().cloned().collect();
        let (tx, rx) = std::sync::mpsc::channel();
        self.cleanup_rx = Some(rx);
        self.cleanup_running = true;
        self.reclaimed_this_run = 0;
        self.push_log("--- executing cleanup ---");
        self.set_status(StatusKind::Busy, "Executing cleanup…");
        self.tab = Tab::Log;
        std::thread::Builder::new()
            .name("silt-cleanup".into())
            .spawn(move || {
                for target in targets {
                    let _ = tx.send(CleanupMsg::Line(format!(">> {}", target.label)));
                    // Measure real occupancy before deleting so the reported
                    // reclaim reflects what actually got freed, not the
                    // pre-run estimate. Pure-command targets (no walkable
                    // paths) fall back to the estimate.
                    let has_paths = !target.paths.is_empty();
                    let sized = |t: &CleanupTarget| -> u64 {
                        t.paths
                            .iter()
                            .map(|p| crate::scanner::walker::path_size(p))
                            .sum()
                    };
                    let before = if has_paths {
                        sized(&target)
                    } else {
                        target.size_bytes.unwrap_or(0)
                    };
                    match target.execute(interactive) {
                        Ok(lines) => {
                            for l in lines {
                                let _ = tx.send(CleanupMsg::Line(format!("   {l}")));
                            }
                            let (reclaimed, new_size) = if has_paths {
                                let after = sized(&target);
                                (before.saturating_sub(after), Some(after))
                            } else {
                                (before, None)
                            };
                            let _ = tx.send(CleanupMsg::TargetDone {
                                id: target.id.clone(),
                                ok: true,
                                reclaimed,
                                new_size,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(CleanupMsg::Line(format!("   ERROR: {e:#}")));
                            let _ = tx.send(CleanupMsg::TargetDone {
                                id: target.id.clone(),
                                ok: false,
                                reclaimed: 0,
                                new_size: None,
                            });
                        }
                    }
                }
                let _ = tx.send(CleanupMsg::AllDone);
            })
            .expect("failed to spawn cleanup thread")
    }

    /// Suspend the TUI for a `sudo -v` prompt, then delete the marked folders.
    /// Mirrors `sudo_gate_and_execute` but for arbitrary marked paths.
    fn sudo_gate_and_delete_marked(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let cached = std::process::Command::new("sudo")
            .args(["-n", "-v"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if cached {
            self.delete_marked(false);
            return Ok(());
        }

        ratatui::restore();
        println!("Silt needs sudo to delete root-owned folders you marked.");
        println!("(your password goes directly to sudo; Silt never sees it)\n");
        let authed = std::process::Command::new("sudo")
            .arg("-v")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !authed {
            *terminal = ratatui::init();
            terminal.clear()?;
            self.push_log("ERROR: sudo authentication failed; nothing was deleted.");
            self.set_status(StatusKind::Error, "sudo authentication failed — nothing deleted.");
            return Ok(());
        }

        // See `sudo_gate_and_execute`: if credentials didn't cache, run inline
        // with a live prompt rather than the non-blocking `sudo -n` path.
        let caches = std::process::Command::new("sudo")
            .args(["-n", "-v"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if caches {
            *terminal = ratatui::init();
            terminal.clear()?;
            self.delete_marked(false);
        } else {
            println!("(sudo won't cache credentials here — you may be prompted per step)\n");
            let handle = self.delete_marked(true);
            let _ = handle.join();
            *terminal = ratatui::init();
            terminal.clear()?;
        }
        Ok(())
    }

    /// Delete every marked path on a worker thread, reusing the cleanup log
    /// channel so results land in the Log tab. Marks are drained up front, so
    /// finishing (or failing) leaves nothing half-flagged.
    fn delete_marked(&mut self, interactive: bool) -> std::thread::JoinHandle<()> {
        let marks = std::mem::take(&mut self.marked);
        let am_root = crate::targets::is_root();
        let (tx, rx) = std::sync::mpsc::channel();
        self.cleanup_rx = Some(rx);
        self.cleanup_running = true;
        self.reclaimed_this_run = 0;
        self.push_log("--- deleting marked folders ---");
        self.set_status(StatusKind::Busy, "Deleting marked folders…");
        self.tab = Tab::Log;
        std::thread::Builder::new()
            .name("silt-marked".into())
            .spawn(move || {
                for m in marks {
                    let display = m.path.display().to_string();
                    let _ = tx.send(CleanupMsg::Line(format!(">> {display}")));
                    let needs_root = !am_root && path_needs_root(&m.path);
                    let result: Result<()> = if needs_root {
                        // `interactive` picks the sudo mode, mirroring the
                        // target cleanup path: -n when credentials are cached,
                        // a live prompt when the system won't cache.
                        let mut cmd = std::process::Command::new("sudo");
                        if !interactive {
                            cmd.arg("-n");
                        }
                        let out = cmd
                            .args(["rm", "-rf", "--"])
                            .arg(&m.path)
                            .output();
                        match out {
                            Ok(o) if o.status.success() => Ok(()),
                            Ok(o) => Err(anyhow::anyhow!(
                                "sudo rm exited with {}: {}",
                                o.status,
                                String::from_utf8_lossy(&o.stderr).trim()
                            )),
                            Err(e) => Err(anyhow::anyhow!("running sudo rm: {e}")),
                        }
                    } else if m.is_dir {
                        std::fs::remove_dir_all(&m.path)
                            .map_err(|e| anyhow::anyhow!("{e}"))
                    } else {
                        std::fs::remove_file(&m.path).map_err(|e| anyhow::anyhow!("{e}"))
                    };
                    match result {
                        Ok(()) => {
                            let _ = tx.send(CleanupMsg::Line(format!("   removed {display}")));
                            let _ = tx.send(CleanupMsg::TargetDone {
                                id: format!("\0mark:{display}"),
                                ok: true,
                                reclaimed: m.size,
                                new_size: Some(0),
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(CleanupMsg::Line(format!("   ERROR: {e:#}")));
                            let _ = tx.send(CleanupMsg::TargetDone {
                                id: format!("\0mark:{display}"),
                                ok: false,
                                reclaimed: 0,
                                new_size: None,
                            });
                        }
                    }
                }
                let _ = tx.send(CleanupMsg::AllDone);
            })
            .expect("failed to spawn marked-delete thread")
    }
}

/// Real UID of the current process, read from `/proc/self/status`.
fn own_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|u| u.parse().ok())
        })
        .unwrap_or(0)
}

/// True when a path is owned by another user, so removing it needs root.
/// (Callers already guard on `is_root()`; a missing path needs no sudo.)
fn path_needs_root(p: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(p) {
        Ok(m) => m.uid() != own_uid(),
        Err(_) => false,
    }
}
