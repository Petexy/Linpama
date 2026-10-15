//! The LineXinBar page: the package manager in LineXinBar's own language,
//! driven by controller, keyboard and pointer, with what the GTK page does:
//! the stability warning, a search of the repositories (and the AUR) typed
//! into a field (LineXinBar's on-screen keyboard rises over it with a
//! controller in hand), each result with its own icon, a package's details
//! (install or remove it, change its version, its Arch Wiki article), an AUR
//! package's PKGBUILD to review before it is built, the maintenance actions,
//! and every command's progress and log. Privileged commands go through
//! pkexec, so passwords are asked by the session's polkit agent (on
//! LineXinBar, the shell's own panel with its on-screen keyboard).
//!
//! Where the toolkit cannot take text yet, the page lists the installed
//! packages in place of a search.

use crate::pacman::{self, Package, Version};
use crate::runner::{self, Current, Options, Privilege};
use crate::ui::{self, tr, tr_with};
use crate::wiki;
use linexin_lxb::{
    Action, Emblem, FieldInput, Flow, FlowFrame, Page, SidePanel, WidgetFrame, guard, icon_file,
    page_result,
};
use linexin_plugin_api::{LXB_PAGE_STAY, LxbContextV1, LxbFrameV1};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};

/// The widget's own icon, at the head of the page.
const ICON: &str = "/usr/share/icons/github.petexy.linpama.svg";
/// What stands for a package that has no icon of its own.
const PACKAGE_MARK: &str = "category-other";
/// How many of the last lines a running command printed the page shows
/// while its details are hidden.
const LOG_LINES: usize = 6;
const DB_LOCK: &str = "/var/lib/pacman/db.lck";
/// How many results are listed at once; the next as many as the light comes
/// near the end (the GTK list's batches).
const BATCH: usize = 50;
/// How long typing rests before the search runs (the GTK page's 400 ms).
const SEARCH_AFTER: f32 = 0.4;
/// How many paragraphs of a package's Arch Wiki article the details show.
const WIKI_PARAGRAPHS: usize = 12;
/// The suffixes the GTK page's icon lookup drops from a package's name.
const SUFFIXES: [&str; 5] = ["-bin", "-git", "-nightly", "-stable", "-beta"];

/// What a command run from the page was, for its closing words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Install,
    Remove,
    Refresh,
    ClearCache,
    RemoveOrphans,
    RemoveDbLock,
}

enum Event {
    Log(String),
    Status(String),
    /// The archive's package could not be fetched (the GTK page's
    /// "Download Failed").
    DownloadFailed(String),
    Done(bool),
}

/// A command (or a few) running, or finished and showing how it went.
struct Operation {
    kind: Kind,
    name: String,
    title: String,
    status: String,
    log: Vec<String>,
    events: Receiver<Event>,
    current: Current,
    cancellable: bool,
    done: Option<bool>,
    download_failed: Option<String>,
    /// Every line shown, not only the last few (Show Details).
    details: bool,
    /// A directory to remove once it is over (an AUR package's clone).
    temp: Option<PathBuf>,
}

/// A search running on a thread.
struct Search {
    query: String,
    results: Receiver<Option<Vec<Package>>>,
}

/// A package's details, and its wiki article once it arrives.
struct Details {
    package: Package,
    wiki: Option<Receiver<Option<wiki::Article>>>,
    /// The article's address and its first paragraphs.
    article: Option<(String, Vec<String>)>,
}

/// An AUR package being cloned, then its PKGBUILD to review.
struct Review {
    package: String,
    temp: PathBuf,
    clone: Option<Receiver<Result<String, String>>>,
    current: Current,
    /// The PKGBUILD, or why there is none.
    script: Option<Result<String, String>>,
}

/// A question of this page's own that is on screen.
enum Question {
    Orphans(Vec<String>),
    Note,
}

/// A menu of this page's own that is on screen.
enum Menu {
    /// Install or remove a listed package.
    Row(Package),
    Versions(String, Vec<Version>),
}

struct State {
    /// The search page and, in a wide window, the actions beside it (the
    /// GTK page's right pane); the other pages share `other`.
    panel: SidePanel,
    other: Flow,
    warning: bool,
    dont_warn: bool,
    query: String,
    aur: bool,
    /// When the field was last typed into, until the search runs.
    typed_at: Option<f32>,
    /// The newest search's number; older ones stop as soon as they notice.
    counter: Arc<AtomicU64>,
    searching: Option<Search>,
    results: Vec<Package>,
    /// The query the results answer.
    answered: String,
    shown: usize,
    icons: HashMap<String, Option<PathBuf>>,
    flatpak_ids: HashMap<String, Vec<String>>,
    flatpak_loading: Option<Receiver<HashMap<String, Vec<String>>>>,
    details: Option<Details>,
    review: Option<Review>,
    operation: Option<Operation>,
    orphans: Option<Receiver<String>>,
    versions: Option<(String, Receiver<Vec<Version>>)>,
    asking: Option<Question>,
    menu: Option<Menu>,
}

pub unsafe extern "C" fn create(context: *const LxbContextV1) -> *mut c_void {
    guard(ptr::null_mut(), || {
        // SAFETY: the pointer the host passed to `lxb_create`.
        if unsafe { LxbContextV1::read(context) }.is_none() {
            return ptr::null_mut();
        }
        let mut state = State::new(ui::should_show_warning());
        let (sender, loaded) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("linpama-flatpak".into())
            .spawn(move || {
                let _ = sender.send(pacman::flatpak_suffixes());
            });
        if worker.is_ok() {
            state.flatpak_loading = Some(loaded);
        }
        state.search();
        Box::into_raw(Box::new(state)).cast()
    })
}

pub unsafe extern "C" fn page(state: *mut c_void, frame: *const LxbFrameV1) -> u32 {
    guard(LXB_PAGE_STAY, || {
        // SAFETY: the state from `create`, alive for the call.
        let Some(state) = (unsafe { state.cast::<State>().as_mut() }) else {
            return LXB_PAGE_STAY;
        };
        // SAFETY: the host's frame, read for this call only.
        let Some(frame) = (unsafe { WidgetFrame::read(frame) }) else {
            return LXB_PAGE_STAY;
        };
        state.draw(&frame)
    })
}

