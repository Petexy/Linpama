//! Development harness: loads the built widget library through the C ABI with a
//! host whose translation services are the Center's own (dictionaries, `{}`
//! patterns and `translate_dialog` from linexin-localization), shows it in an
//! `AdwApplicationWindow` and plays the steps given on the command line, as
//! `examples/python_reference.py` does with the original widget:
//!
//! ```text
//! type=TEXT        the search entry's text
//! check=LABEL      toggle the check button labelled LABEL
//! click=LABEL      click the (first visible) button labelled LABEL
//! row=NAME         open the result row titled NAME
//! row-action=NAME  click the Install/Remove button of that row
//! select=TITLE     select the row titled TITLE in the open dialog's list
//! activate=TITLE   activate the (first visible) action row titled TITLE
//! sidebar          toggle the actions button beside the search entry
//! answer=RESPONSE  answer the open dialog (RESPONSE=PASSWORD types one)
//! wait=PAGE        wait until the content page (search_view, progress_view,
//!                  info_view, pkgbuild_view) is PAGE
//! wait-results     wait until a search has finished
//! wait-done        wait until the Back button of the progress page works
//! settle           wait 1.5 s
//! print            print what the widget shows
//! shot=PATH        render the window
//! dialog-shot=PATH render the open dialog
//! files            print the config file
//! ```
//!
//! Run it inside a headless compositor, with the fake commands of
//! examples/compare.sh first on `PATH`. The host accepts only the password in
//! `$FAKE_PASSWORD`, and its sudo wrapper is `$FAKE_WRAPPER`.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib::translate::{FromGlibPtrBorrow, FromGlibPtrFull};
use gtk::{gio, glib, graphene};
use linexin_localization::{Localization, widgets};
use linexin_plugin_api::{
    ABI_VERSION, CreateContextV1, ENTRY_SYMBOL, EntryV1, Header, HostApiV1, WidgetDescriptorV1,
};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const SETTLE: Duration = Duration::from_millis(1500);
const PATIENCE: Duration = Duration::from_secs(30);

struct Options {
    library: PathBuf,
    localization: PathBuf,
    /// The Center's stylesheet; its own copy (`CENTER_STYLE_CSS`) without one.
    css: Option<PathBuf>,
    language: CString,
    dark: bool,
    width: i32,
    height: i32,
    steps: Vec<String>,
}

static OPTIONS: OnceLock<Options> = OnceLock::new();

fn options() -> &'static Options {
    OPTIONS
        .get()
        .expect("options are set before the host is used")
}

