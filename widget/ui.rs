//! The GTK interface: a port of `LinexinPackageManager`.

use crate::pacman::{self, Package, Version};
use crate::runner::{self, Current, Options, Privilege};
use crate::wiki;
use adw::prelude::*;
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, pango};
use linexin_plugin_api::{self as api, Host};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use webkit::prelude::*;

/// Widget id and translation domain.
pub const DOMAIN: &str = "package_manager";

const APP_NAME: &str = "linpama";
const WIDE_LAYOUT_THRESHOLD: i32 = 900;
const WIDE_LAYOUT_SIDE_PADDING: i32 = 12;
const RIGHT_PANE_MIN_WIDTH: i32 = 300;
const LAYOUT_ANIMATION_DURATION: u32 = 350;
const BATCH_SIZE: usize = 50;
const DB_LOCK: &str = "/var/lib/pacman/db.lck";
/// The stability warning's words (Pango markup).
pub(crate) const WARNING_TEXT: &str = "Installing system packages directly can lead to conflicts and system instability.\n\n\
     It is highly recommended to use <b>Flatpaks</b> for applications, as they are isolated \
     and will not break your core system.";

const CSS: &str = "
            .buttons_all {
                font-size: 14px;
                min-width: 200px;
                min-height: 40px;
            }
            .rounded-list {
                border-radius: 12px;
            }
        ";

const AUR_BADGE: &str = "<span background='#a40000' color='white' size='small'><b> AUR </b></span>";

pub fn tr(text: &str) -> String {
    api::tr(DOMAIN, text)
}

/// `_("... {} ...").format(value)`.
pub fn tr_with(text: &str, value: &str) -> String {
    tr(text).replacen("{}", value, 1)
}

/// GTK calls back through C, where a panic must not unwind.
fn guard(what: &str, callback: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(callback)).is_err() {
        eprintln!("Package Manager: {what} panicked");
    }
}

/// `~/.config/linpama/config.json`.
fn config_file() -> PathBuf {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(glib::home_dir);
    home.join(".config").join(APP_NAME).join("config.json")
}

/// The original `should_show_warning`.
pub(crate) fn should_show_warning() -> bool {
    let Ok(text) = std::fs::read_to_string(config_file()) else {
        return true;
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(config)) => match config.get("show_warning") {
            None => true,
            Some(value) => python_truthy(value),
        },
        _ => true,
    }
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// The original `save_warning_preference`: the config written as
/// `json.dump` writes it (`{"show_warning": false}`).
pub(crate) fn save_warning_preference(show_warning: bool) {
    let path = config_file();
    let result = (|| -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        }
        let mut config = serde_json::Map::new();
        if path.exists() {
            let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
            match serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())? {
                Value::Object(map) => config = map,
                _ => return Err("'list' object does not support item assignment".into()),
            }
        }
        config.insert("show_warning".into(), Value::Bool(show_warning));
        std::fs::write(&path, python_json(&Value::Object(config)))
            .map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        println!("Failed to save config: {error}");
    }
}

/// `json.dumps(value)`: `, ` and `: ` separators, non-ASCII escaped.
fn python_json(value: &Value) -> String {
    match value {
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("{}: {}", python_json_string(key), python_json(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_json).collect::<Vec<_>>().join(", ")
        ),
        Value::String(text) => python_json_string(text),
        other => other.to_string(),
    }
}