pub unsafe extern "C" fn destroy(state: *mut c_void) {
    guard((), || {
        if !state.is_null() {
            // SAFETY: the state from `create`, released once. Commands still
            // running finish on their own threads; their answers are dropped.
            drop(unsafe { Box::from_raw(state.cast::<State>()) });
        }
    })
}

/// What pressing something on the search page asks for.
enum Press {
    Refresh,
    ClearCache,
    Orphans,
    DbLock,
    Package(usize),
}

impl State {
    fn new(warning: bool) -> Self {
        Self {
            panel: SidePanel::new(),
            other: Flow::new(),
            warning,
            dont_warn: false,
            query: String::new(),
            aur: false,
            typed_at: None,
            counter: Arc::default(),
            searching: None,
            results: Vec::new(),
            answered: String::new(),
            shown: BATCH,
            icons: HashMap::new(),
            flatpak_ids: HashMap::new(),
            flatpak_loading: None,
            details: None,
            review: None,
            operation: None,
            orphans: None,
            versions: None,
            asking: None,
            menu: None,
        }
    }

    fn draw(&mut self, frame: &WidgetFrame) -> u32 {
        let page = &frame.page;
        self.collect(page);
        self.take_answers(page);
        if self.searching.is_some()
            || self.typed_at.is_some()
            || self.orphans.is_some()
            || self.versions.is_some()
            || self
                .details
                .as_ref()
                .is_some_and(|details| details.wiki.is_some())
            || self
                .review
                .as_ref()
                .is_some_and(|review| review.clone.is_some())
            || self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.done.is_none())
        {
            page.redraw_within(0.2);
        }
        if self.warning {
            self.warning_page(frame)
        } else if self.operation.is_some() {
            self.operation_page(frame)
        } else if self.review.is_some() {
            self.review_page(frame)
        } else if self.details.is_some() {
            self.details_page(frame)
        } else {
            self.search_page(frame)
        }
    }

    // -- the pages ----------------------------------------------------------------------------

    /// The GTK page's stability warning, before anything else.
    fn warning_page(&mut self, frame: &WidgetFrame) -> u32 {
        let page = &frame.page;
        let input = if frame.focused {
            self.other.input(page, &frame.actions)
        } else {
            Default::default()
        };
        let store = app_store();
        let (mut open_store, mut go_on, mut flip) = (false, false, false);
        let icon = icon_file(ICON);
        let mut flow = self.other.begin(page, frame.rect, frame.focused);
        flow.head(
            Emblem::with_picture(icon.as_deref(), "category-system"),
            &tr("System Stability Warning"),
        );
        for paragraph in without_markup(&tr(ui::WARNING_TEXT)).split("\n\n") {
            flow.text(paragraph.trim());
        }
        if let Some((_, label)) = &store
            && flow.entry("launch", label, None)
        {
            open_store = true;
        }
        if flow.entry("chosen", &tr("I Understand, Continue"), None) {
            go_on = true;
        }
        if flow.switch("", &tr("Do not show this warning again"), self.dont_warn) {
            flip = true;
        }
        flow.finish();
        if flip {
            self.dont_warn = !self.dont_warn;
        }
        if open_store && let Some((command, _)) = store {
            spawn_detached(&[command]);
        }
        if go_on {
            if self.dont_warn {
                ui::save_warning_preference(false);
            }
            self.warning = false;
            self.other.reset();
        }
        page_result(input.leave.is_some())
    }

    /// The search page: the field, the AUR switch and the packages, with
    /// the maintenance actions in a panel beside them in a wide window (the
    /// GTK page's right pane, from 900 pixels) or above the installed
    /// packages in a narrow one.
    fn search_page(&mut self, frame: &WidgetFrame) -> u32 {
        let page = &frame.page;
        let now = page.seconds();
        let side_width = SidePanel::width(page, frame.rect);
        let (main_rect, side_rect) =
            SidePanel::split(page, frame.rect, side_width, page.scaled(600.0));
        let wide = side_rect.is_some();
        let (input, side_input) = self.panel.input(page, &frame.actions, frame.focused, wide);
        let (main_lit, side_lit) = self.panel.lit(frame.focused, wide);
        let typing = page.takes_typing();
        let shown = self.shown.min(self.results.len());
        {
            let (ids, icons) = (&self.flatpak_ids, &mut self.icons);
            for package in &self.results[..shown] {
                icons
                    .entry(package.name.clone())
                    .or_insert_with(|| package_icon(&package.name, ids));
            }
        }
        let searching_for = !self.query.trim().is_empty();
        let no_description = tr("No description available.");
        let mut pressed = None;
        let mut field = FieldInput::default();
        let mut flip_aur = false;
        // The controls before the first package, to know which one the
        // light stands on.
        let mut before = 0;
        let icon = icon_file(ICON);
        let mut flow = self.panel.main.begin(page, main_rect, main_lit);
        flow.head(
            Emblem::with_picture(icon.as_deref(), "category-system"),
            &tr("Package Manager"),
        );
        if typing {
            field = flow.field(
                "search",
                &tr("Search for packages..."),
                &mut self.query,
                false,
            );
            flip_aur = flow.switch("", &tr("Search AUR"), self.aur);
            before += 2;
        }
        if searching_for {
            flow.heading(&tr("Search Packages"));
        } else {
            if !wide {
                pressed = actions(&mut flow);
                before += 4;
                flow.gap();
            }
            flow.heading(&tr("Installed"));
        }
        if self.searching.is_some() && self.results.is_empty() {
            flow.note(&tr("Searching..."));
        } else if self.results.is_empty() && !self.answered.is_empty() {
            flow.text(&tr("No Results Found"));
            flow.note(&tr("Try refining your search terms."));
        }
        for (index, package) in self.results[..shown].iter().enumerate() {
            let picture = self
                .icons
                .get(&package.name)
                .and_then(|path| path.as_deref());
            let description = if package.desc.is_empty() {
                &no_description
            } else {
                &package.desc
            };
            if flow.detailed(
                Emblem::with_picture(picture, PACKAGE_MARK),
                &package.name,
                description,
                Some(&row_value(package)),
            ) {
                pressed = Some(Press::Package(index));
            }
        }
        flow.finish();
        if let Some(side) = side_rect {
            // Level with the page's content, under its heading.
            let pinned = self.panel.main.pinned();
            let side = [
                side[0],
                side[1] + pinned,
                side[2],
                (side[3] - pinned).max(0.0),
            ];
            let mut flow = self.panel.side.begin(page, side, side_lit);
            pressed = pressed.or(actions(&mut flow));
            flow.finish();
        }
        let leave = self.panel.settle(&input, &side_input, wide);

        let focused = self.panel.main.focused().unwrap_or(0);
        // The next batch as the light comes near the end of this one.
        if shown < self.results.len() && focused + 3 >= before + shown {
            self.shown += BATCH;
        }
        if field.changed {
            self.typed_at = Some(now);
        }
        if field.submitted || flip_aur {
            if flip_aur {
                self.aur = !self.aur;
            }
            self.typed_at = None;
            self.search();
        } else if self.typed_at.is_some_and(|at| now - at >= SEARCH_AFTER) {
            self.typed_at = None;
            self.search();
        }
        if input.unhandled.contains(&Action::Menu)
            && let Some(package) = focused
                .checked_sub(before)
                .and_then(|index| self.results.get(index))
        {
            let command = if package.installed {
                tr("Remove")
            } else {
                tr("Install")
            };
            page.menu(Some(&package.name), &[&command]);
            self.menu = Some(Menu::Row(package.clone()));
        }
        match pressed {
            Some(Press::Refresh) => self.refresh(),
            Some(Press::ClearCache) => self.clear_cache(),
            Some(Press::Orphans) => self.find_orphans(),
            Some(Press::DbLock) => self.remove_db_lock(),
            Some(Press::Package(index)) => {
                if let Some(package) = self.results.get(index).cloned() {
                    self.open_details(package);
                }
            }
            None => {}
        }
        page_result(leave)
    }

    /// A package's details: what the GTK page's detail pane shows and does.
    fn details_page(&mut self, frame: &WidgetFrame) -> u32 {
        let page = &frame.page;
        let input = if frame.focused {
            self.other.input(page, &frame.actions)
        } else {
            Default::default()
        };
        let Some(details) = &self.details else {
            return LXB_PAGE_STAY;
        };
        let package = details.package.clone();
        let article = details.article.clone();
        let picture = self.icons.get(&package.name).cloned().flatten();
        let (mut act, mut versions, mut wiki_open, mut back) = (false, false, false, false);
        let mut flow = self.other.begin(page, frame.rect, frame.focused);
        flow.head(
            Emblem::with_picture(picture.as_deref(), PACKAGE_MARK),
            &package.name,
        );
        flow.note(&format!("{}  \u{b7}  {}", package.version, package.repo));
        let status = if package.installed {
            format!("\u{25cf} {}", tr("Installed"))
        } else {
            format!("\u{25cb} {}", tr("Not installed"))
        };
        flow.note(&if package.is_aur {
            format!("{status}  \u{b7}  AUR")
        } else {
            status
        });
        let (mark, label) = if package.installed {
            ("uninstall", tr("Remove"))
        } else {
            ("add", tr("Install"))
        };
        if flow.entry(mark, &label, None) {
            act = true;
        }
        if !package.is_aur {
            let label = if package.installed {
                tr("Change Version")
            } else {
                tr("Install Older Version")
            };
            let waiting = self.versions.is_some().then(|| tr("Please wait..."));
            versions = flow.entry("sort", &label, waiting.as_deref());
        }
        if let Some((url, _)) = &article {
            wiki_open = flow.entry("launch", &tr("Arch Wiki"), Some(url));
        }
        if flow.entry("arrow-left", &tr("Back"), None) {
            back = true;
        }
        flow.gap();
        let description = if package.desc.is_empty() {
            tr("No description available.")
        } else {
            package.desc.clone()
        };
        flow.text(&description);
        if let Some((_, paragraphs)) = &article {
            flow.gap();
            flow.heading(&tr("Arch Wiki"));
            for paragraph in paragraphs {
                flow.text(paragraph);
            }
        }
        flow.finish();

        if act {
            if package.installed {
                self.remove(package.name.clone());
            } else {
                self.install(package.clone());
            }
        }
        if versions {
            self.find_versions(&package.name);
        }
        if wiki_open && let Some((url, _)) = &article {
            spawn_detached(&["xdg-open", url]);
        }
        if back || input.leave.is_some() {
            self.details = None;
            self.other.reset();
        }
        LXB_PAGE_STAY
    }

    /// An AUR package's PKGBUILD, to read before it is built.
    fn review_page(&mut self, frame: &WidgetFrame) -> u32 {
        let page = &frame.page;
        let input = if frame.focused {
            self.other.input(page, &frame.actions)
        } else {
            Default::default()
        };
        let Some(review) = &self.review else {
            return LXB_PAGE_STAY;
        };
        let mut build = false;
        let mut flow = self.other.begin(page, frame.rect, frame.focused);
        flow.head("file-page", &tr("Review PKGBUILD"));
        let cancel = match &review.script {
            None => {
                flow.text(&tr("Cloning repository..."));
                flow.note(&tr_with("Fetching {} from AUR...", &review.package));
                flow.entry("key-escape", &tr("Cancel"), None)
            }
            Some(Err(message)) => {
                flow.text(message);
                flow.entry("arrow-left", &tr("Back"), None)
            }
            Some(Ok(script)) => {
                flow.text(&tr(
                    "You are about to build a package from the AUR. Please review the build script carefully for malicious code.",
                ));
                build = flow.entry("launch", &tr("Proceed to Build"), None);
                let cancel = flow.entry("key-escape", &tr("Cancel"), None);
                flow.gap();
                for line in script.lines() {
                    flow.note(if line.trim().is_empty() { " " } else { line });
                }
                cancel
            }
        };
        flow.finish();
        if build {
            self.build();
        } else if cancel || input.leave.is_some() {
            if let Some(review) = self.review.take() {
                review.current.interrupt();
                if review.temp.exists() {
                    let _ = std::fs::remove_dir_all(&review.temp);
                }
            }
            self.other.reset();
        }
        LXB_PAGE_STAY
    }

    /// A command's progress, then how it went, with its log.
    fn operation_page(&mut self, frame: &WidgetFrame) -> u32 {
        let page = &frame.page;
        let input = if frame.focused {
            self.other.input(page, &frame.actions)
        } else {
            Default::default()
        };
        let Some(operation) = &self.operation else {
            return LXB_PAGE_STAY;
        };
        let (mut cancel, mut done) = (false, false);
        let icon = icon_file(ICON);
        let mut flow = self.other.begin(page, frame.rect, frame.focused);
        let title = match (&operation.download_failed, operation.done) {
            (Some(_), _) => tr("Download Failed"),
            (None, Some(true)) => finished_message(operation, true),
            _ => operation.title.clone(),
        };
        flow.head(
            Emblem::with_picture(icon.as_deref(), "category-system"),
            &title,
        );
        match (&operation.download_failed, operation.done) {
            (Some(error), _) => flow.text(error),
            (None, None) => flow.text(&operation.status),
            (None, Some(true)) => {}
            (None, Some(false)) => flow.text(&finished_message(operation, false)),
        }
        match operation.done {
            None if operation.cancellable => {
                cancel = flow.entry("key-escape", &tr("Cancel"), None);
            }
            None => {}
            Some(true) => done = flow.entry("search", &tr("Search Again"), None),
            Some(false) => done = flow.entry("arrow-left", &tr("Back"), None),
        }
        let details = if operation.details {
            tr("Hide Details")
        } else {
            tr("Show Details")
        };
        let toggle = flow.entry("file-page", &details, None);
        flow.gap();
        let lines = if operation.details {
            &operation.log[..]
        } else {
            &operation.log[operation.log.len().saturating_sub(LOG_LINES)..]
        };
        for line in lines {
            flow.note(line);
        }
        if operation.done == Some(false) && operation.details {
            flow.note(&tr("Transaction Failed."));
        }
        flow.finish();

        let finished = operation.done.is_some() || operation.download_failed.is_some();
        if toggle && let Some(operation) = &mut self.operation {
            operation.details = !operation.details;
        }
        if cancel
            && let Some(operation) = &mut self.operation
            && operation.current.interrupt()
        {
            operation.log.push(tr("--- Cancelling operation... ---"));
        }
        if (done || input.leave.is_some()) && finished {
            self.close_operation();
        }
        LXB_PAGE_STAY
    }

    // -- what arrives --------------------------------------------------------------------

    /// Answers to this page's questions and menus, on the frame they come.
    fn take_answers(&mut self, page: &Page) {
        if self.menu.is_some()
            && let Some(choice) = page.chose()
        {
            match self.menu.take() {
                Some(Menu::Row(package)) if choice == 0 => {
                    if package.installed {
                        self.remove(package.name);
                    } else {
                        self.install(package);
                    }
                }
                Some(Menu::Versions(package, versions)) => {
                    if let Some(version) = versions.get(choice) {
                        self.install_version(&package, version);
                    }
                }
                _ => {}
            }
        }
        if self.asking.is_some()
            && let Some(answer) = page.answered()
            && let Some(Question::Orphans(orphans)) = self.asking.take()
            && answer == 1
        {
            self.remove_orphans(orphans);
        }
    }

    /// Searches answered, articles fetched, lines printed, commands done.
    fn collect(&mut self, page: &Page) {
        if let Some(loading) = &self.flatpak_loading {
            match loading.try_recv() {
                Ok(ids) => {
                    self.flatpak_ids = ids;
                    self.flatpak_loading = None;
                    // Asked again with the Flatpaks' names known.
                    self.icons.retain(|_, found| found.is_some());
                }
                Err(TryRecvError::Disconnected) => self.flatpak_loading = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(search) = &self.searching {
            match search.results.try_recv() {
                Ok(Some(results)) => {
                    self.results = results;
                    self.answered = search.query.clone();
                    self.shown = BATCH;
                    self.searching = None;
                }
                Ok(None) | Err(TryRecvError::Disconnected) => self.searching = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(details) = &mut self.details
            && let Some(wiki) = &details.wiki
        {
            match wiki.try_recv() {
                Ok(article) => {
                    details.article = article.map(|article| {
                        let paragraphs = wiki_paragraphs(&article.html);
                        (article.url, paragraphs)
                    });
                    details.wiki = None;
                }
                Err(TryRecvError::Disconnected) => details.wiki = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(review) = &mut self.review
            && let Some(clone) = &review.clone
        {
            match clone.try_recv() {
                Ok(script) => {
                    review.script = Some(script);
                    review.clone = None;
                }
                Err(TryRecvError::Disconnected) => {
                    review.script = Some(Err("Clone failed or cancelled.".into()));
                    review.clone = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some((package, found)) = &self.versions {
            let versions = match found.try_recv() {
                Ok(versions) => Some(versions),
                Err(TryRecvError::Disconnected) => Some(Vec::new()),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(versions) = versions {
                let package = package.clone();
                self.versions = None;
                self.offer_versions(page, package, versions);
            }
        }
        if let Some(found) = &self.orphans {
            let orphans = match found.try_recv() {
                Ok(orphans) => Some(orphans),
                Err(TryRecvError::Disconnected) => Some(String::new()),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(orphans) = orphans {
                self.orphans = None;
                self.ask_about_orphans(page, &orphans);
            }
        }
        if let Some(operation) = &mut self.operation {
            loop {
                match operation.events.try_recv() {
                    Ok(Event::Log(line)) => {
                        let line = line.trim_end().to_owned();
                        if !line.is_empty() {
                            operation.log.push(line);
                        }
                    }
                    Ok(Event::Status(status)) => operation.status = status,
                    Ok(Event::DownloadFailed(error)) => operation.download_failed = Some(error),
                    Ok(Event::Done(success)) => {
                        operation.done = Some(success);
                        if let Some(temp) = operation.temp.take()
                            && temp.exists()
                        {
                            let _ = std::fs::remove_dir_all(temp);
                        }
                        if !success && operation.kind == Kind::Refresh {
                            page.ask(
                                &tr("Update Failed"),
                                &tr("Repository update failed"),
                                &[&tr("OK")],
                            );
                            self.asking = Some(Question::Note);
                        }
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        operation.done.get_or_insert(false);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
        }
    }

    // -- what the page does --------------------------------------------------------------

    /// Search for the field's words (the installed packages for none), with
    /// the AUR when its switch is on; a newer search stops an older one.
    fn search(&mut self) {
        let query = self.query.trim().to_owned();
        let aur = self.aur;
        let id = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let counter = self.counter.clone();
        let (sender, results) = mpsc::channel();
        let asked = query.clone();
        let worker = std::thread::Builder::new()
            .name("linpama-search".into())
            .spawn(move || {
                let stale = || counter.load(Ordering::SeqCst) != id;
                let _ = sender.send(pacman::search(&asked, aur, &stale));
            });
        if worker.is_ok() {
            self.searching = Some(Search { query, results });
        }
    }

    fn open_details(&mut self, package: Package) {
        let (sender, wiki) = mpsc::channel();
        let name = package.name.clone();
        let worker = std::thread::Builder::new()
            .name("linpama-wiki".into())
            .spawn(move || {
                let _ = sender.send(wiki::fetch(&name));
            });
        self.details = Some(Details {
            package,
            wiki: worker.is_ok().then_some(wiki),
            article: None,
        });
        self.other.reset();
    }

    fn find_versions(&mut self, package: &str) {
        if self.versions.is_some() {
            return;
        }
        let (sender, found) = mpsc::channel();
        let (name, cache, archive) = (package.to_owned(), tr("cache"), tr("archive"));
        let worker = std::thread::Builder::new()
            .name("linpama-versions".into())
            .spawn(move || {
                let _ = sender.send(pacman::versions(&name, &cache, &archive));
            });
        if worker.is_ok() {
            self.versions = Some((package.to_owned(), found));
        }
    }

    /// The GTK page's version dialog, as a menu (the installed one marked).
    fn offer_versions(&mut self, page: &Page, package: String, versions: Vec<Version>) {
        if versions.is_empty() {
            page.ask(
                &tr("No Versions Found"),
                &tr_with("No alternative versions found for {}.", &package),
                &[&tr("OK")],
            );
            self.asking = Some(Question::Note);
            return;
        }
        let labels: Vec<String> = versions
            .iter()
            .map(|version| format!("{}  \u{b7}  {}", version.ver, version.source))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let current = versions.iter().position(|version| version.current);
        page.menu_marked(
            Some(&tr_with("Select a version for {}:", &package)),
            &labels,
            current,
        );
        self.menu = Some(Menu::Versions(package, versions));
    }

    /// Start `work` on a worker thread as the page's operation.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        kind: Kind,
        name: String,
        title: String,
        status: String,
        cancellable: bool,
        temp: Option<PathBuf>,
        work: impl FnOnce(&Sender<Event>, &Current) -> bool + Send + 'static,
    ) {
        if self.operation.is_some() {
            return;
        }
        let (sender, events) = mpsc::channel();
        let current = Current::default();
        let worker_current = current.clone();
        let worker = std::thread::Builder::new()
            .name("linpama".into())
            .spawn(move || {
                let success = work(&sender, &worker_current);
                let _ = sender.send(Event::Done(success));
            });
        if worker.is_err() {
            return;
        }
        self.operation = Some(Operation {
            kind,
            name,
            title,
            status,
            log: Vec::new(),
            events,
            current,
            cancellable,
            done: None,
            download_failed: None,
            details: false,
            temp,
        });
        self.other.reset();
    }

    /// Back from a finished command. After one that worked the search starts
    /// over, as the GTK page empties its field.
    fn close_operation(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        if operation.done == Some(true) {
            self.query.clear();
            self.details = None;
        }
        self.review = None;
        self.other.reset();
        self.panel.main.focus(0);
        self.search();
    }

    fn install(&mut self, package: Package) {
        if package.is_aur {
            self.review_aur(&package.name);
            return;
        }
        let name = package.name;
        let argv = ["pkexec", "pacman", "-S", "--noconfirm", "--needed", &name].map(str::to_owned);
        self.start(
            Kind::Install,
            name.clone(),
            tr_with("Installing {}...", &name),
            tr("Downloading and installing..."),
            true,
            None,
            move |events, current| run(&argv, &[], None, events, current),
        );
    }

    fn remove(&mut self, name: String) {
        let argv = ["pkexec", "pacman", "-Rns", "--noconfirm", &name].map(str::to_owned);
        self.start(
            Kind::Remove,
            name.clone(),
            tr_with("Removing {}...", &name),
            tr("Removing package..."),
            true,
            None,
            move |events, current| run(&argv, &[], None, events, current),
        );
    }

    /// The GTK page's `install_specific_version`: a repository's version,
    /// a cached file, or a package fetched from the archive first.
    fn install_version(&mut self, package: &str, version: &Version) {
        let target = match &version.url {
            Some(url) => url.clone(),
            None => format!("{}/{package}", version.source),
        };
        let downloading = tr("Downloading package...");
        self.start(
            Kind::Install,
            package.to_owned(),
            tr_with("Installing {}...", package),
            tr("Changing version..."),
            true,
            None,
            move |events, current| {
                if !target.starts_with("http") {
                    let verb = if target.starts_with('/') { "-U" } else { "-S" };
                    let argv =
                        ["pkexec", "pacman", verb, "--noconfirm", &target].map(str::to_owned);
                    return run(&argv, &[], None, events, current);
                }
                let temp = match ui::make_temp_dir("linpama_") {
                    Ok(temp) => temp,
                    Err(error) => {
                        let _ = events.send(Event::DownloadFailed(error.to_string()));
                        return false;
                    }
                };
                let file = crate::http::unquote(target.rsplit('/').next().unwrap_or(&target));
                let local = temp.join(file);
                let _ = events.send(Event::Status(downloading));
                if let Err(error) = crate::http::download(&target, &local) {
                    let _ = std::fs::remove_dir_all(&temp);
                    let _ = events.send(Event::DownloadFailed(error));
                    return false;
                }
                let local = local.to_string_lossy().into_owned();
                let argv = ["pkexec", "pacman", "-U", "--noconfirm", &local].map(str::to_owned);
                let success = run(&argv, &[], None, events, current);
                let _ = std::fs::remove_dir_all(&temp);
                success
            },
        );
    }

    /// The GTK page's AUR review: the package's repository cloned, its
    /// PKGBUILD shown before anything is built.
    fn review_aur(&mut self, package: &str) {
        let temp = match ui::make_temp_dir("linpama_aur_") {
            Ok(temp) => temp,
            Err(error) => {
                eprintln!("Could not create a directory for {package}: {error}");
                return;
            }
        };
        let (sender, clone) = mpsc::channel();
        let current = Current::default();
        let (name, at, worker_current) = (package.to_owned(), temp.clone(), current.clone());
        let worker = std::thread::Builder::new()
            .name("linpama-aur".into())
            .spawn(move || {
                let _ = sender.send(ui::clone_aur(&name, &at, &worker_current));
            });
        if worker.is_err() {
            let _ = std::fs::remove_dir_all(&temp);
            return;
        }
        self.review = Some(Review {
            package: package.to_owned(),
            temp,
            clone: Some(clone),
            current,
            script: None,
        });
        self.other.reset();
    }

    /// Build the reviewed package: paru where it is installed (asking with
    /// pkexec), else makepkg in the clone (pacman through pkexec).
    fn build(&mut self) {
        let Some(review) = &self.review else {
            return;
        };
        let (package, temp) = (review.package.clone(), review.temp.clone());
        let (argv, cwd): (Vec<String>, Option<PathBuf>) = if pacman::which("paru") {
            (
                ["paru", "-S", "--noconfirm", "--sudo", "pkexec", &package]
                    .map(str::to_owned)
                    .into(),
                None,
            )
        } else {
            (
                ["makepkg", "-si", "--noconfirm"].map(str::to_owned).into(),
                Some(temp.clone()),
            )
        };
        self.start(
            Kind::Install,
            package.clone(),
            tr_with("Building {}...", &package),
            tr("Compiling package..."),
            true,
            Some(temp),
            move |events, current| {
                let env = [("PACMAN_AUTH".to_owned(), "pkexec".to_owned())];
                run(&argv, &env, cwd.as_deref(), events, current)
            },
        );
    }

    fn refresh(&mut self) {
        self.start(
            Kind::Refresh,
            String::new(),
            tr("Refresh Repositories"),
            tr("Please wait..."),
            false,
            None,
            |events, current| run_words(&["pkexec", "pacman", "-Sy"], events, current),
        );
    }

    fn clear_cache(&mut self) {
        let paru = pacman::which("paru");
        self.start(
            Kind::ClearCache,
            tr("cache"),
            tr("Clearing Package Cache..."),
            tr("Removing cached packages..."),
            true,
            None,
            move |events, current| {
                let mut success = run_words(
                    &["pkexec", "pacman", "-Scc", "--noconfirm"],
                    events,
                    current,
                );
                if paru && !run_words(&["paru", "-Scc", "--noconfirm"], events, current) {
                    success = false;
                }
                success
            },
        );
    }

    fn find_orphans(&mut self) {
        if self.orphans.is_some() || self.operation.is_some() {
            return;
        }
        let (sender, found) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("linpama-orphans".into())
            .spawn(move || {
                let _ = sender.send(pacman::orphans());
            });
        if worker.is_ok() {
            self.orphans = Some(found);
        }
    }

    /// The GTK page's orphan dialogs, as LineXinBar questions.
    fn ask_about_orphans(&mut self, page: &Page, orphans: &str) {
        if orphans.is_empty() {
            page.ask(
                &tr("No Orphans Found"),
                &tr("There are no orphan packages to remove."),
                &[&tr("OK")],
            );
            self.asking = Some(Question::Note);
            return;
        }
        let list: Vec<String> = orphans.split('\n').map(str::to_owned).collect();
        let body = format!(
            "{}\n{}",
            tr_with(
                "The following {} packages will be removed:",
                &list.len().to_string()
            ),
            list.join(", ")
        );
        page.ask(
            &tr("Remove Orphan Packages"),
            &body,
            &[&tr("Cancel"), &tr("Remove All")],
        );
        self.asking = Some(Question::Orphans(list));
    }

    fn remove_orphans(&mut self, orphans: Vec<String>) {
        self.start(
            Kind::RemoveOrphans,
            tr("orphans"),
            tr("Removing Orphan Packages..."),
            tr("Removing orphans..."),
            true,
            None,
            move |events, current| {
                let mut argv = vec!["pkexec", "pacman", "-Rns", "--noconfirm"];
                argv.extend(orphans.iter().map(String::as_str));
                run_words(&argv, events, current)
            },
        );
    }

    fn remove_db_lock(&mut self) {
        let (nothing, removed) = (
            tr("No database lock file found. Nothing to do.\n"),
            tr("Database lock file removed successfully.\n"),
        );
        self.start(
            Kind::RemoveDbLock,
            tr("db lock"),
            tr("Removing Database Lock..."),
            tr("Removing /var/lib/pacman/db.lck..."),
            false,
            None,
            move |events, current| {
                if !Path::new(DB_LOCK).exists() {
                    let _ = events.send(Event::Log(nothing));
                    return true;
                }
                let success = run_words(&["pkexec", "rm", "-f", DB_LOCK], events, current);
                if success {
                    let _ = events.send(Event::Log(removed));
                }
                success
            },
        );
    }
}

/// The GTK page's four maintenance actions; the one pressed.
fn actions(flow: &mut FlowFrame) -> Option<Press> {
    flow.heading(&tr("Actions"));
    let actions = [
        (
            "refresh",
            "Refresh Repositories",
            "Sync package databases",
            Press::Refresh,
        ),
        (
            "file-drive",
            "Clear Package Cache",
            "Free disk space from cached packages",
            Press::ClearCache,
        ),
        (
            "uninstall",
            "Remove Orphans",
            "Remove unused dependency packages",
            Press::Orphans,
        ),
        (
            "authenticate",
            "Remove DB Lock",
            "Delete /var/lib/pacman/db.lck",
            Press::DbLock,
        ),
    ];
    let mut pressed = None;
    for (mark, title, subtitle, press) in actions {
        if flow.detailed(mark, &tr(title), &tr(subtitle), None) {
            pressed = Some(press);
        }
    }
    pressed
}

/// One command with its lines sent to the page; whether it exited 0.
fn run(
    argv: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    events: &Sender<Event>,
    current: &Current,
) -> bool {
    let options = Options {
        c_locale: true,
        env: env.to_vec(),
        cwd,
    };
    let log = |line: String| {
        let _ = events.send(Event::Log(line));
    };
    match runner::run_logged(argv, options, Privilege::Polkit, current, &log) {
        Ok(status) => status.success(),
        Err(error) => {
            log(format!("Error: {error}"));
            false
        }
    }
}

fn run_words(argv: &[&str], events: &Sender<Event>, current: &Current) -> bool {
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    run(&argv, &[], None, events, current)
}

/// Start a program and let it go (the store, the browser).
fn spawn_detached(argv: &[&str]) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };
    if let Ok(mut child) = std::process::Command::new(program).args(args).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// The GTK page's closing words for an operation.
fn finished_message(operation: &Operation, success: bool) -> String {
    if !success {
        let log = operation.log.join("\n").to_lowercase();
        return if log.contains("incorrect password") || log.contains("not authorized") {
            tr("Incorrect sudo password. Please try again.")
        } else {
            tr("Failed or Cancelled.")
        };
    }
    match operation.kind {
        Kind::Install => tr_with("Successfully installed {}", &operation.name),
        Kind::Remove => tr_with("Successfully removed {}", &operation.name),
        Kind::ClearCache => tr("Package cache cleared successfully."),
        Kind::RemoveOrphans => tr("Orphan packages removed successfully."),
        Kind::RemoveDbLock => tr("Database lock removed successfully."),
        Kind::Refresh => tr("Sync package databases"),
    }
}

/// A listed package's far end: its version and repository, as the GTK row
/// writes them, and whether it is installed.
fn row_value(package: &Package) -> String {
    let mut value = format!("{} ({})", package.version, package.repo);
    if package.installed {
        value.push_str("  \u{b7}  ");
        value.push_str(&tr("Installed"));
    }
    value
}

/// The store to offer instead of system packages: LineXinBar's Software Hub
/// on LineXinBar, else the GTK page's GNOME Software or Discover.
fn app_store() -> Option<(&'static str, String)> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_uppercase();
    if desktop.contains("LINEXINBAR") && pacman::which("distribumpy") {
        return Some(("distribumpy", tr("Open Software Hub instead")));
    }
    if desktop.contains("GNOME") {
        return Some(("gnome-software", tr("Open GNOME Software instead")));
    } else if desktop.contains("KDE") {
        return Some(("plasma-discover", tr("Open Discover instead")));
    }
    if pacman::which("gnome-software") {
        Some(("gnome-software", tr("Open GNOME Software instead")))
    } else if pacman::which("plasma-discover") {
        Some(("plasma-discover", tr("Open Discover instead")))
    } else {
        None
    }
}

/// A package's own icon, looked up as the GTK page's `resolve_icon_name`
/// does: its name, its name without a `-git`-like suffix, the Flatpaks named
/// like it, then the GTK page's few names of its own; in the icon theme and
/// in Flatpak's AppStream icons.
fn package_icon(name: &str, flatpak: &HashMap<String, Vec<String>>) -> Option<PathBuf> {
    let clean = SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name);
    let mut names = vec![name.to_owned()];
    if clean != name {
        names.push(clean.to_owned());
    }
    names.extend(
        flatpak
            .get(&clean.to_lowercase())
            .cloned()
            .unwrap_or_default(),
    );
    let own = match name {
        "ttf-google-fonts-git" | "noto-fonts" => Some("preferences-desktop-font"),
        "base-devel" => Some("applications-engineering"),
        "linux" => Some("system-run"),
        "networkmanager" => Some("network-workgroup"),
        "code" => Some("visual-studio-code"),
        "steam-native-runtime" => Some("steam"),
        _ => None,
    };
    names.extend(own.map(str::to_owned));
    names
        .iter()
        .find_map(|name| icon_file(name).or_else(|| appstream_icon(name)))
        .or_else(generic_icon)
}

/// The icon themes' `package-x-generic`, the GTK page's last resort: the
/// largest of the themes that are commonly installed.
fn generic_icon() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            [
                "breeze/mimetypes/64/package-x-generic.svg",
                "breeze-dark/mimetypes/64/package-x-generic.svg",
                "Adwaita/scalable/mimetypes/package-x-generic.svg",
                "AdwaitaLegacy/48x48/mimetypes/package-x-generic.png",
                "hicolor/48x48/mimetypes/package-x-generic.png",
            ]
            .iter()
            .find_map(|icon| icon_file(&format!("/usr/share/icons/{icon}")))
        })
        .clone()
}

/// An icon Flatpak's AppStream data keeps for an application id.
fn appstream_icon(id: &str) -> Option<PathBuf> {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    let dirs = DIRS.get_or_init(|| {
        let visible =
            |entry: &std::fs::DirEntry| !entry.file_name().as_encoded_bytes().starts_with(b".");
        let listed = |dir: &Path| -> Vec<std::fs::DirEntry> {
            std::fs::read_dir(dir)
                .map(|entries| entries.flatten().filter(visible).collect())
                .unwrap_or_default()
        };
        let mut dirs = Vec::new();
        for remote in listed(Path::new("/var/lib/flatpak/appstream")) {
            for arch in listed(&remote.path()) {
                let root = arch.path().join("active").join("icons");
                for size in ["128x128", "64x64", "128", "64"] {
                    let dir = root.join(size);
                    if dir.is_dir() {
                        dirs.push(dir);
                    }
                }
            }
        }
        dirs
    });
    dirs.iter()
        .map(|dir| dir.join(format!("{id}.png")))
        .find(|path| path.is_file())
}

/// Pango markup's tags taken out of a translated sentence.
fn without_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for letter in text.chars() {
        match letter {
            '<' => inside = true,
            '>' if inside => inside = false,
            _ if !inside => out.push(letter),
            _ => {}
        }
    }
    out
}

/// The first paragraphs of an Arch Wiki article's HTML, as plain text: the
/// page shows them where the GTK page shows the article itself.
fn wiki_paragraphs(html: &str) -> Vec<String> {
    let mut text = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].trim().to_lowercase();
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|letter| letter.is_ascii_alphanumeric())
            .collect();
        rest = &rest[start + end + 1..];
        // What is not prose is skipped whole.
        if !tag.starts_with('/') && matches!(name.as_str(), "script" | "style" | "table") {
            let close = format!("</{name}");
            // ASCII case only: lowering other letters can change their
            // length, and the offset found must stay one in `rest`.
            match rest.to_ascii_lowercase().find(&close) {
                Some(at) => {
                    rest = &rest[at..];
                    if let Some(end) = rest.find('>') {
                        rest = &rest[end + 1..];
                    }
                }
                None => rest = "",
            }
            text.push('\n');
            continue;
        }
        if matches!(
            name.as_str(),
            "p" | "br"
                | "li"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "pre"
                | "div"
                | "dd"
                | "dt"
                | "tr"
        ) {
            text.push('\n');
        }
    }
    text.push_str(rest);
    entities(&text)
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .take(WIKI_PARAGRAPHS)
        .collect()
}

/// HTML's character references, read.
fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // Within the first 12 bytes; counted in bytes, so a letter of
        // more than one byte after the `&` is not cut through.
        let Some(end) = rest.bytes().take(12).position(|byte| byte == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let letter = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match letter {
            Some(letter) => {
                out.push(letter);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wiki_article_reads_as_paragraphs() {
        let html = "<p><b>Firefox</b> is a popular open source graphical web browser from \
            <a href=\"x\">Mozilla</a>.</p><style>.x{}</style><h2>Installation</h2>\
            <p>Install the <code>firefox</code> package &amp; enjoy&#33;</p><table><tr><td>no</td></tr></table>\
            <pre>pacman -S firefox</pre>";
        assert_eq!(
            wiki_paragraphs(html),
            vec![
                "Firefox is a popular open source graphical web browser from Mozilla.",
                "Installation",
                "Install the firefox package & enjoy!",
                "pacman -S firefox",
            ]
        );
        assert_eq!(
            entities("a &lt;b&gt; &#x41; &unknown; & c"),
            "a <b> A &unknown; & c"
        );
    }

    #[test]
    fn letters_of_more_than_one_byte_do_not_break_the_article() {
        assert_eq!(
            wiki_paragraphs("<p>Ruby&amp;中文字</p>"),
            vec!["Ruby&中文字"]
        );
        assert_eq!(entities("&中文字字"), "&中文字字");
        assert_eq!(
            wiki_paragraphs("<p>a</p><table>éé\u{212A}</table><p>after</p>"),
            vec!["a", "after"]
        );
        assert_eq!(
            wiki_paragraphs("<p>İİ<script>x</SCRIPT>ok</p>"),
            vec!["İİ", "ok"]
        );
    }

    #[test]
    fn markup_is_taken_out() {
        assert_eq!(
            without_markup("use <b>Flatpaks</b> for apps"),
            "use Flatpaks for apps"
        );
    }

    #[test]
    fn rows_say_version_and_repository() {
        let package = Package {
            name: "vlc".into(),
            version: "3.0".into(),
            repo: "extra".into(),
            installed: false,
            ..Package::default()
        };
        assert_eq!(row_value(&package), "3.0 (extra)");
    }

    /// Draws the search page with a few packages, and a package being
    /// removed, to `$LINPAMA_LXB_SHOT_DIR/{idle,working}.png` (headless;
    /// needs `liblxb_app.so`). Skipped without the variable.
    #[test]
    fn page_shots() {
        use linexin_lxb::{App, Overlay, Panes, host_frame};
        let Some(dir) = std::env::var_os("LINPAMA_LXB_SHOT_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let package = |name: &str, version: &str| Package {
            name: name.into(),
            version: version.into(),
            repo: "local".into(),
            installed: true,
            ..Package::default()
        };
        let state = || {
            let mut state = State::new(false);
            state.results = vec![
                package("bash", "5.3.3-2"),
                package("firefox", "143.0.4-1"),
                package("vlc", "3.0.21-21"),
            ];
            state
        };
        let mut idle = state();
        let mut working = state();
        let (_sender, events) = mpsc::channel();
        working.operation = Some(Operation {
            kind: Kind::Remove,
            name: "vlc".into(),
            title: tr_with("Removing {}...", "vlc"),
            status: tr("Removing package..."),
            log: vec![
                "checking dependencies...".into(),
                "(1/1) removing vlc".into(),
            ],
            events,
            current: Current::default(),
            cancellable: true,
            done: None,
            download_failed: None,
            details: false,
            temp: None,
        });
        for (name, state) in [("idle", &mut idle), ("working", &mut working)] {
            let mut app = App::new("linpama-test", "Package Manager")
                .expect("liblxb_app.so")
                .plain()
                .driven();
            let path = dir.join(format!("{name}.png"));
            app.shot(&path, 1600, 1000, 0.5, |page| {
                let panes = Panes::whole(page);
                page.draw_pane(panes.page, Overlay::Dialog);
                let frame = host_frame(page, panes.content, true, &[]);
                // SAFETY: a frame built for this call.
                let frame = unsafe { WidgetFrame::read(&frame) }.expect("frame");
                state.draw(&frame);
            })
            .expect("shot");
            assert!(path.exists());
        }
    }
}