fn parse_options() -> Result<Options, String> {
    let manifest = Path::new(MANIFEST_DIR);
    let mut options = Options {
        library: beside_the_examples("libpackage_manager.so"),
        localization: manifest.join("data/usr/share/linexin/widgets/localization"),
        css: None,
        language: c"en_US".into(),
        dark: false,
        width: 1300,
        height: 820,
        steps: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--library" => options.library = value()?.into(),
            "--localization" => options.localization = value()?.into(),
            "--css" => options.css = Some(value()?.into()),
            "--lang" => options.language = CString::new(value()?).map_err(|e| e.to_string())?,
            "--dark" => options.dark = true,
            "--width" => options.width = value()?.parse().map_err(|_| "bad --width")?,
            "--height" => options.height = value()?.parse().map_err(|_| "bad --height")?,
            "--widget" => {
                value()?;
            }
            step if !step.starts_with("--") => options.steps.push(step.to_owned()),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(options)
}

fn install_localization() {
    let options = options();
    let localization = Localization::load(
        std::slice::from_ref(&options.localization),
        &options.language.to_string_lossy(),
        None,
    );
    for diagnostic in localization.diagnostics() {
        eprintln!("localization: {diagnostic}");
    }
    linexin_localization::install(localization);
}

/// A line in `$FAKE_LOG` (or `$FAKE_LOCK_LOG`), beside the fake commands'.
fn log(variable: &str, line: &str) {
    if let Some(path) = std::env::var_os(variable)
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

unsafe extern "C" fn language() -> *const c_char {
    options().language.as_ptr()
}

unsafe extern "C" fn translate(domain: *const c_char, text: *const c_char) -> *mut c_char {
    if domain.is_null() || text.is_null() {
        return std::ptr::null_mut();
    }
    let (domain, text) = unsafe { (CStr::from_ptr(domain), CStr::from_ptr(text)) };
    let translated = linexin_localization::global()
        .translate(&text.to_string_lossy(), &domain.to_string_lossy());
    CString::new(translated).map_or(std::ptr::null_mut(), CString::into_raw)
}

unsafe extern "C" fn free_string(string: *mut c_char) {
    if !string.is_null() {
        drop(unsafe { CString::from_raw(string) });
    }
}

unsafe extern "C" fn translate_widget(domain: *const c_char, widget: *mut c_void) {
    if domain.is_null() || widget.is_null() {
        return;
    }
    let domain = unsafe { CStr::from_ptr(domain) }.to_string_lossy();
    let widget = unsafe { gtk::Widget::from_glib_borrow(widget.cast::<gtk::ffi::GtkWidget>()) };
    widgets::translate_dialog(&widget, &domain);
}

unsafe extern "C" fn no_op() {}
unsafe extern "C" fn lock_ui() {
    log("FAKE_LOCK_LOG", "lock");
}
unsafe extern "C" fn unlock_ui() {
    log("FAKE_LOCK_LOG", "unlock");
}
unsafe extern "C" fn validate_password(password: *const c_char) -> bool {
    log("FAKE_LOG", "sudo validate_password");
    let expected = std::env::var("FAKE_PASSWORD").unwrap_or_default();
    !password.is_null()
        && !expected.is_empty()
        && unsafe { CStr::from_ptr(password) }.to_bytes() == expected.as_bytes()
}
unsafe extern "C" fn set_password(_: *const c_char) {
    log("FAKE_LOG", "sudo set_password");
}
unsafe extern "C" fn no_password() -> bool {
    false
}
unsafe extern "C" fn forget_password() {
    log("FAKE_LOG", "sudo forget_password");
}
unsafe extern "C" fn wrapper_path() -> *const c_char {
    static WRAPPER: OnceLock<CString> = OnceLock::new();
    WRAPPER
        .get_or_init(|| {
            CString::new(std::env::var("FAKE_WRAPPER").unwrap_or_else(|_| "/bin/false".into()))
                .unwrap_or_default()
        })
        .as_ptr()
}
unsafe extern "C" fn start_session() {
    log("FAKE_LOG", "sudo start_privileged_session");
}
unsafe extern "C" fn stop_session() {
    log("FAKE_LOG", "sudo stop_privileged_session");
}
unsafe extern "C" fn run_privileged(argv: *const *const c_char) -> c_int {
    let mut args = Vec::new();
    for index in 0.. {
        let arg = unsafe { *argv.add(index) };
        if arg.is_null() {
            break;
        }
        args.push(
            unsafe { CStr::from_ptr(arg) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    // The script's name is random; its content is kept for comparison.
    if let [_, script] = args.as_slice()
        && let Some(copy) = std::env::var_os("FAKE_SCRIPT_COPY")
    {
        let _ = std::fs::copy(script, copy);
    }
    let shown: Vec<&str> = args
        .iter()
        .map(|arg| {
            if arg.ends_with(".py") {
                "SCRIPT"
            } else {
                arg.as_str()
            }
        })
        .collect();
    log(
        "FAKE_LOG",
        &format!("sudo run_privileged {}", shown.join(" ")),
    );
    0
}

unsafe extern "C" fn askpass_path() -> *const c_char {
    c"/bin/false".as_ptr()
}

unsafe extern "C" fn not_compact(_: *mut c_void) -> bool {
    false
}

static HOST: HostApiV1 = HostApiV1 {
    abi_version: ABI_VERSION,
    struct_size: size_of::<HostApiV1>() as u32,
    language,
    translate,
    free_string,
    translate_widget,
    lock_ui,
    unlock_ui,
    sudo_validate_password: validate_password,
    sudo_set_password: set_password,
    sudo_has_password: no_password,
    sudo_forget_password: forget_password,
    sudo_wrapper_path: wrapper_path,
    sudo_start_session: start_session,
    sudo_stop_session: stop_session,
    sudo_run_privileged: run_privileged,
    refresh_widgets: no_op,
    sudo_askpass_path: askpass_path,
    window_compact_mode: not_compact,
};

fn load_descriptor(path: &Path) -> Result<&'static WidgetDescriptorV1, String> {
    // SAFETY: a trusted development build of the widget.
    let library = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
    // Like the Center, never unload: GTK keeps pointers into the library.
    let library = Box::leak(Box::new(library));
    let entry = unsafe { library.get::<EntryV1>(ENTRY_SYMBOL) }.map_err(|e| e.to_string())?;
    let descriptor = unsafe { entry(&HOST) };
    if !unsafe { Header::is_compatible(descriptor.cast(), size_of::<WidgetDescriptorV1>()) } {
        return Err("incompatible descriptor".into());
    }
    Ok(unsafe { &*descriptor })
}

fn find_all<T: IsA<gtk::Widget>>(widget: &gtk::Widget, found: &mut Vec<T>) {
    if let Ok(widget) = widget.clone().downcast::<T>() {
        found.push(widget);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        find_all(&current, found);
        child = current.next_sibling();
    }
}

fn render(widget: &impl IsA<gtk::Widget>, path: &Path) -> Result<(), String> {
    let widget = widget.as_ref();
    let (width, height) = (widget.width(), widget.height());
    let snapshot = gtk::Snapshot::new();
    gtk::WidgetPaintable::new(Some(widget)).snapshot(&snapshot, width.into(), height.into());
    let node = snapshot.to_node().ok_or("nothing was drawn")?;
    let renderer = widget
        .native()
        .and_then(|native| native.renderer())
        .ok_or("no renderer")?;
    let viewport = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    renderer
        .render_texture(&node, Some(&viewport))
        .save_to_png(path)
        .map_err(|e| e.to_string())?;
    println!("rendered {width}x{height} to {}", path.display());
    Ok(())
}

async fn sleep(duration: Duration) {
    glib::timeout_future(duration).await;
}

/// Wait (up to [`PATIENCE`]) until `ready` holds.
async fn until(what: &str, ready: impl Fn() -> bool) -> Result<(), String> {
    let start = Instant::now();
    while !ready() {
        if start.elapsed() > PATIENCE {
            return Err(format!("timed out waiting for {what}"));
        }
        sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

#[allow(deprecated)] // Adw.MessageDialog, as the Python widget uses
fn message_dialogs() -> Vec<adw::MessageDialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .filter(|dialog| dialog.get_visible())
        .collect()
}

#[allow(deprecated)]
fn print_dialogs() {
    for dialog in message_dialogs() {
        println!(
            "dialog heading={:?} body={:?}",
            dialog.heading().unwrap_or_default(),
            dialog.body()
        );
        let mut rows = Vec::<adw::ActionRow>::new();
        find_all(dialog.upcast_ref(), &mut rows);
        for row in rows {
            println!(
                "  row {:?} {:?}",
                row.title(),
                row.subtitle().unwrap_or_default()
            );
        }
        let mut labels = Vec::<gtk::Label>::new();
        find_all(dialog.upcast_ref(), &mut labels);
        for label in labels
            .iter()
            .filter(|label| label.has_css_class("monospace"))
        {
            println!("  item {:?}", label.label());
        }
    }
}

fn visible(widget: &gtk::Widget) -> bool {
    widget.is_mapped()
}

/// The stack among the widget's that has a page named `name`.
fn stack_with(widget: &gtk::Widget, name: &str) -> Option<gtk::Stack> {
    let mut stacks = Vec::<gtk::Stack>::new();
    find_all(widget, &mut stacks);
    stacks
        .into_iter()
        .find(|stack| stack.child_by_name(name).is_some())
}

fn page(widget: &gtk::Widget, name: &str) -> String {
    stack_with(widget, name)
        .and_then(|stack| stack.visible_child_name())
        .map(|name| name.to_string())
        .unwrap_or_default()
}

fn labels_in(widget: &gtk::Widget) -> Vec<gtk::Label> {
    let mut labels = Vec::<gtk::Label>::new();
    find_all(widget, &mut labels);
    labels
}

fn describe(widget: &gtk::Widget) -> String {
    let top = page(widget, "warning_view");
    let mut line = format!("state top={top}");
    if top != "main_view" {
        return line;
    }
    let content = page(widget, "search_view");
    line.push_str(&format!(" content={content}"));
    let Some(content_stack) = stack_with(widget, "search_view") else {
        return line;
    };
    let mut toggles = Vec::<gtk::ToggleButton>::new();
    find_all(widget, &mut toggles);
    if let Some(toggle) = toggles
        .iter()
        .find(|toggle| toggle.icon_name().as_deref() == Some("sidebar-show-right-symbolic"))
    {
        line.push_str(&format!(" sidebar={}", toggle.is_active()));
    }
    let mut revealers = Vec::<gtk::Revealer>::new();
    find_all(widget, &mut revealers);
    let revealed: Vec<String> = revealers
        .iter()
        .map(|revealer| revealer.reveals_child().to_string())
        .collect();
    line.push_str(&format!(" revealers=[{}]", revealed.join(",")));
    match content.as_str() {
        "search_view" => {
            let search = page(widget, "welcome");
            line.push_str(&format!(" search={search}"));
            if search == "results" {
                let shown = page(widget, "list");
                line.push_str(&format!(" shown={shown}"));
                if shown == "list" {
                    let mut views = Vec::<gtk::ListView>::new();
                    find_all(widget, &mut views);
                    if let Some(view) = views.first()
                        && let Some(model) = view.model()
                    {
                        line.push_str(&format!(" items={}", model.n_items()));
                    }
                    let mut rows = Vec::<adw::ActionRow>::new();
                    if let Some(view) = views.first() {
                        find_all(view.upcast_ref(), &mut rows);
                    }
                    for row in rows.iter().filter(|row| visible(row.upcast_ref())).take(6) {
                        let suffixes: Vec<String> = labels_in(row.upcast_ref())
                            .iter()
                            .filter(|label| label.get_visible() && label.has_css_class("dim-label"))
                            .map(|label| label.label().to_string())
                            .collect();
                        let mut buttons = Vec::<gtk::Button>::new();
                        find_all(row.upcast_ref(), &mut buttons);
                        let button = buttons
                            .first()
                            .map(|button| button.label().unwrap_or_default().to_string())
                            .unwrap_or_default();
                        line.push_str(&format!(
                            "\n  row {:?} {:?} {suffixes:?} {button:?}",
                            row.title(),
                            row.subtitle().unwrap_or_default()
                        ));
                    }
                } else if let Some(detail) =
                    stack_with(widget, "list").and_then(|stack| stack.child_by_name("detail"))
                {
                    let texts: Vec<String> = labels_in(&detail)
                        .iter()
                        .filter(|label| label.get_visible() && !label.label().is_empty())
                        .map(|label| label.label().to_string())
                        .collect();
                    let mut buttons = Vec::<gtk::Button>::new();
                    find_all(&detail, &mut buttons);
                    let shown: Vec<String> = buttons
                        .iter()
                        .filter(|button| button.get_visible() && button.label().is_some())
                        .map(|button| {
                            format!(
                                "{}{}",
                                button.label().unwrap_or_default(),
                                if button.is_sensitive() { "" } else { "(off)" }
                            )
                        })
                        .collect();
                    line.push_str(&format!("\n  detail {texts:?} {shown:?}"));
                }
            }
        }
        "progress_view" | "info_view" | "pkgbuild_view" => {
            if let Some(child) = content_stack.child_by_name(&content) {
                let texts: Vec<String> = labels_in(&child)
                    .iter()
                    .filter(|label| label.get_visible() && !label.label().is_empty())
                    .map(|label| label.label().to_string())
                    .collect();
                let mut buttons = Vec::<gtk::Button>::new();
                find_all(&child, &mut buttons);
                let shown: Vec<String> = buttons
                    .iter()
                    .filter(|button| button.get_visible())
                    .map(|button| {
                        format!(
                            "{}{}",
                            button.label().unwrap_or_default(),
                            if button.is_sensitive() { "" } else { "(off)" }
                        )
                    })
                    .collect();
                line.push_str(&format!("\n  texts {texts:?} buttons {shown:?}"));
                let mut views = Vec::<gtk::TextView>::new();
                find_all(&child, &mut views);
                for view in views {
                    let buffer = view.buffer();
                    let (start, end) = buffer.bounds();
                    line.push_str(&format!("\n  text {:?}", buffer.text(&start, &end, true)));
                }
            }
        }
        _ => {}
    }
    line
}

fn search_entry(widget: &gtk::Widget) -> Option<gtk::SearchEntry> {
    let mut entries = Vec::<gtk::SearchEntry>::new();
    find_all(widget, &mut entries);
    entries.into_iter().next()
}

fn first_visible_button(root: &gtk::Widget, label: &str) -> Option<gtk::Button> {
    let mut buttons = Vec::<gtk::Button>::new();
    find_all(root, &mut buttons);
    buttons
        .into_iter()
        .find(|button| button.label().as_deref() == Some(label) && button.is_mapped())
}

fn result_row(widget: &gtk::Widget, name: &str) -> Option<adw::ActionRow> {
    let mut views = Vec::<gtk::ListView>::new();
    find_all(widget, &mut views);
    let mut rows = Vec::<adw::ActionRow>::new();
    find_all(views.first()?.upcast_ref(), &mut rows);
    rows.into_iter()
        .find(|row| row.title() == name && row.is_mapped())
}

#[allow(deprecated)]
async fn step(
    window: &adw::ApplicationWindow,
    widget: &gtk::Widget,
    step: &str,
) -> Result<(), String> {
    let (name, value) = step.split_once('=').unwrap_or((step, ""));
    match name {
        "type" => search_entry(widget)
            .ok_or("no search entry")?
            .set_text(value),
        "check" => {
            let mut checks = Vec::<gtk::CheckButton>::new();
            find_all(widget, &mut checks);
            let check = checks
                .into_iter()
                .find(|check| check.label().as_deref() == Some(value))
                .ok_or(format!("no check {value:?}"))?;
            check.set_active(!check.is_active());
        }
        "click" => {
            first_visible_button(widget, value)
                .ok_or(format!("no button {value:?}"))?
                .emit_clicked();
        }
        "row" => {
            // The rows of an unscrolled list stand in the order of their items.
            let mut views = Vec::<gtk::ListView>::new();
            find_all(widget, &mut views);
            let view = views.first().ok_or("no list")?;
            let mut rows = Vec::<adw::ActionRow>::new();
            find_all(view.upcast_ref(), &mut rows);
            let position = rows
                .iter()
                .position(|row| row.title() == value)
                .ok_or(format!("no row {value:?}"))? as u32;
            view.emit_by_name::<()>("activate", &[&position]);
        }
        "row-action" => {
            let row = result_row(widget, value).ok_or(format!("no row {value:?}"))?;
            let mut buttons = Vec::<gtk::Button>::new();
            find_all(row.upcast_ref(), &mut buttons);
            buttons
                .first()
                .ok_or("row without a button")?
                .emit_clicked();
        }
        "select" => {
            let dialog = message_dialogs().into_iter().next().ok_or("no dialog")?;
            let mut lists = Vec::<gtk::ListBox>::new();
            find_all(dialog.upcast_ref(), &mut lists);
            let list = lists.first().ok_or("no list in the dialog")?;
            let mut rows = Vec::<adw::ActionRow>::new();
            find_all(list.upcast_ref(), &mut rows);
            let row = rows
                .into_iter()
                .find(|row| row.title() == value)
                .ok_or(format!("no version {value:?}"))?;
            list.select_row(Some(&row));
        }
        "activate" => {
            let mut rows = Vec::<adw::ActionRow>::new();
            find_all(widget, &mut rows);
            let row = rows
                .into_iter()
                .find(|row| row.title() == value && row.is_mapped())
                .ok_or(format!("no action row {value:?}"))?;
            adw::prelude::ActionRowExt::activate(&row);
        }
        "sidebar" => {
            let mut toggles = Vec::<gtk::ToggleButton>::new();
            find_all(widget, &mut toggles);
            let toggle = toggles
                .into_iter()
                .find(|toggle| toggle.icon_name().as_deref() == Some("sidebar-show-right-symbolic"))
                .ok_or("no sidebar button")?;
            toggle.set_active(!toggle.is_active());
        }
        "answer" => {
            until("a dialog", || !message_dialogs().is_empty()).await?;
            sleep(Duration::from_millis(500)).await;
            print_dialogs();
            let dialog = message_dialogs().into_iter().next().ok_or("no dialog")?;
            match value.split_once('=') {
                Some((response, password)) => {
                    let mut entries = Vec::<gtk::PasswordEntry>::new();
                    find_all(dialog.upcast_ref(), &mut entries);
                    entries
                        .first()
                        .ok_or("no password entry")?
                        .set_text(password);
                    dialog.response(response);
                    println!("answered {response} with a password");
                }
                None => {
                    dialog.response(value);
                    println!("answered {value}");
                }
            }
            sleep(Duration::from_millis(300)).await;
        }
        "wait" => until(value, || page(widget, "search_view") == value).await?,
        "wait-results" => {
            until("results", || {
                let search = page(widget, "welcome");
                search == "results" || search == "no_results"
            })
            .await?
        }
        "wait-done" => {
            until("the end", || {
                let content = page(widget, "search_view");
                content == "info_view"
                    || (content == "progress_view"
                        && first_visible_button(
                            widget,
                            &linexin_localization::global()
                                .translate("Back to Search", "package_manager"),
                        )
                        .is_some_and(|button| button.is_sensitive()))
            })
            .await?
        }
        "settle" => sleep(SETTLE).await,
        "sleep" => sleep(Duration::from_secs_f64(value.parse().unwrap_or(0.5))).await,
        "print" => {
            println!("{}", describe(widget));
            print_dialogs();
        }
        "shot" => render(window, Path::new(value))?,
        "dialog-shot" => {
            let dialog = message_dialogs()
                .into_iter()
                .next()
                .ok_or("no dialog to render")?;
            render(&dialog, Path::new(value))?;
        }
        "files" => {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            let config = std::fs::read_to_string(home.join(".config/linpama/config.json"))
                .unwrap_or_else(|_| "(none)".into());
            println!("config {config:?}");
        }
        other => return Err(format!("unknown step {other:?}")),
    }
    Ok(())
}

async fn scenario(window: adw::ApplicationWindow, widget: gtk::Widget) -> Result<(), String> {
    sleep(SETTLE).await;
    for current in &options().steps {
        step(&window, &widget, current).await?;
    }
    sleep(Duration::from_millis(300)).await;
    for dialog in message_dialogs() {
        dialog.response("cancel");
    }
    Ok(())
}

fn activate(app: &adw::Application) -> Result<(), String> {
    let options = options();
    adw::StyleManager::default().set_color_scheme(if options.dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    });
    // The Center's application style sheet (.dim-label, ...).
    let css = gtk::CssProvider::new();
    css.load_from_string(&match &options.css {
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => linexin_plugin_api::CENTER_STYLE_CSS.to_owned(),
    });
    let display = gtk::gdk::Display::default().ok_or("no display")?;
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let descriptor = load_descriptor(&options.library)?;
    let text = |pointer| {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    };
    println!(
        "widgetname={:?} icon={} id={}",
        text(descriptor.name),
        text(descriptor.icon),
        text(descriptor.widget_id)
    );

    let window = adw::ApplicationWindow::new(app);
    window.set_default_size(options.width, options.height);
    let context = CreateContextV1 {
        abi_version: ABI_VERSION,
        struct_size: size_of::<CreateContextV1>() as u32,
        host: &HOST,
        window: window.upcast_ref::<gtk::Window>().as_ptr().cast(),
        hide_sidebar: false,
        voice_autostart: false,
        conversation_id: std::ptr::null(),
    };
    let create = descriptor.create.ok_or("descriptor without create")?;
    let pointer = unsafe { create(&context) };
    if pointer.is_null() {
        return Err("create returned null".into());
    }
    let widget = unsafe { gtk::Widget::from_glib_full(pointer.cast::<gtk::ffi::GtkWidget>()) };
    window.set_content(Some(&widget));
    window.present();

    let app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        if let Err(error) = scenario(window, widget).await {
            eprintln!("harness: {error}");
            FAILED.with(|failed| failed.replace(true));
        }
        app.quit();
    });
    Ok(())
}

thread_local! {
    static FAILED: RefCell<bool> = const { RefCell::new(false) };
}

fn main() -> ExitCode {
    // The stylesheet, for the Python reference to load the same one.
    if std::env::args().nth(1).as_deref() == Some("--print-css") {
        print!("{}", linexin_plugin_api::CENTER_STYLE_CSS);
        return ExitCode::SUCCESS;
    }
    match parse_options() {
        Ok(options) => {
            let _ = OPTIONS.set(options);
            install_localization();
        }
        Err(error) => {
            eprintln!("harness: {error}");
            return ExitCode::FAILURE;
        }
    }
    let app = adw::Application::builder()
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| {
        if let Err(error) = activate(app) {
            eprintln!("harness: {error}");
            FAILED.with(|failed| failed.replace(true));
            app.quit();
        }
    });
    let status = app.run_with_args::<&str>(&[]);
    if status == glib::ExitCode::SUCCESS && !FAILED.with(|failed| *failed.borrow()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `name` where cargo built this harness's library: `<target>/<profile>/`,
/// beside the `examples/` holding the harness (wherever CARGO_TARGET_DIR put it).
fn beside_the_examples(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|harness| Some(harness.parent()?.parent()?.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}