fn python_json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `self.action_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Install,
    Remove,
    Refresh,
    ClearCache,
    RemoveOrphans,
    RemoveDbLock,
}

/// What a worker thread tells the page.
enum Event {
    Log(String),
    Status(String),
    Finished(bool, String),
    DownloadFailed(String, String),
}

/// The widgets of one list row (the original's attributes on the row).
struct RowParts {
    row: adw::ActionRow,
    icon: gtk::Image,
    ver_label: gtk::Label,
    badge: gtk::Label,
    action_btn: gtk::Button,
    package: Rc<RefCell<Option<Package>>>,
}

struct Detail {
    header_title: gtk::Label,
    icon: gtk::Image,
    name_label: gtk::Label,
    version_label: gtk::Label,
    status_label: gtk::Label,
    aur_badge: gtk::Label,
    version_btn: gtk::Button,
    install_older_btn: gtk::Button,
    action_btn: gtk::Button,
    desc_label: gtk::Label,
    wiki_box: gtk::Box,
    wiki_spinner: gtk::Spinner,
    webview: webkit::WebView,
}

struct Manager {
    host: Option<Host>,
    window: Option<glib::WeakRef<gtk::Window>>,
    root: glib::WeakRef<gtk::Box>,
    current: Current,
    pulse_timer: RefCell<Option<glib::SourceId>>,
    current_package_name: RefCell<String>,
    action_type: Cell<Option<Action>>,
    search_timer: RefCell<Option<glib::SourceId>>,
    search_counter: Arc<AtomicU64>,
    all_search_results: RefCell<Vec<Package>>,
    store: gio::ListStore,
    displayed_count: Cell<usize>,
    flatpak_suffixes: Arc<Mutex<HashMap<String, Vec<String>>>>,
    wide_layout_enabled: Cell<Option<bool>>,
    last_measured_width: Cell<i32>,
    detail_current: RefCell<Option<Package>>,
    detail_wiki_url: RefCell<Option<String>>,
    wiki_fetch_id: Cell<u64>,
    aur_temp_dir: RefCell<Option<PathBuf>>,
    aur_pkg_name: RefCell<String>,

    top_stack: gtk::Stack,
    content_hbox: gtk::Box,
    right_revealer: gtk::Revealer,
    content_stack: gtk::Stack,
    dont_show_check: gtk::CheckButton,
    search_entry: gtk::SearchEntry,
    aur_check: gtk::CheckButton,
    compact_sidebar_btn: gtk::ToggleButton,
    sidebar_toggled_handler: RefCell<Option<glib::SignalHandlerId>>,
    search_stack: gtk::Stack,
    spinner: gtk::Spinner,
    list_detail_stack: gtk::Stack,
    pkgbuild_buffer: gtk::TextBuffer,
    progress_title: gtk::Label,
    progress_bar: gtk::ProgressBar,
    lbl_progress_status: gtk::Label,
    btn_details: gtk::Button,
    revealer_details: gtk::Revealer,
    output_buffer: gtk::TextBuffer,
    btn_cancel: gtk::Button,
    btn_back: gtk::Button,
    info_icon: gtk::Image,
    info_text: gtk::Label,
    compact_panel_revealer: gtk::Revealer,
    detail: Detail,
}

/// Build the widget (GTK main thread).
pub fn build(host: Option<Host>, window: Option<gtk::Window>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(12);
    root.set_margin_bottom(50);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let manager = Rc::new(Manager {
        host,
        window: window.map(|window| window.downgrade()),
        root: root.downgrade(),
        current: Current::default(),
        pulse_timer: RefCell::default(),
        current_package_name: RefCell::default(),
        action_type: Cell::new(None),
        search_timer: RefCell::default(),
        search_counter: Arc::new(AtomicU64::new(0)),
        all_search_results: RefCell::default(),
        store: gio::ListStore::new::<glib::BoxedAnyObject>(),
        displayed_count: Cell::new(0),
        flatpak_suffixes: Arc::default(),
        wide_layout_enabled: Cell::new(None),
        last_measured_width: Cell::new(0),
        detail_current: RefCell::default(),
        detail_wiki_url: RefCell::default(),
        wiki_fetch_id: Cell::new(0),
        aur_temp_dir: RefCell::default(),
        aur_pkg_name: RefCell::default(),
        top_stack: gtk::Stack::new(),
        content_hbox: gtk::Box::new(gtk::Orientation::Horizontal, 0),
        right_revealer: gtk::Revealer::new(),
        content_stack: gtk::Stack::new(),
        dont_show_check: gtk::CheckButton::with_label(&tr("Do not show this warning again")),
        search_entry: gtk::SearchEntry::new(),
        aur_check: gtk::CheckButton::with_label(&tr("Search AUR")),
        compact_sidebar_btn: gtk::ToggleButton::new(),
        sidebar_toggled_handler: RefCell::default(),
        search_stack: gtk::Stack::new(),
        spinner: gtk::Spinner::new(),
        list_detail_stack: gtk::Stack::new(),
        pkgbuild_buffer: gtk::TextBuffer::new(None),
        progress_title: gtk::Label::new(Some(&tr("Processing..."))),
        progress_bar: gtk::ProgressBar::new(),
        lbl_progress_status: gtk::Label::new(Some(&tr("Please wait..."))),
        btn_details: gtk::Button::with_label(&tr("Show Details")),
        revealer_details: gtk::Revealer::new(),
        output_buffer: gtk::TextBuffer::new(None),
        btn_cancel: gtk::Button::with_label(&tr("Cancel")),
        btn_back: gtk::Button::with_label(&tr("Back to Search")),
        info_icon: gtk::Image::new(),
        info_text: gtk::Label::new(None),
        compact_panel_revealer: gtk::Revealer::new(),
        detail: new_detail(),
    });

    manager.setup_custom_styles();
    manager.setup_appstream_icon_paths();
    {
        let suffixes = manager.flatpak_suffixes.clone();
        std::thread::spawn(move || {
            let map = pacman::flatpak_suffixes();
            if let Ok(mut shared) = suffixes.lock() {
                *shared = map;
            }
        });
    }

    let top_stack = &manager.top_stack;
    top_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    top_stack.set_hexpand(true);
    top_stack.set_vexpand(true);
    manager.content_hbox.set_hexpand(true);
    manager.content_hbox.set_vexpand(true);
    let revealer = &manager.right_revealer;
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
    revealer.set_transition_duration(LAYOUT_ANIMATION_DURATION);
    revealer.set_reveal_child(true);
    revealer.set_hexpand(false);
    revealer.set_vexpand(true);
    let content_stack = &manager.content_stack;
    content_stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);
    root.append(top_stack);

    manager.setup_warning_view();
    manager.setup_search_view();
    manager.setup_pkgbuild_view();
    manager.setup_progress_view();
    manager.setup_info_view();
    manager.setup_right_pane();
    manager.update_adaptive_layout(true, None);
    {
        let weak = Rc::downgrade(&manager);
        glib::timeout_add_local(Duration::from_millis(200), move || match weak.upgrade() {
            Some(manager) => {
                guard("layout", || manager.monitor_adaptive_layout());
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }
    {
        let webview = manager.detail.webview.clone();
        root.connect_unrealize(move |_| {
            webview.stop_loading();
            webview.load_uri("about:blank");
            webview.terminate_web_process();
        });
    }
    if should_show_warning() {
        manager.top_stack.set_visible_child_name("warning_view");
    } else {
        manager.top_stack.set_visible_child_name("main_view");
        manager.content_stack.set_visible_child_name("search_view");
    }

    // The root owns the state; everything else holds it weakly, so the widget
    // and its state go away together.
    let keep = RefCell::new(Some(manager));
    root.connect_destroy(move |_| {
        keep.take();
    });
    root
}

fn new_detail() -> Detail {
    let settings = webkit::Settings::new();
    settings.set_enable_javascript(false);
    settings.set_enable_media(false);
    let content_manager = webkit::UserContentManager::new();
    content_manager.add_style_sheet(&webkit::UserStyleSheet::new(
        wiki::WIKI_CSS,
        webkit::UserContentInjectedFrames::AllFrames,
        webkit::UserStyleLevel::User,
        &[],
        &[],
    ));
    let webview = glib::Object::builder::<webkit::WebView>()
        .property("settings", &settings)
        .property("user-content-manager", &content_manager)
        .build();
    webview.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
    webview.set_vexpand(true);
    Detail {
        header_title: gtk::Label::new(None),
        icon: gtk::Image::new(),
        name_label: gtk::Label::new(None),
        version_label: gtk::Label::new(None),
        status_label: gtk::Label::new(None),
        aur_badge: gtk::Label::new(None),
        version_btn: gtk::Button::new(),
        install_older_btn: gtk::Button::new(),
        action_btn: gtk::Button::new(),
        desc_label: gtk::Label::new(None),
        wiki_box: gtk::Box::new(gtk::Orientation::Vertical, 0),
        wiki_spinner: gtk::Spinner::new(),
        webview,
    }
}

impl Manager {
    fn connect_clicked(
        self: &Rc<Self>,
        button: &gtk::Button,
        what: &'static str,
        then: fn(&Rc<Self>),
    ) {
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(manager) = weak.upgrade() {
                guard(what, || then(&manager));
            }
        });
    }

    fn connect_activated(
        self: &Rc<Self>,
        row: &adw::ActionRow,
        what: &'static str,
        then: fn(&Rc<Self>),
    ) {
        let weak = Rc::downgrade(self);
        row.connect_activated(move |_| {
            if let Some(manager) = weak.upgrade() {
                guard(what, || then(&manager));
            }
        });
    }

    fn setup_custom_styles(&self) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    /// `self.get_root() or self.window`.
    fn parent_window(&self) -> Option<gtk::Window> {
        self.root
            .upgrade()
            .and_then(|root| root.root())
            .and_then(|root| root.downcast::<gtk::Window>().ok())
            .or_else(|| self.window.as_ref().and_then(|window| window.upgrade()))
    }

    /// The injected `translate_dialog` of the original.
    fn translate_dialog(&self, dialog: &adw::MessageDialog) {
        if let Some(host) = self.host {
            // SAFETY: a live GtkWidget on the GTK thread.
            unsafe {
                host.translate_widget(DOMAIN, dialog.upcast_ref::<gtk::Widget>().as_ptr().cast())
            };
        }
    }

    fn privilege(&self) -> Privilege {
        Privilege::Sudo(self.host)
    }

    /// A dialog with one OK, as the original made several.
    fn message(&self, heading: &str, body: &str) {
        let dialog =
            adw::MessageDialog::new(self.parent_window().as_ref(), Some(heading), Some(body));
        dialog.add_response("ok", &tr("OK"));
        self.translate_dialog(&dialog);
        dialog.present();
    }

    // Warning

    /// The original `get_app_store_info`.
    fn app_store_info() -> Option<(&'static str, String)> {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_uppercase();
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

    fn setup_warning_view(self: &Rc<Self>) {
        let warn_box = gtk::Box::new(gtk::Orientation::Vertical, 15);
        warn_box.set_valign(gtk::Align::Center);
        warn_box.set_halign(gtk::Align::Center);
        warn_box.set_margin_start(30);
        warn_box.set_margin_end(30);
        let icon = gtk::Image::from_icon_name("dialog-warning");
        icon.set_pixel_size(64);
        icon.add_css_class("warning");
        warn_box.append(&icon);
        let title = gtk::Label::new(Some(&tr("System Stability Warning")));
        title.add_css_class("title-2");
        warn_box.append(&title);
        let desc = gtk::Label::new(Some(&tr(WARNING_TEXT)));
        desc.set_use_markup(true);
        desc.set_wrap(true);
        desc.set_justify(gtk::Justification::Center);
        desc.add_css_class("body");
        warn_box.append(&desc);
        let btn_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        btn_box.set_margin_top(20);
        btn_box.set_halign(gtk::Align::Center);
        if let Some((command, label)) = Self::app_store_info() {
            let store_btn = gtk::Button::with_label(&label);
            store_btn.add_css_class("suggested-action");
            store_btn.add_css_class("buttons_all");
            store_btn.set_halign(gtk::Align::Center);
            store_btn.connect_clicked(move |_| {
                if let Ok(mut child) = std::process::Command::new(command).spawn() {
                    std::thread::spawn(move || child.wait());
                }
            });
            btn_box.append(&store_btn);
        }
        let continue_btn = gtk::Button::with_label(&tr("I Understand, Continue"));
        continue_btn.add_css_class("flat");
        continue_btn.add_css_class("buttons_all");
        continue_btn.set_halign(gtk::Align::Center);
        self.connect_clicked(&continue_btn, "continue", |manager| {
            manager.on_warning_continue()
        });
        btn_box.append(&continue_btn);
        self.dont_show_check.set_halign(gtk::Align::Center);
        btn_box.append(&self.dont_show_check);
        warn_box.append(&btn_box);
        self.top_stack.add_named(&warn_box, Some("warning_view"));
    }

    fn on_warning_continue(&self) {
        if self.dont_show_check.is_active() {
            save_warning_preference(false);
        }
        self.top_stack.set_visible_child_name("main_view");
        self.content_stack.set_visible_child_name("search_view");
    }

    // Search

    fn setup_search_view(self: &Rc<Self>) {
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        search_box.set_margin_start(30);
        search_box.set_margin_end(30);
        let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        self.search_entry
            .set_placeholder_text(Some(&tr("Search for packages...")));
        self.search_entry.set_hexpand(true);
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |_| {
            if let Some(manager) = weak.upgrade() {
                guard("search", || manager.on_search_changed());
            }
        });
        header_box.append(&self.search_entry);
        self.aur_check.set_tooltip_text(Some(&tr(
            "Search Arch User Repository (Unstable/Community packages)",
        )));
        let weak = Rc::downgrade(self);
        self.aur_check.connect_toggled(move |_| {
            if let Some(manager) = weak.upgrade() {
                guard("search", || manager.on_search_changed());
            }
        });
        header_box.append(&self.aur_check);
        self.compact_sidebar_btn
            .set_icon_name("sidebar-show-right-symbolic");
        self.compact_sidebar_btn
            .set_tooltip_text(Some(&tr("Show Actions")));
        self.compact_sidebar_btn.set_active(true);
        let weak = Rc::downgrade(self);
        let handler = self.compact_sidebar_btn.connect_toggled(move |button| {
            if let Some(manager) = weak.upgrade() {
                let active = button.is_active();
                guard("sidebar", || manager.on_sidebar_btn_toggled(active));
            }
        });
        self.sidebar_toggled_handler.replace(Some(handler));
        header_box.append(&self.compact_sidebar_btn);
        search_box.append(&header_box);

        let search_stack = &self.search_stack;
        search_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        search_stack.set_vexpand(true);
        let welcome_page = adw::StatusPage::new();
        welcome_page.set_icon_name(Some("system-search-symbolic"));
        welcome_page.set_title(&tr("Search Packages"));
        welcome_page.set_description(Some(&tr(
            "Enter a package name above to search the repositories.",
        )));
        welcome_page.set_vexpand(true);
        search_stack.add_named(&welcome_page, Some("welcome"));
        let loading_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        loading_box.set_valign(gtk::Align::Center);
        loading_box.set_halign(gtk::Align::Center);
        self.spinner.set_size_request(48, 48);
        self.spinner.start();
        loading_box.append(&self.spinner);
        let loading_lbl = gtk::Label::new(Some(&tr("Searching...")));
        loading_lbl.add_css_class("title-4");
        loading_box.append(&loading_lbl);
        search_stack.add_named(&loading_box, Some("loading"));
        let no_results_page = adw::StatusPage::new();
        no_results_page.set_icon_name(Some("edit-find-symbolic"));
        no_results_page.set_title(&tr("No Results Found"));
        no_results_page.set_description(Some(&tr("Try refining your search terms.")));
        no_results_page.set_vexpand(true);
        search_stack.add_named(&no_results_page, Some("no_results"));

        self.list_detail_stack.set_transition_duration(300);
        self.list_detail_stack.set_vexpand(true);
        let results_scrolled = gtk::ScrolledWindow::new();
        results_scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        results_scrolled.set_vexpand(true);
        results_scrolled.add_css_class("rounded-list");
        results_scrolled.set_overflow(gtk::Overflow::Hidden);
        let weak = Rc::downgrade(self);
        results_scrolled.connect_edge_reached(move |_, position| {
            if position == gtk::PositionType::Bottom
                && let Some(manager) = weak.upgrade()
            {
                guard("more", || manager.load_more_results());
            }
        });
        let selection_model = gtk::NoSelection::new(Some(self.store.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            if let Some(manager) = weak.upgrade()
                && let Some(item) = item.downcast_ref::<gtk::ListItem>()
            {
                guard("row", || manager.setup_list_item(item));
            }
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            if let Some(manager) = weak.upgrade()
                && let Some(item) = item.downcast_ref::<gtk::ListItem>()
            {
                guard("bind", || manager.bind_list_item(item));
            }
        });
        let results_listview = gtk::ListView::new(Some(selection_model), Some(factory));
        results_listview.set_single_click_activate(true);
        let weak = Rc::downgrade(self);
        results_listview.connect_activate(move |_, position| {
            if let Some(manager) = weak.upgrade() {
                guard("activate", || manager.on_listview_item_activate(position));
            }
        });
        results_scrolled.set_child(Some(&results_listview));
        self.list_detail_stack
            .add_named(&results_scrolled, Some("list"));
        self.build_detail_page();
        search_stack.add_named(&self.list_detail_stack, Some("results"));
        search_box.append(search_stack);
        self.on_search_changed();
        self.content_stack
            .add_named(&search_box, Some("search_view"));
    }

    fn setup_pkgbuild_view(self: &Rc<Self>) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
        page.set_margin_start(30);
        page.set_margin_end(30);
        page.set_margin_top(10);
        page.set_margin_bottom(10);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let icon = gtk::Image::from_icon_name("text-x-script");
        icon.set_pixel_size(32);
        header.append(&icon);
        let label = gtk::Label::new(Some(&tr("Review PKGBUILD")));
        label.add_css_class("title-3");
        header.append(&label);
        page.append(&header);
        let desc = gtk::Label::new(Some(&tr(
            "You are about to build a package from the AUR. Please review the build script carefully for malicious code.",
        )));
        desc.set_wrap(true);
        desc.set_halign(gtk::Align::Start);
        page.append(&desc);
        let view = gtk::TextView::with_buffer(&self.pkgbuild_buffer);
        view.set_editable(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::None);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&view));
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(300);
        let frame = gtk::Frame::new(None);
        frame.set_child(Some(&scrolled));
        page.append(&frame);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        actions.set_halign(gtk::Align::End);
        let btn_cancel = gtk::Button::with_label(&tr("Cancel"));
        btn_cancel.add_css_class("buttons_all");
        self.connect_clicked(&btn_cancel, "pkgbuild cancel", |manager| {
            manager.on_pkgbuild_cancel()
        });
        actions.append(&btn_cancel);
        let btn_proceed = gtk::Button::with_label(&tr("Proceed to Build"));
        btn_proceed.add_css_class("suggested-action");
        btn_proceed.add_css_class("buttons_all");
        self.connect_clicked(&btn_proceed, "pkgbuild proceed", |manager| {
            let weak = Rc::downgrade(manager);
            manager.prompt_for_password(Box::new(move || {
                if let Some(manager) = weak.upgrade() {
                    manager.run_aur_build();
                }
            }));
        });
        actions.append(&btn_proceed);
        page.append(&actions);
        self.content_stack.add_named(&page, Some("pkgbuild_view"));
    }

    fn setup_progress_view(self: &Rc<Self>) {
        let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 15);
        progress_box.set_margin_start(30);
        progress_box.set_margin_end(30);
        progress_box.set_valign(gtk::Align::Center);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        self.progress_title.add_css_class("title-2");
        header.append(&self.progress_title);
        progress_box.append(&header);
        self.progress_bar.set_pulse_step(0.1);
        self.progress_bar.set_hexpand(true);
        progress_box.append(&self.progress_bar);
        self.lbl_progress_status.set_halign(gtk::Align::Start);
        self.lbl_progress_status.add_css_class("dim-label");
        progress_box.append(&self.lbl_progress_status);
        self.btn_details.add_css_class("flat");
        self.connect_clicked(&self.btn_details, "details", |manager| {
            manager.on_toggle_details()
        });
        progress_box.append(&self.btn_details);
        self.revealer_details
            .set_transition_type(gtk::RevealerTransitionType::SlideDown);
        let output_textview = gtk::TextView::with_buffer(&self.output_buffer);
        output_textview.set_editable(false);
        output_textview.set_monospace(true);
        output_textview.set_wrap_mode(gtk::WrapMode::WordChar);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&output_textview));
        scrolled.set_min_content_height(200);
        scrolled.set_vexpand(true);
        let frame = gtk::Frame::new(None);
        frame.set_child(Some(&scrolled));
        self.revealer_details.set_child(Some(&frame));
        progress_box.append(&self.revealer_details);
        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        btn_box.set_halign(gtk::Align::End);
        self.btn_cancel.add_css_class("destructive-action");
        self.btn_cancel.add_css_class("buttons_all");
        self.connect_clicked(&self.btn_cancel, "cancel", |manager| {
            manager.on_cancel_clicked()
        });
        btn_box.append(&self.btn_cancel);
        self.btn_back.add_css_class("buttons_all");
        self.connect_clicked(&self.btn_back, "back", |manager| manager.on_back_clicked());
        self.btn_back.set_sensitive(false);
        btn_box.append(&self.btn_back);
        progress_box.append(&btn_box);
        self.content_stack
            .add_named(&progress_box, Some("progress_view"));
    }

    fn on_toggle_details(&self) {
        if self.revealer_details.reveals_child() {
            self.revealer_details.set_reveal_child(false);
            self.btn_details.set_label(&tr("Show Details"));
        } else {
            self.revealer_details.set_reveal_child(true);
            self.btn_details.set_label(&tr("Hide Details"));
        }
    }

    fn on_cancel_clicked(&self) {
        if self.current.running() {
            self.append_log(&format!("\n{}\n", tr("--- Cancelling operation... ---")));
            self.current.interrupt();
        }
    }

    fn setup_info_view(self: &Rc<Self>) {
        let info_box = gtk::Box::new(gtk::Orientation::Vertical, 20);
        info_box.set_valign(gtk::Align::Center);
        info_box.set_halign(gtk::Align::Center);
        self.info_icon.set_pixel_size(64);
        info_box.append(&self.info_icon);
        self.info_text.add_css_class("title-3");
        info_box.append(&self.info_text);
        let btn_view_log = gtk::Button::with_label(&tr("View Log"));
        btn_view_log.add_css_class("flat");
        self.connect_clicked(&btn_view_log, "view log", |manager| {
            manager.on_view_log_clicked()
        });
        info_box.append(&btn_view_log);
        let btn_return = gtk::Button::with_label(&tr("Search Again"));
        btn_return.add_css_class("buttons_all");
        self.connect_clicked(&btn_return, "back", |manager| manager.on_back_clicked());
        info_box.append(&btn_return);
        self.content_stack.add_named(&info_box, Some("info_view"));
    }

    fn on_view_log_clicked(&self) {
        self.content_stack.set_visible_child_name("progress_view");
        self.revealer_details.set_reveal_child(true);
        self.btn_details.set_label(&tr("Hide Details"));
        self.progress_bar.set_visible(false);
        self.lbl_progress_status.set_text(&tr("Transaction Log"));
        self.btn_back.set_sensitive(true);
        self.btn_cancel.set_visible(false);
        let name = self.current_package_name.borrow().clone();
        if self.action_type.get() == Some(Action::Remove) {
            self.progress_title.set_text(&tr_with("Removed {}", &name));
        } else {
            self.progress_title
                .set_text(&tr_with("Installed {}", &name));
        }
    }

    /// The original `setup_appstream_icon_paths`: Flatpak's AppStream icons
    /// join the icon theme.
    fn setup_appstream_icon_paths(&self) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let icon_theme = gtk::IconTheme::for_display(&display);
        let base = std::path::Path::new("/var/lib/flatpak/appstream");
        if !base.exists() {
            return;
        }
        // glob("*/*/active/icons"): directory order, hidden entries skipped.
        let Ok(remotes) = std::fs::read_dir(base) else {
            return;
        };
        let visible =
            |entry: &std::fs::DirEntry| !entry.file_name().as_encoded_bytes().starts_with(b".");
        for remote in remotes.flatten().filter(visible) {
            let Ok(arches) = std::fs::read_dir(remote.path()) else {
                continue;
            };
            for arch in arches.flatten().filter(visible) {
                let root = arch.path().join("active").join("icons");
                if !root.exists() {
                    continue;
                }
                for size in ["64x64", "128x128", "64", "128"] {
                    let icon_dir = root.join(size);
                    if icon_dir.exists() {
                        icon_theme.add_search_path(&icon_dir);
                    }
                }
            }
        }
    }

    /// The original `resolve_icon_name`.
    fn resolve_icon_name(&self, package_name: &str) -> String {
        let Some(display) = gdk::Display::default() else {
            return "package-x-generic".into();
        };
        let icon_theme = gtk::IconTheme::for_display(&display);
        if icon_theme.has_icon(package_name) {
            return package_name.into();
        }
        let mut clean_name = package_name.to_owned();
        for suffix in ["-bin", "-git", "-nightly", "-stable", "-beta"] {
            if let Some(stripped) = package_name.strip_suffix(suffix) {
                clean_name = stripped.to_owned();
                break;
            }
        }
        if clean_name != package_name && icon_theme.has_icon(&clean_name) {
            return clean_name;
        }
        let lower = clean_name.to_lowercase();
        if let Ok(suffixes) = self.flatpak_suffixes.lock()
            && let Some(ids) = suffixes.get(&lower)
        {
            for id in ids {
                if icon_theme.has_icon(id) {
                    return id.clone();
                }
            }
        }
        match package_name {
            "ttf-google-fonts-git" => "preferences-desktop-font",
            "noto-fonts" => "preferences-desktop-font",
            "base-devel" => "applications-engineering",
            "linux" => "system-run",
            "networkmanager" => "network-workgroup",
            "code" => "visual-studio-code",
            "steam-native-runtime" => "steam",
            _ => "package-x-generic",
        }
        .into()
    }

    fn on_search_changed(self: &Rc<Self>) {
        if let Some(timer) = self.search_timer.take() {
            timer.remove();
        }
        if self.list_detail_stack.visible_child_name().as_deref() == Some("detail") {
            self.list_detail_stack
                .set_transition_type(gtk::StackTransitionType::SlideRight);
            self.list_detail_stack.set_visible_child_name("list");
        }
        let query = self.search_entry.text().trim().to_owned();
        let search_id = self.search_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(Duration::from_millis(400), move || {
            if let Some(manager) = weak.upgrade() {
                manager.search_timer.take();
                guard("search", || manager.trigger_search(query, search_id));
            }
        });
        self.search_timer.replace(Some(timer));
    }

    fn trigger_search(self: &Rc<Self>, query: String, search_id: u64) {
        self.search_stack.set_visible_child_name("loading");
        self.spinner.start();
        self.all_search_results.borrow_mut().clear();
        self.displayed_count.set(0);
        self.store.remove_all();
        let aur = self.aur_check.is_active();
        let counter = self.search_counter.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || {
                let stale = || counter.load(Ordering::SeqCst) != search_id;
                pacman::search(&query, aur, &stale)
            })
            .await
            .ok()
            .flatten();
            if let (Some(results), Some(manager)) = (found, weak.upgrade())
                && manager.search_counter.load(Ordering::SeqCst) == search_id
            {
                manager.all_search_results.replace(results);
                guard("results", || manager.update_results_initial());
            }
        });
    }

    fn update_results_initial(&self) {
        self.spinner.stop();
        if self.all_search_results.borrow().is_empty() {
            self.store.remove_all();
            self.search_stack.set_visible_child_name("no_results");
            return;
        }
        self.search_stack.set_visible_child_name("results");
        self.load_more_results();
    }

    fn load_more_results(&self) {
        let results = self.all_search_results.borrow();
        let total = results.len();
        let shown = self.displayed_count.get();
        if shown >= total {
            return;
        }
        let end = (shown + BATCH_SIZE).min(total);
        let items: Vec<glib::BoxedAnyObject> = results[shown..end]
            .iter()
            .map(|package| glib::BoxedAnyObject::new(package.clone()))
            .collect();
        self.store.splice(self.store.n_items(), 0, &items);
        self.displayed_count.set(end);
    }

    // Detail page

    fn build_detail_page(self: &Rc<Self>) {
        let d = &self.detail;
        let detail_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        detail_box.set_vexpand(true);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.set_margin_top(6);
        toolbar.set_margin_start(6);
        toolbar.set_margin_end(6);
        toolbar.set_margin_bottom(6);
        let back_btn = gtk::Button::new();
        back_btn.set_icon_name("go-previous-symbolic");
        back_btn.add_css_class("flat");
        back_btn.add_css_class("circular");
        back_btn.set_tooltip_text(Some(&tr("Back")));
        self.connect_clicked(&back_btn, "detail back", |manager| {
            manager.on_detail_back_clicked()
        });
        toolbar.append(&back_btn);
        d.header_title.add_css_class("title-4");
        d.header_title.set_ellipsize(pango::EllipsizeMode::End);
        d.header_title.set_hexpand(true);
        d.header_title.set_halign(gtk::Align::Start);
        toolbar.append(&d.header_title);
        detail_box.append(&toolbar);
        detail_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let app_header = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        app_header.set_valign(gtk::Align::Start);
        app_header.set_margin_top(24);
        app_header.set_margin_bottom(16);
        app_header.set_margin_start(24);
        app_header.set_margin_end(24);
        d.icon.set_pixel_size(96);
        d.icon.set_valign(gtk::Align::Start);
        app_header.append(&d.icon);

        let meta_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        meta_box.set_valign(gtk::Align::Center);
        meta_box.set_hexpand(true);
        d.name_label.add_css_class("title-1");
        d.name_label.set_halign(gtk::Align::Start);
        d.name_label.set_wrap(true);
        meta_box.append(&d.name_label);
        d.version_label.add_css_class("dim-label");
        d.version_label.set_halign(gtk::Align::Start);
        meta_box.append(&d.version_label);
        d.status_label.set_halign(gtk::Align::Start);
        meta_box.append(&d.status_label);
        d.aur_badge.set_markup(AUR_BADGE);
        d.aur_badge.set_halign(gtk::Align::Start);
        meta_box.append(&d.aur_badge);
        app_header.append(&meta_box);

        let btn_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        btn_box.set_valign(gtk::Align::Center);
        let size_group = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        d.version_btn.set_label(&tr("Change Version"));
        d.version_btn.add_css_class("pill");
        let weak = Rc::downgrade(self);
        d.version_btn.connect_clicked(move |button| {
            if let Some(manager) = weak.upgrade() {
                guard("versions", || {
                    manager.on_detail_change_version_clicked(button)
                });
            }
        });
        size_group.add_widget(&d.version_btn);
        btn_box.append(&d.version_btn);
        let install_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        d.install_older_btn.set_label(&tr("Install Older Version"));
        d.install_older_btn.add_css_class("pill");
        let weak = Rc::downgrade(self);
        d.install_older_btn.connect_clicked(move |button| {
            if let Some(manager) = weak.upgrade() {
                guard("versions", || {
                    manager.on_detail_change_version_clicked(button)
                });
            }
        });
        install_row.append(&d.install_older_btn);
        d.action_btn.add_css_class("pill");
        self.connect_clicked(&d.action_btn, "detail action", |manager| {
            manager.on_detail_action_clicked()
        });
        size_group.add_widget(&d.action_btn);
        install_row.append(&d.action_btn);
        btn_box.append(&install_row);
        app_header.append(&btn_box);
        detail_box.append(&app_header);
        detail_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        d.desc_label.set_wrap(true);
        d.desc_label.set_halign(gtk::Align::Start);
        d.desc_label.add_css_class("body");
        d.desc_label.set_margin_top(12);
        d.desc_label.set_margin_bottom(12);
        d.desc_label.set_margin_start(24);
        d.desc_label.set_margin_end(24);
        detail_box.append(&d.desc_label);
        detail_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        d.wiki_box.set_vexpand(true);
        let wiki_btn = gtk::Button::new();
        wiki_btn.add_css_class("flat");
        wiki_btn.set_margin_top(4);
        wiki_btn.set_margin_bottom(4);
        wiki_btn.set_margin_start(18);
        wiki_btn.set_margin_end(18);
        let wiki_btn_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let wiki_icon = gtk::Image::from_icon_name("help-browser-symbolic");
        wiki_icon.set_pixel_size(16);
        wiki_btn_content.append(&wiki_icon);
        let wiki_title = gtk::Label::new(Some(&tr("Arch Wiki")));
        wiki_title.add_css_class("heading");
        wiki_title.set_hexpand(true);
        wiki_title.set_halign(gtk::Align::Start);
        wiki_btn_content.append(&wiki_title);
        let wiki_open_icon = gtk::Image::from_icon_name("web-browser-symbolic");
        wiki_open_icon.set_pixel_size(14);
        wiki_btn_content.append(&wiki_open_icon);
        d.wiki_spinner.set_size_request(16, 16);
        wiki_btn_content.append(&d.wiki_spinner);
        wiki_btn.set_child(Some(&wiki_btn_content));
        self.connect_clicked(&wiki_btn, "wiki", |manager| manager.on_wiki_open_clicked());
        d.wiki_box.append(&wiki_btn);
        d.wiki_box.append(&d.webview);
        detail_box.append(&d.wiki_box);
        self.list_detail_stack
            .add_named(&detail_box, Some("detail"));
    }

    fn show_package_detail(self: &Rc<Self>, package: Package) {
        let d = &self.detail;
        d.header_title.set_label(&package.name);
        d.icon
            .set_icon_name(Some(&self.resolve_icon_name(&package.name)));
        d.name_label.set_label(&package.name);
        d.version_label
            .set_label(&format!("{}  ·  {}", package.version, package.repo));
        d.aur_badge.set_visible(package.is_aur);
        d.version_btn
            .set_visible(package.installed && !package.is_aur);
        d.install_older_btn
            .set_visible(!package.installed && !package.is_aur);
        if package.installed {
            d.status_label.set_markup(&format!(
                "<span foreground='#33d17a'>● {}</span>",
                tr("Installed")
            ));
            d.action_btn.set_label(&tr("Remove"));
            d.action_btn.remove_css_class("suggested-action");
            d.action_btn.add_css_class("destructive-action");
        } else {
            d.status_label.set_markup(&format!(
                "<span foreground='gray'>○ {}</span>",
                tr("Not installed")
            ));
            d.action_btn.set_label(&tr("Install"));
            d.action_btn.remove_css_class("destructive-action");
            d.action_btn.add_css_class("suggested-action");
        }
        if package.desc.is_empty() {
            d.desc_label.set_label(&tr("No description available."));
        } else {
            d.desc_label.set_label(&package.desc);
        }
        let fetch_id = self.wiki_fetch_id.get() + 1;
        self.wiki_fetch_id.set(fetch_id);
        d.wiki_box.set_visible(true);
        d.wiki_spinner.start();
        d.webview.load_uri("about:blank");
        let name = package.name.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let article = gio::spawn_blocking(move || wiki::fetch(&name))
                .await
                .ok()
                .flatten();
            if let Some(manager) = weak.upgrade() {
                guard("wiki", || manager.on_wiki_fetched(article, fetch_id));
            }
        });
        self.detail_current.replace(Some(package));
        self.list_detail_stack
            .set_transition_type(gtk::StackTransitionType::SlideLeft);
        self.list_detail_stack.set_visible_child_name("detail");
    }

    fn on_wiki_fetched(&self, article: Option<wiki::Article>, fetch_id: u64) {
        if fetch_id != self.wiki_fetch_id.get() {
            return;
        }
        let d = &self.detail;
        d.wiki_spinner.stop();
        let Some(article) = article else {
            self.detail_wiki_url.replace(None);
            d.wiki_box.set_visible(false);
            return;
        };
        self.detail_wiki_url.replace(Some(article.url));
        d.wiki_box.set_visible(true);
        d.webview.load_html(
            &wiki::page(&article.html),
            Some("https://wiki.archlinux.org/"),
        );
    }

    fn on_wiki_open_clicked(&self) {
        if let Some(url) = self.detail_wiki_url.borrow().as_deref() {
            #[allow(deprecated)] // Gtk.show_uri, as the original
            gtk::show_uri(self.parent_window().as_ref(), url, gdk::CURRENT_TIME);
        }
    }

    fn on_detail_back_clicked(&self) {
        self.list_detail_stack
            .set_transition_type(gtk::StackTransitionType::SlideRight);
        self.list_detail_stack.set_visible_child_name("list");
    }

    fn on_detail_action_clicked(self: &Rc<Self>) {
        let Some(package) = self.detail_current.borrow().clone() else {
            return;
        };
        if package.installed {
            self.initiate_remove(&package.name);
        } else {
            self.initiate_install(package);
        }
    }

    fn on_detail_change_version_clicked(self: &Rc<Self>, button: &gtk::Button) {
        let Some(name) = self
            .detail_current
            .borrow()
            .as_ref()
            .map(|package| package.name.clone())
        else {
            return;
        };
        button.set_sensitive(false);
        let (cache, archive) = (tr("cache"), tr("archive"));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let lookup = name.clone();
            let versions = gio::spawn_blocking(move || pacman::versions(&lookup, &cache, &archive))
                .await
                .unwrap_or_default();
            if let Some(manager) = weak.upgrade() {
                guard("versions", || manager.show_version_dialog(&name, versions));
            }
        });
    }

    fn show_version_dialog(self: &Rc<Self>, package: &str, versions: Vec<Version>) {
        let parent = self.parent_window();
        if versions.is_empty() {
            let dialog = adw::MessageDialog::new(
                parent.as_ref(),
                Some(&tr("No Versions Found")),
                Some(&tr_with("No alternative versions found for {}.", package)),
            );
            dialog.add_response("ok", &tr("OK"));
            self.translate_dialog(&dialog);
            dialog.present();
            return;
        }
        let dialog = adw::MessageDialog::new(
            parent.as_ref(),
            Some(&tr("Change Version")),
            Some(&tr_with("Select a version for {}:", package)),
        );
        dialog.add_response("cancel", &tr("Cancel"));
        let listbox = gtk::ListBox::new();
        listbox.set_selection_mode(gtk::SelectionMode::Single);
        listbox.add_css_class("boxed-list");
        for version in &versions {
            let row = adw::ActionRow::new();
            row.set_title(&version.ver);
            row.set_subtitle(&version.source);
            if version.current {
                let badge = gtk::Label::new(Some(&tr("Current")));
                badge.add_css_class("dim-label");
                badge.set_valign(gtk::Align::Center);
                row.add_suffix(&badge);
            }
            listbox.append(&row);
        }
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_min_content_height(150);
        scrolled.set_max_content_height(300);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&listbox));
        dialog.set_extra_child(Some(&scrolled));
        dialog.add_response("install", &tr("Install Selected"));
        dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        let weak = Rc::downgrade(self);
        let package = package.to_owned();
        dialog.connect_response(None, move |dialog, response| {
            if response == "install"
                && let Some(row) = listbox.selected_row()
                && let Some(version) = usize::try_from(row.index())
                    .ok()
                    .and_then(|index| versions.get(index))
                && let Some(manager) = weak.upgrade()
            {
                let target = match &version.url {
                    Some(url) => url.clone(),
                    None => format!("{}/{package}", version.source),
                };
                manager.action_type.set(Some(Action::Install));
                let weak = Rc::downgrade(&manager);
                let package = package.clone();
                guard("version", || {
                    manager.prompt_for_password(Box::new(move || {
                        if let Some(manager) = weak.upgrade() {
                            manager.install_specific_version(&package, &target);
                        }
                    }))
                });
            }
            dialog.close();
        });
        self.translate_dialog(&dialog);
        self.detail.version_btn.set_sensitive(true);
        self.detail.install_older_btn.set_sensitive(true);
        dialog.present();
    }

    /// The first steps of every transaction's progress page.
    fn show_progress(&self, title: Option<&str>, status: &str, cancellable: bool) {
        self.content_stack.set_visible_child_name("progress_view");
        self.output_buffer.set_text("");
        self.btn_back.set_sensitive(false);
        self.progress_bar.set_visible(true);
        self.revealer_details.set_reveal_child(false);
        self.btn_details.set_label(&tr("Show Details"));
        self.btn_cancel.set_visible(cancellable);
        if let Some(title) = title {
            self.progress_title.set_text(title);
        }
        self.lbl_progress_status.set_text(status);
        self.start_pulse();
    }

    fn start_pulse(&self) {
        let bar = self.progress_bar.clone();
        let timer = glib::timeout_add_local(Duration::from_millis(100), move || {
            bar.pulse();
            glib::ControlFlow::Continue
        });
        if let Some(old) = self.pulse_timer.replace(Some(timer)) {
            // The original kept only the newest id; the older timer kept
            // pulsing until the page was gone. One is enough.
            old.remove();
        }
    }

    fn install_specific_version(self: &Rc<Self>, package: &str, target: &str) {
        self.current_package_name.replace(package.to_owned());
        self.show_progress(
            Some(&tr_with("Installing {}...", package)),
            &tr("Changing version..."),
            true,
        );
        let privilege = self.privilege();
        let wrapper = privilege.wrapper();
        if target.starts_with("http") {
            let (url, package) = (target.to_owned(), package.to_owned());
            self.spawn_worker(move |events| {
                download_and_install(&package, &url, privilege, &wrapper, events)
            });
        } else {
            let verb = if target.starts_with('/') { "-U" } else { "-S" };
            let argv = vec![
                wrapper,
                "pacman".into(),
                verb.into(),
                "--noconfirm".into(),
                target.to_owned(),
            ];
            let package = package.to_owned();
            self.spawn_shell(argv, package, None, Vec::new());
        }
    }

    // List rows

    fn on_listview_item_activate(self: &Rc<Self>, position: u32) {
        if let Some(item) = self.store.item(position)
            && let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>()
        {
            let package = boxed.borrow::<Package>().clone();
            self.show_package_detail(package);
        }
    }

    fn setup_list_item(self: &Rc<Self>, item: &gtk::ListItem) {
        let row = adw::ActionRow::new();
        let icon = gtk::Image::new();
        icon.set_pixel_size(32);
        row.add_prefix(&icon);
        let ver_label = gtk::Label::new(None);
        ver_label.add_css_class("dim-label");
        ver_label.set_margin_end(12);
        row.add_suffix(&ver_label);
        let badge = gtk::Label::new(Some("AUR"));
        badge.add_css_class("caption");
        badge.set_markup(AUR_BADGE);
        badge.set_margin_end(8);
        row.add_suffix(&badge);
        let action_btn = gtk::Button::new();
        action_btn.set_valign(gtk::Align::Center);
        row.add_suffix(&action_btn);
        let package: Rc<RefCell<Option<Package>>> = Rc::default();
        let weak = Rc::downgrade(self);
        let current = package.clone();
        action_btn.connect_clicked(move |_| {
            if let (Some(manager), Some(package)) = (weak.upgrade(), current.borrow().clone()) {
                guard("row action", || {
                    if package.installed {
                        manager.initiate_remove(&package.name);
                    } else {
                        manager.initiate_install(package);
                    }
                });
            }
        });
        item.set_child(Some(&row));
        let parts = RowParts {
            row,
            icon,
            ver_label,
            badge,
            action_btn,
            package,
        };
        // SAFETY: only ever read back as RowParts, under this key.
        unsafe { item.set_data("linpama-row", parts) };
    }

    fn bind_list_item(&self, item: &gtk::ListItem) {
        let Some(boxed) = item
            .item()
            .and_then(|object| object.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let package = boxed.borrow::<Package>().clone();
        // SAFETY: set in setup_list_item under this key, with this type.
        let Some(parts) = (unsafe { item.data::<RowParts>("linpama-row") }) else {
            return;
        };
        // SAFETY: the data lives as long as the list item, which outlives this call.
        let parts = unsafe { parts.as_ref() };
        parts.row.set_title(&package.name);
        parts.row.set_subtitle(&package.desc);
        parts
            .icon
            .set_icon_name(Some(&self.resolve_icon_name(&package.name)));
        parts
            .ver_label
            .set_label(&format!("{} ({})", package.version, package.repo));
        parts.badge.set_visible(package.is_aur);
        if package.installed {
            parts.action_btn.set_label(&tr("Remove"));
            parts.action_btn.remove_css_class("suggested-action");
            parts.action_btn.add_css_class("destructive-action");
        } else {
            parts.action_btn.set_label(&tr("Install"));
            parts.action_btn.remove_css_class("destructive-action");
            parts.action_btn.add_css_class("suggested-action");
        }
        parts.package.replace(Some(package));
    }

    // Transactions

    fn initiate_install(self: &Rc<Self>, package: Package) {
        self.action_type.set(Some(Action::Install));
        if package.is_aur {
            self.start_aur_review_process(&package.name);
        } else {
            let weak = Rc::downgrade(self);
            self.prompt_for_password(Box::new(move || {
                if let Some(manager) = weak.upgrade() {
                    manager.run_transaction(&package.name);
                }
            }));
        }
    }

    fn initiate_remove(self: &Rc<Self>, name: &str) {
        self.action_type.set(Some(Action::Remove));
        let weak = Rc::downgrade(self);
        let name = name.to_owned();
        self.prompt_for_password(Box::new(move || {
            if let Some(manager) = weak.upgrade() {
                manager.run_transaction(&name);
            }
        }));
    }

    fn on_refresh_repos_clicked(self: &Rc<Self>) {
        self.action_type.set(Some(Action::Refresh));
        let weak = Rc::downgrade(self);
        self.prompt_for_password(Box::new(move || {
            if let Some(manager) = weak.upgrade() {
                manager.run_repo_update();
            }
        }));
    }

    /// The original `run_repo_update`: `pacman -Sy` with no progress page.
    fn run_repo_update(self: &Rc<Self>) {
        let privilege = self.privilege();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let error = gio::spawn_blocking(move || {
                privilege.start();
                let argv = vec![privilege.wrapper(), "pacman".into(), "-Sy".into()];
                let options = Options {
                    c_locale: true,
                    ..Options::default()
                };
                let error = match runner::run_captured(&argv, options, privilege) {
                    Ok((status, _, _)) if status.success() => None,
                    Ok((_, _, stderr)) => {
                        if stderr.contains("Sorry, try again")
                            || stderr.contains("sudo: no password was provided")
                        {
                            Some(Translatable::IncorrectPassword)
                        } else {
                            println!("Repo update stderr: {stderr}");
                            Some(Translatable::UpdateFailed)
                        }
                    }
                    Err(error) => {
                        println!("Repo update failed: {error}");
                        Some(Translatable::Plain(error))
                    }
                };
                privilege.stop();
                error
            })
            .await
            .unwrap_or(None);
            privilege.forget();
            if let (Some(manager), Some(error)) = (weak.upgrade(), error) {
                let body = match error {
                    Translatable::IncorrectPassword => {
                        tr("Incorrect sudo password. Please try again.")
                    }
                    Translatable::UpdateFailed => tr("Repository update failed"),
                    Translatable::Plain(text) => text,
                };
                manager.message(&tr("Update Failed"), &body);
            }
        });
    }

    fn on_clear_cache_clicked(self: &Rc<Self>) {
        self.action_type.set(Some(Action::ClearCache));
        let weak = Rc::downgrade(self);
        self.prompt_for_password(Box::new(move || {
            if let Some(manager) = weak.upgrade() {
                manager.run_clear_cache();
            }
        }));
    }

    fn run_clear_cache(self: &Rc<Self>) {
        let label = tr("cache");
        self.current_package_name.replace(label.clone());
        self.show_progress(
            Some(&tr("Clearing Package Cache...")),
            &tr("Removing cached packages..."),
            true,
        );
        let privilege = self.privilege();
        let commands = vec![vec![
            privilege.wrapper(),
            "pacman".into(),
            "-Scc".into(),
            "--noconfirm".into(),
        ]];
        let mut user_commands = Vec::new();
        if pacman::which("paru") {
            user_commands.push(vec!["paru".into(), "-Scc".into(), "--noconfirm".into()]);
        }
        let current = self.current.clone();
        self.spawn_worker(move |events| {
            run_sequential(&commands, &user_commands, privilege, &current, events);
            label
        });
    }

    fn on_remove_orphans_clicked(self: &Rc<Self>) {
        self.action_type.set(Some(Action::RemoveOrphans));
        let weak = Rc::downgrade(self);
        self.prompt_for_password(Box::new(move || {
            if let Some(manager) = weak.upgrade() {
                let weak = Rc::downgrade(&manager);
                glib::spawn_future_local(async move {
                    let orphans = gio::spawn_blocking(pacman::orphans)
                        .await
                        .unwrap_or_default();
                    if let Some(manager) = weak.upgrade() {
                        guard("orphans", || manager.show_orphans_confirmation(&orphans));
                    }
                });
            }
        }));
    }

    fn show_orphans_confirmation(self: &Rc<Self>, orphans: &str) {
        let parent = self.parent_window();
        if orphans.is_empty() {
            let dialog = adw::MessageDialog::new(
                parent.as_ref(),
                Some(&tr("No Orphans Found")),
                Some(&tr("There are no orphan packages to remove.")),
            );
            dialog.add_response("ok", &tr("OK"));
            self.translate_dialog(&dialog);
            dialog.present();
            return;
        }
        let orphan_list: Vec<String> = orphans.split('\n').map(str::to_owned).collect();
        let dialog = adw::MessageDialog::new(
            parent.as_ref(),
            Some(&tr("Remove Orphan Packages")),
            Some(&tr_with(
                "The following {} packages will be removed:",
                &orphan_list.len().to_string(),
            )),
        );
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("remove", &tr("Remove All"));
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_min_content_height(150);
        scrolled.set_max_content_height(300);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list_box.set_margin_start(8);
        list_box.set_margin_end(8);
        for package in &orphan_list {
            let label = gtk::Label::new(Some(package));
            label.set_halign(gtk::Align::Start);
            label.add_css_class("monospace");
            list_box.append(&label);
        }
        scrolled.set_child(Some(&list_box));
        dialog.set_extra_child(Some(&scrolled));
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |dialog, response| {
            if response == "remove"
                && let Some(manager) = weak.upgrade()
            {
                guard("orphan removal", || {
                    manager.start_orphan_removal(orphan_list.clone())
                });
            }
            dialog.close();
        });
        self.translate_dialog(&dialog);
        dialog.present();
    }

    fn start_orphan_removal(self: &Rc<Self>, orphans: Vec<String>) {
        let label = tr("orphans");
        self.current_package_name.replace(label.clone());
        self.show_progress(
            Some(&tr("Removing Orphan Packages...")),
            &tr("Removing orphans..."),
            true,
        );
        let privilege = self.privilege();
        let current = self.current.clone();
        self.spawn_worker(move |events| {
            privilege.start();
            let mut argv = vec![
                privilege.wrapper(),
                "pacman".into(),
                "-Rns".into(),
                "--noconfirm".into(),
            ];
            argv.extend(orphans);
            let success = run_and_log(
                &argv,
                Options {
                    c_locale: true,
                    ..Options::default()
                },
                privilege,
                &current,
                events,
            );
            let _ = events.unbounded_send(Event::Finished(success, label.clone()));
            label
        });
    }

    fn on_remove_db_lock_clicked(self: &Rc<Self>) {
        self.action_type.set(Some(Action::RemoveDbLock));
        let weak = Rc::downgrade(self);
        self.prompt_for_password(Box::new(move || {
            if let Some(manager) = weak.upgrade() {
                manager.run_remove_db_lock();
            }
        }));
    }

    fn run_remove_db_lock(self: &Rc<Self>) {
        let label = tr("db lock");
        self.current_package_name.replace(label.clone());
        self.show_progress(
            Some(&tr("Removing Database Lock...")),
            &tr("Removing /var/lib/pacman/db.lck..."),
            false,
        );
        let privilege = self.privilege();
        let (nothing, removed) = (
            tr("No database lock file found. Nothing to do.\n"),
            tr("Database lock file removed successfully.\n"),
        );
        self.spawn_worker(move |events| {
            privilege.start();
            let success = if !std::path::Path::new(DB_LOCK).exists() {
                let _ = events.unbounded_send(Event::Log(nothing));
                true
            } else {
                let argv = vec![
                    privilege.wrapper(),
                    "rm".into(),
                    "-f".into(),
                    DB_LOCK.into(),
                ];
                let options = Options {
                    c_locale: true,
                    ..Options::default()
                };
                match runner::run_captured(&argv, options, privilege) {
                    Ok((status, _, _)) if status.success() => {
                        let _ = events.unbounded_send(Event::Log(removed));
                        true
                    }
                    Ok((_, _, stderr)) => {
                        let _ = events.unbounded_send(Event::Log(stderr));
                        false
                    }
                    Err(error) => {
                        let _ = events.unbounded_send(Event::Log(format!("\nError: {error}")));
                        false
                    }
                }
            };
            let _ = events.unbounded_send(Event::Finished(success, label.clone()));
            label
        });
    }

    // AUR

    fn start_aur_review_process(self: &Rc<Self>, package: &str) {
        let temp = match make_temp_dir(&format!("{APP_NAME}_aur_")) {
            Ok(temp) => temp,
            Err(error) => {
                eprintln!("Could not create a directory for {package}: {error}");
                return;
            }
        };
        self.aur_temp_dir.replace(Some(temp.clone()));
        self.aur_pkg_name.replace(package.to_owned());
        self.current_package_name.replace(package.to_owned());
        self.content_stack.set_visible_child_name("progress_view");
        self.output_buffer
            .set_text(&tr_with("Fetching {} from AUR...", package));
        self.btn_back.set_sensitive(false);
        self.btn_cancel.set_visible(true);
        self.progress_bar.pulse();
        self.lbl_progress_status
            .set_text(&tr("Cloning repository..."));
        let current = self.current.clone();
        let package = package.to_owned();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let outcome = gio::spawn_blocking(move || clone_aur(&package, &temp, &current))
                .await
                .unwrap_or_else(|_| Err("Error: the clone stopped".into()));
            if let Some(manager) = weak.upgrade() {
                match outcome {
                    Ok(content) => {
                        manager.pkgbuild_buffer.set_text(&content);
                        manager
                            .content_stack
                            .set_visible_child_name("pkgbuild_view");
                    }
                    Err(message) => {
                        manager.output_buffer.set_text(&message);
                        manager.btn_back.set_sensitive(true);
                        manager.btn_cancel.set_visible(false);
                    }
                }
            }
        });
    }

    fn on_pkgbuild_cancel(&self) {
        if let Some(temp) = self.aur_temp_dir.borrow().as_ref()
            && temp.exists()
        {
            let _ = std::fs::remove_dir_all(temp);
        }
        self.content_stack.set_visible_child_name("search_view");
    }

    fn run_aur_build(self: &Rc<Self>) {
        let package = self.aur_pkg_name.borrow().clone();
        self.content_stack.set_visible_child_name("progress_view");
        self.output_buffer.set_text("");
        self.progress_title
            .set_text(&tr_with("Building {}...", &package));
        self.progress_bar.set_visible(true);
        self.revealer_details.set_reveal_child(false);
        self.btn_details.set_label(&tr("Show Details"));
        self.lbl_progress_status
            .set_text(&tr("Compiling package..."));
        self.btn_cancel.set_visible(true);
        self.start_pulse();
        self.current_package_name.replace(package.clone());
        let (argv, cwd) = if pacman::which("paru") {
            (
                vec![
                    "paru".into(),
                    "-S".into(),
                    "--noconfirm".into(),
                    package.clone(),
                ],
                None,
            )
        } else {
            (
                vec!["makepkg".into(), "-si".into(), "--noconfirm".into()],
                self.aur_temp_dir.borrow().clone(),
            )
        };
        let privilege = self.privilege();
        let mut env = vec![("PACMAN_AUTH".to_owned(), privilege.wrapper())];
        if let Some(askpass) = privilege.askpass() {
            env.push(("SUDO_ASKPASS".to_owned(), askpass));
        }
        self.spawn_shell(argv, package, cwd, env);
    }

    // Password prompt

    /// The original `prompt_for_password`, its wording following
    /// `self.action_type`.
    fn prompt_for_password(self: &Rc<Self>, success: Box<dyn Fn()>) {
        let action = self.action_type.get().unwrap_or(Action::Install);
        let action_label = if action == Action::Install {
            tr("install")
        } else {
            tr("remove")
        };
        let body = match action {
            Action::Refresh => tr("Please enter your password to update repositories."),
            Action::ClearCache => tr("Please enter your password to clear the package cache."),
            Action::RemoveOrphans => tr("Please enter your password to remove orphan packages."),
            Action::RemoveDbLock => {
                tr("Please enter your password to remove the database lock file.")
            }
            Action::Install | Action::Remove => tr_with(
                "Please enter your password to {} this package.",
                &action_label,
            ),
        };
        let dialog = adw::MessageDialog::new(
            self.parent_window().as_ref(),
            Some(&tr("Authentication Required")),
            Some(&body),
        );
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("unlock", &tr("Unlock"));
        dialog.set_response_appearance("unlock", adw::ResponseAppearance::Suggested);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let entry = gtk::PasswordEntry::new();
        entry.set_property("placeholder-text", tr("Password"));
        content.append(&entry);
        dialog.set_extra_child(Some(&content));
        let weak = Rc::downgrade(self);
        let password = entry.clone();
        let success: Rc<dyn Fn()> = Rc::from(success);
        dialog.connect_response(None, move |dialog, response| {
            if response == "unlock"
                && let Some(manager) = weak.upgrade()
            {
                let typed = password.text().to_string();
                if !typed.is_empty() {
                    guard("unlock", || manager.check_password(typed, success.clone()));
                }
            }
            dialog.close();
        });
        let dialog_weak = dialog.downgrade();
        entry.connect_activate(move |_| {
            if let Some(dialog) = dialog_weak.upgrade() {
                dialog.response("unlock");
            }
        });
        self.translate_dialog(&dialog);
        dialog.present();
    }

    /// Validate off the GTK thread (the original blocked it), then go on as
    /// the original's response handler did.
    fn check_password(self: &Rc<Self>, password: String, success: Rc<dyn Fn()>) {
        let Some(host) = self.host else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let checked = password.clone();
            let valid = gio::spawn_blocking(move || host.sudo_validate_password(&checked))
                .await
                .unwrap_or(false);
            let Some(manager) = weak.upgrade() else {
                return;
            };
            if valid {
                host.sudo_set_password(&password);
                guard("authenticated", || success());
            } else {
                manager.message(&tr("Authentication Failed"), &tr("Incorrect password."));
            }
        });
    }

    fn run_transaction(self: &Rc<Self>, package: &str) {
        self.current_package_name.replace(package.to_owned());
        let wrapper = self.privilege().wrapper();
        let argv: Vec<String> = if self.action_type.get() == Some(Action::Remove) {
            self.show_progress(
                Some(&tr_with("Removing {}...", package)),
                &tr("Removing package..."),
                true,
            );
            vec![
                wrapper,
                "pacman".into(),
                "-Rns".into(),
                "--noconfirm".into(),
                package.to_owned(),
            ]
        } else {
            self.show_progress(
                Some(&tr_with("Installing {}...", package)),
                &tr("Downloading and installing..."),
                true,
            );
            vec![
                wrapper,
                "pacman".into(),
                "-S".into(),
                "--noconfirm".into(),
                "--needed".into(),
                package.to_owned(),
            ]
        };
        self.spawn_shell(argv, package.to_owned(), None, Vec::new());
    }

    /// The original `execute_shell` on a worker thread.
    fn spawn_shell(
        self: &Rc<Self>,
        argv: Vec<String>,
        package: String,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
    ) {
        let privilege = self.privilege();
        let current = self.current.clone();
        self.spawn_worker(move |events| {
            execute_shell(
                &argv,
                &package,
                cwd.as_deref(),
                env,
                privilege,
                &current,
                events,
            );
            package
        });
    }

    /// Run `work` on a worker thread, applying what it reports on the GTK
    /// thread as the original's `GLib.idle_add` calls did.
    fn spawn_worker(
        self: &Rc<Self>,
        work: impl FnOnce(&futures_channel::mpsc::UnboundedSender<Event>) -> String + Send + 'static,
    ) {
        let (sender, mut events) = futures_channel::mpsc::unbounded::<Event>();
        let worker = std::thread::Builder::new()
            .name("linpama".into())
            .spawn(move || {
                if catch_unwind(AssertUnwindSafe(|| work(&sender))).is_err() {
                    let _ = sender.unbounded_send(Event::Finished(false, String::new()));
                }
            });
        if let Err(error) = worker {
            eprintln!("Package Manager: could not start a worker: {error}");
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(event) = events.next().await {
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                match event {
                    Event::Log(text) => manager.append_log(&text),
                    Event::Status(text) => manager.lbl_progress_status.set_text(&text),
                    Event::Finished(success, name) => {
                        guard("finished", || manager.on_process_finished(success, &name))
                    }
                    Event::DownloadFailed(_package, error) => manager.download_failed(&error),
                }
            }
        });
    }

    fn download_failed(&self, error: &str) {
        self.btn_back.set_sensitive(true);
        self.btn_cancel.set_visible(false);
        self.progress_bar.set_visible(false);
        if let Some(timer) = self.pulse_timer.take() {
            timer.remove();
        }
        self.progress_title.set_text(&tr("Download Failed"));
        self.lbl_progress_status.set_text(error);
    }

    fn append_log(&self, text: &str) {
        let mut end = self.output_buffer.end_iter();
        self.output_buffer.insert(&mut end, text);
    }

    fn on_process_finished(&self, success: bool, package: &str) {
        let privilege = self.privilege();
        privilege.stop();
        privilege.forget();
        if let Some(timer) = self.pulse_timer.take() {
            timer.remove();
        }
        if let Some(temp) = self.aur_temp_dir.borrow().as_ref()
            && temp.exists()
        {
            let _ = std::fs::remove_dir_all(temp);
        }
        if success {
            self.info_icon.set_icon_name(Some("object-select-symbolic"));
            let text = match self.action_type.get() {
                Some(Action::Remove) => tr_with("Successfully removed {}", package),
                Some(Action::ClearCache) => tr("Package cache cleared successfully."),
                Some(Action::RemoveOrphans) => tr("Orphan packages removed successfully."),
                Some(Action::RemoveDbLock) => tr("Database lock removed successfully."),
                _ => tr_with("Successfully installed {}", package),
            };
            self.info_text.set_text(&text);
            self.content_stack.set_visible_child_name("info_view");
            self.search_entry.set_text("");
        } else {
            self.revealer_details.set_reveal_child(true);
            self.btn_details.set_label(&tr("Hide Details"));
            let (start, end) = self.output_buffer.bounds();
            let log = self.output_buffer.text(&start, &end, true).to_string();
            let lower = log.to_lowercase();
            let status = if log.contains("Sorry, try again")
                || lower.contains("incorrect password")
                || lower.contains("sudo: no password was provided")
            {
                tr("Incorrect sudo password. Please try again.")
            } else {
                tr("Failed or Cancelled.")
            };
            self.lbl_progress_status.set_text(&status);
            self.btn_back.set_sensitive(true);
            self.btn_cancel.set_visible(false);
            self.append_log(&format!("\n\n{}", tr("Transaction Failed.")));
        }
    }

    fn on_back_clicked(&self) {
        self.content_stack.set_visible_child_name("search_view");
    }

    // Layout

    fn on_sidebar_btn_toggled(&self, active: bool) {
        if self.wide_layout_enabled.get() == Some(true) {
            self.right_revealer.set_reveal_child(active);
        } else {
            self.compact_panel_revealer.set_reveal_child(active);
        }
    }

    /// One group of the Actions pane (the wide one, or the compact overlay's).
    fn action_rows(self: &Rc<Self>) -> (adw::PreferencesGroup, adw::PreferencesGroup) {
        let top = adw::PreferencesGroup::new();
        top.set_title(&tr("Actions"));
        let refresh = action_row(
            &tr("Refresh Repositories"),
            &tr("Sync package databases"),
            "view-refresh-symbolic",
        );
        self.connect_activated(&refresh, "refresh", |manager| {
            manager.on_refresh_repos_clicked()
        });
        top.add(&refresh);
        let bottom = adw::PreferencesGroup::new();
        let cache = action_row(
            &tr("Clear Package Cache"),
            &tr("Free disk space from cached packages"),
            "user-trash-symbolic",
        );
        self.connect_activated(&cache, "cache", |manager| manager.on_clear_cache_clicked());
        bottom.add(&cache);
        let orphans = action_row(
            &tr("Remove Orphans"),
            &tr("Remove unused dependency packages"),
            "edit-clear-all-symbolic",
        );
        self.connect_activated(&orphans, "orphans", |manager| {
            manager.on_remove_orphans_clicked()
        });
        bottom.add(&orphans);
        let db_lock = action_row(
            &tr("Remove DB Lock"),
            &tr("Delete /var/lib/pacman/db.lck"),
            "channel-insecure-symbolic",
        );
        self.connect_activated(&db_lock, "db lock", |manager| {
            manager.on_remove_db_lock_clicked()
        });
        bottom.add(&db_lock);
        (top, bottom)
    }

    fn setup_right_pane(self: &Rc<Self>) {
        let right_pane = gtk::Box::new(gtk::Orientation::Vertical, 12);
        right_pane.set_hexpand(false);
        right_pane.set_vexpand(true);
        right_pane.set_valign(gtk::Align::Fill);
        right_pane.set_margin_end(16);
        let (top, bottom) = self.action_rows();
        right_pane.append(&top);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_vexpand(true);
        right_pane.append(&spacer);
        right_pane.append(&bottom);

        let compact_panel_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        compact_panel_box.set_hexpand(false);
        compact_panel_box.set_vexpand(true);
        compact_panel_box.set_valign(gtk::Align::Fill);
        compact_panel_box.set_halign(gtk::Align::End);
        compact_panel_box.set_size_request(RIGHT_PANE_MIN_WIDTH, -1);
        compact_panel_box.add_css_class("background");
        let compact_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        compact_header.set_halign(gtk::Align::End);
        compact_header.set_margin_top(4);
        compact_header.set_margin_end(4);
        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");
        close_btn.set_tooltip_text(Some(&tr("Close")));
        self.connect_clicked(&close_btn, "close panel", |manager| {
            manager.compact_sidebar_btn.set_active(false)
        });
        compact_header.append(&close_btn);
        compact_panel_box.append(&compact_header);
        let compact_center = gtk::Box::new(gtk::Orientation::Vertical, 12);
        compact_center.set_valign(gtk::Align::Fill);
        compact_center.set_halign(gtk::Align::Center);
        compact_center.set_vexpand(true);
        compact_center.set_margin_start(WIDE_LAYOUT_SIDE_PADDING);
        compact_center.set_margin_end(WIDE_LAYOUT_SIDE_PADDING);
        let (compact_top, compact_bottom) = self.action_rows();
        compact_center.append(&compact_top);
        let compact_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        compact_spacer.set_vexpand(true);
        compact_center.append(&compact_spacer);
        compact_center.append(&compact_bottom);
        compact_panel_box.append(&compact_center);

        let revealer = &self.compact_panel_revealer;
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
        revealer.set_transition_duration(250);
        revealer.set_reveal_child(false);
        revealer.set_halign(gtk::Align::End);
        revealer.set_vexpand(true);
        revealer.set_child(Some(&compact_panel_box));
        let compact_overlay = gtk::Overlay::new();
        compact_overlay.set_hexpand(true);
        compact_overlay.set_vexpand(true);
        compact_overlay.add_overlay(revealer);

        self.right_revealer.set_child(Some(&right_pane));
        self.content_hbox.append(&self.content_stack);
        self.content_hbox.append(&self.right_revealer);
        compact_overlay.set_child(Some(&self.content_hbox));
        let main_layout_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        main_layout_box.set_hexpand(true);
        main_layout_box.set_vexpand(true);
        main_layout_box.append(&compact_overlay);
        self.top_stack
            .add_named(&main_layout_box, Some("main_view"));
    }

    fn width(&self) -> i32 {
        let width = self.root.upgrade().map_or(0, |root| root.width());
        if width <= 0
            && let Some(window) = self.window.as_ref().and_then(|window| window.upgrade())
        {
            return window.width();
        }
        width
    }

    fn monitor_adaptive_layout(&self) {
        let width = self.width();
        if width > 0 && width != self.last_measured_width.get() {
            self.last_measured_width.set(width);
            self.update_adaptive_layout(false, Some(width));
        }
    }

    fn update_adaptive_layout(&self, force: bool, width: Option<i32>) {
        let width = width.unwrap_or_else(|| self.width());
        let wide = width > WIDE_LAYOUT_THRESHOLD;
        if !force && Some(wide) == self.wide_layout_enabled.get() {
            return;
        }
        self.wide_layout_enabled.set(Some(wide));
        let Some(root) = self.root.upgrade() else {
            return;
        };
        let handler = self.sidebar_toggled_handler.borrow();
        if wide {
            root.set_margin_start(0);
            root.set_margin_end(0);
            self.content_stack.set_margin_start(12);
            self.content_stack.set_margin_end(0);
            self.compact_panel_revealer.set_reveal_child(false);
            if let Some(handler) = handler.as_ref() {
                self.compact_sidebar_btn.block_signal(handler);
                self.compact_sidebar_btn.set_active(true);
                self.compact_sidebar_btn.unblock_signal(handler);
            }
            self.right_revealer.set_reveal_child(true);
        } else {
            root.set_margin_start(12);
            root.set_margin_end(12);
            self.content_stack.set_margin_start(0);
            self.content_stack.set_margin_end(0);
            self.right_revealer.set_reveal_child(false);
            self.compact_panel_revealer.set_reveal_child(false);
            if let Some(handler) = handler.as_ref() {
                self.compact_sidebar_btn.block_signal(handler);
                self.compact_sidebar_btn.set_active(false);
                self.compact_sidebar_btn.unblock_signal(handler);
            }
        }
    }
}

/// What the repository update's dialog says, translated on the GTK thread.
enum Translatable {
    IncorrectPassword,
    UpdateFailed,
    Plain(String),
}

fn action_row(title: &str, subtitle: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.set_activatable(true);
    row
}

/// `tempfile.mkdtemp(prefix=...)`.
pub fn make_temp_dir(prefix: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let base = ["TMPDIR", "TEMP", "TMP"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut bytes = base
        .join(format!("{prefix}XXXXXX"))
        .as_os_str()
        .as_bytes()
        .to_vec();
    bytes.push(0);
    // SAFETY: a writable NUL-terminated template that mkdtemp fills in.
    if unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) }.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    bytes.pop();
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

/// The original `start_aur_review_process`'s clone: the PKGBUILD, or the
/// message shown instead. Blocking.
pub fn clone_aur(
    package: &str,
    temp: &std::path::Path,
    current: &Current,
) -> Result<String, String> {
    let argv = vec![
        "git".to_owned(),
        "clone".into(),
        format!("https://aur.archlinux.org/{package}.git"),
        temp.to_string_lossy().into_owned(),
    ];
    let status = runner::run_logged(
        &argv,
        Options::default(),
        Privilege::Polkit,
        current,
        &|_| {},
    )
    .map_err(|error| format!("Error: {error}"))?;
    if !status.success() {
        return Err("Clone failed or cancelled.".into());
    }
    let pkgbuild = temp.join("PKGBUILD");
    if !pkgbuild.exists() {
        return Err("PKGBUILD not found in cloned repository.".into());
    }
    std::fs::read_to_string(pkgbuild).map_err(|error| format!("Error: {error}"))
}

/// Run one command with its lines logged; whether it exited 0.
fn run_and_log(
    argv: &[String],
    options: Options,
    privilege: Privilege,
    current: &Current,
    events: &futures_channel::mpsc::UnboundedSender<Event>,
) -> bool {
    let log = |line: String| {
        let _ = events.unbounded_send(Event::Log(line));
    };
    match runner::run_logged(argv, options, privilege, current, &log) {
        Ok(status) => status.success(),
        Err(error) => {
            log(format!("\nError: {error}"));
            false
        }
    }
}

/// The original `execute_shell`.
fn execute_shell(
    argv: &[String],
    package: &str,
    cwd: Option<&std::path::Path>,
    env: Vec<(String, String)>,
    privilege: Privilege,
    current: &Current,
    events: &futures_channel::mpsc::UnboundedSender<Event>,
) {
    privilege.start();
    let options = Options {
        c_locale: true,
        env,
        cwd,
    };
    let success = run_and_log(argv, options, privilege, current, events);
    let _ = events.unbounded_send(Event::Finished(success, package.to_owned()));
}

/// The original `_run_sequential_cmds`: privileged commands, then the
/// user's own; success only if every one exited 0.
fn run_sequential(
    commands: &[Vec<String>],
    user_commands: &[Vec<String>],
    privilege: Privilege,
    current: &Current,
    events: &futures_channel::mpsc::UnboundedSender<Event>,
) {
    let mut success = true;
    privilege.start();
    for argv in commands {
        let options = Options {
            c_locale: true,
            ..Options::default()
        };
        if !run_and_log(argv, options, privilege, current, events) {
            success = false;
        }
    }
    for argv in user_commands {
        let options = Options {
            c_locale: true,
            ..Options::default()
        };
        if !run_and_log(argv, options, privilege, current, events) {
            success = false;
        }
    }
    let _ = events.unbounded_send(Event::Finished(success, tr("cache")));
}

/// The original `_download_and_install`: the archive's package fetched into
/// a temporary directory and installed from there.
fn download_and_install(
    package: &str,
    url: &str,
    privilege: Privilege,
    wrapper: &str,
    events: &futures_channel::mpsc::UnboundedSender<Event>,
) -> String {
    let temp = match make_temp_dir("linpama_") {
        Ok(temp) => temp,
        Err(error) => {
            let _ =
                events.unbounded_send(Event::DownloadFailed(package.to_owned(), error.to_string()));
            return package.to_owned();
        }
    };
    let filename = crate::http::unquote(url.rsplit('/').next().unwrap_or(url));
    let local = temp.join(&filename);
    let _ = events.unbounded_send(Event::Status(tr("Downloading package...")));
    if let Err(error) = crate::http::download(url, &local) {
        let _ = std::fs::remove_dir_all(&temp);
        let _ = events.unbounded_send(Event::DownloadFailed(package.to_owned(), error));
        return package.to_owned();
    }
    let argv = vec![
        wrapper.to_owned(),
        "pacman".into(),
        "-U".into(),
        "--noconfirm".into(),
        local.to_string_lossy().into_owned(),
    ];
    let current = Current::default();
    execute_shell(
        &argv,
        package,
        None,
        Vec::new(),
        privilege,
        &current,
        events,
    );
    let _ = std::fs::remove_dir_all(&temp);
    package.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_written_like_json_dump() {
        let mut map = serde_json::Map::new();
        map.insert("show_warning".into(), Value::Bool(false));
        let name: String = ['\u{141}', '\u{f3}', 'd', '\u{17a}', ' ', '"', 'x', '"']
            .iter()
            .collect();
        map.insert("name".into(), Value::String(name));
        let slash = '\\';
        let expected = format!(
            "{{\"show_warning\": false, \"name\": \"{slash}u0141{slash}u00f3d{slash}u017a {slash}\"x{slash}\"\"}}"
        );
        assert_eq!(python_json(&Value::Object(map)), expected);
        assert!(python_truthy(&Value::Bool(true)));
        assert!(!python_truthy(&serde_json::json!(0)));
    }
}
