#!/usr/bin/env python3
"""Run the ORIGINAL y-package_manager.py the way examples/harness.rs runs the
native port (same options, same steps, same output lines), for comparison.

The original Python Center injected `_`, `translate_dialog` and
`translate_window` into the widget module after loading it, using the
package_manager dictionaries; the functions below are its
`_translate_with_patterns_helper`, `_translate_dialog` and helpers, verbatim
apart from the minimal localization manager they read. Its subprocess patch,
which locked the window while a command through the sudo wrapper ran, is
reduced to logging. `sudo_manager` is a stand-in that accepts only
$FAKE_PASSWORD and logs what it is asked. urllib's urlopen and urlretrieve
read the fixtures examples/compare.sh serves to the native widget's curl;
everything else the widget runs is a fake command of compare.sh."""

import argparse
import importlib.util
import io
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import warnings

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, Gio, GLib, Graphene, Gtk  # noqa: E402

warnings.filterwarnings("ignore", category=DeprecationWarning)
# The widget prints from other threads: keep the order.
sys.stdout.reconfigure(line_buffering=True)

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

parser = argparse.ArgumentParser()
# The original widget: installed, or the copy PYTHON_WIDGET names.
parser.add_argument("--widget", default=os.environ.get(
    "PYTHON_WIDGET", "/usr/share/linexin/widgets/y-package_manager.py"))
parser.add_argument("--localization",
                    default=os.path.join(REPO, "data/usr/share/linexin/widgets/localization"))
# The Center's stylesheet: examples/harness --print-css writes it (compare.sh
# does, and names it in LINEXIN_CENTER_CSS).
parser.add_argument("--css", default=os.environ.get("LINEXIN_CENTER_CSS"))
parser.add_argument("--lang", default="en_US")
parser.add_argument("--dark", action="store_true")
parser.add_argument("--width", type=int, default=1300)
parser.add_argument("--height", type=int, default=820)
parser.add_argument("--library", help="ignored (harness.rs option)")
parser.add_argument("steps", nargs="*")
args = parser.parse_args()
if not args.css:
    parser.error("--css: the Center's stylesheet is needed (examples/harness --print-css >FILE)")

WIDGET_ID = "package_manager"
FALLBACK_LANGUAGE = "en_US"


# --- The original Center's localization, reduced to one domain -------------

class LocalizationManager:
    def __init__(self):
        self.system_language = args.lang
        self.widget_translations = {}
        if os.path.isdir(args.localization):
            for language_code in sorted(os.listdir(args.localization)):
                path = os.path.join(args.localization, language_code, f"{WIDGET_ID}_dictionary.py")
                if not os.path.isfile(path):
                    continue
                spec = importlib.util.spec_from_file_location(f"{WIDGET_ID}_{language_code}_dict", path)
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                if hasattr(module, "translations"):
                    self.widget_translations.setdefault(WIDGET_ID, {})[language_code] = module.translations

    def get_text(self, key, widget_name="widget_loader"):
        if widget_name in self.widget_translations:
            widget_trans = self.widget_translations[widget_name]
            if self.system_language in widget_trans:
                if key in widget_trans[self.system_language]:
                    return widget_trans[self.system_language][key]
            if FALLBACK_LANGUAGE in widget_trans:
                if key in widget_trans[FALLBACK_LANGUAGE]:
                    return widget_trans[FALLBACK_LANGUAGE][key]
        return key


_manager = LocalizationManager()


def get_localization_manager():
    return _manager


def _(key, widget_name="widget_loader"):
    return get_localization_manager().get_text(key, widget_name)


def _translate_with_patterns_helper(text, widget_id):
    if not text:
        return ""
    translated = _(text, widget_id)
    if translated != text:
        return translated
    manager = get_localization_manager()
    if widget_id in manager.widget_translations:
        widget_trans = manager.widget_translations[widget_id]
        lang = manager.system_language
        trans_dict = widget_trans.get(lang) or widget_trans.get(FALLBACK_LANGUAGE, {})
        for pattern, translated_pattern in trans_dict.items():
            if '{}' not in pattern:
                continue
            escaped_pattern = re.escape(pattern)
            regex_pattern = escaped_pattern.replace(re.escape("{}"), r"(.*)")
            match = re.search(regex_pattern, text)
            if match and translated_pattern != pattern:
                if match.groups():
                    content = match.group(1)
                    translated_content = _translate_with_patterns_helper(content, widget_id)
                    translated_text = translated_pattern.replace("{}", translated_content, 1)
                    return text.replace(match.group(0), translated_text)
                else:
                    return text.replace(match.group(0), translated_pattern)
    return text


def _translate_dialog(dialog, widget_id=None):
    if widget_id is None:
        try:
            win = dialog.get_transient_for()
            if win and hasattr(win, 'active_widget_id'):
                widget_id = win.active_widget_id
        except Exception:
            pass
    if not widget_id:
        return
    if hasattr(dialog, 'get_heading') and dialog.get_heading():
        translated = _translate_with_patterns_helper(dialog.get_heading(), widget_id)
        if translated != dialog.get_heading():
            dialog.set_heading(translated)
    if hasattr(dialog, 'get_body') and dialog.get_body():
        translated = _translate_with_patterns_helper(dialog.get_body(), widget_id)
        if translated != dialog.get_body():
            dialog.set_body(translated)
    _translate_widget_recursive_static(dialog, widget_id)


def _translate_window(window, widget_id=None):
    if not widget_id:
        return
    _translate_widget_recursive_static(window, widget_id)


def _translate_widget_recursive_static(widget, widget_id):
    try:
        _translate_single_widget_static(widget, widget_id)
        if hasattr(widget, 'get_first_child'):
            child = widget.get_first_child()
            while child:
                _translate_widget_recursive_static(child, widget_id)
                child = child.get_next_sibling()
        elif hasattr(widget, 'get_child'):
            child = widget.get_child()
            if child:
                _translate_widget_recursive_static(child, widget_id)
    except Exception:
        pass


def _translate_single_widget_static(widget, widget_id):
    try:
        if isinstance(widget, Gtk.Label):
            if widget.get_use_markup():
                markup = widget.get_label()
                if markup:
                    translated_markup = _translate_markup_content_static(markup, widget_id)
                    if translated_markup != markup:
                        widget.set_markup(translated_markup)
            else:
                text = widget.get_text()
                if text:
                    translated = _translate_with_patterns_helper(text, widget_id)
                    if translated != text:
                        widget.set_text(translated)
        elif isinstance(widget, Gtk.Button):
            label = widget.get_label()
            if label:
                translated = _translate_with_patterns_helper(label, widget_id)
                if translated != label:
                    widget.set_label(translated)
        elif isinstance(widget, (Gtk.Entry, Gtk.SearchEntry, Gtk.PasswordEntry)):
            placeholder = widget.get_placeholder_text()
            if placeholder:
                translated = _translate_with_patterns_helper(placeholder, widget_id)
                if translated != placeholder:
                    widget.set_placeholder_text(translated)
        elif isinstance(widget, Adw.ActionRow):
            title = widget.get_title()
            if title:
                translated = _translate_with_patterns_helper(title, widget_id)
                if translated != title:
                    widget.set_title(translated)
            subtitle = widget.get_subtitle()
            if subtitle:
                translated = _translate_with_patterns_helper(subtitle, widget_id)
                if translated != subtitle:
                    widget.set_subtitle(translated)
        elif isinstance(widget, Adw.PreferencesGroup):
            title = widget.get_title()
            if title:
                translated = _translate_with_patterns_helper(title, widget_id)
                if translated != title:
                    widget.set_title(translated)
            desc = widget.get_description()
            if desc:
                translated = _translate_with_patterns_helper(desc, widget_id)
                if translated != desc:
                    widget.set_description(translated)
        elif isinstance(widget, (Adw.PreferencesRow, Adw.PreferencesPage)):
            title = widget.get_title()
            if title:
                translated = _translate_with_patterns_helper(title, widget_id)
                if translated != title:
                    widget.set_title(translated)
        elif isinstance(widget, Adw.StatusPage):
            title = widget.get_title()
            if title:
                translated = _translate_with_patterns_helper(title, widget_id)
                if translated != title:
                    widget.set_title(translated)
            desc = widget.get_description()
            if desc:
                translated = _translate_with_patterns_helper(desc, widget_id)
                if translated != desc:
                    widget.set_description(translated)
        elif isinstance(widget, (Gtk.Window, Adw.ApplicationWindow)):
            title = widget.get_title()
            if title:
                translated = _translate_with_patterns_helper(title, widget_id)
                if translated != title:
                    widget.set_title(translated)
    except Exception:
        pass


def _translate_markup_content_static(markup_text, widget_id):
    markup_text = _translate_with_patterns_helper(markup_text, widget_id)

    def replace_text(match):
        text_content = match.group(1)
        if text_content.strip():
            translated = _(text_content.strip(), widget_id)
            if translated != text_content.strip():
                leading_space = text_content[:len(text_content) - len(text_content.lstrip())]
                trailing_space = text_content[len(text_content.rstrip()):]
                return ">" + leading_space + translated + trailing_space + "<"
        return ">" + text_content + "<"
    return re.sub(r'>([^<]+)<', replace_text, markup_text)


# --- Stand-ins for the Center's services -----------------------------------

def log(variable, line):
    path = os.environ.get(variable)
    if path:
        with open(path, "a") as file:
            file.write(line + "\n")


class FakeSudo:
    user_password = None
    wrapper_path = os.environ.get("FAKE_WRAPPER", "/bin/false")
    askpass_script = "/bin/false"

    def validate_password(self, password):
        log("FAKE_LOG", "sudo validate_password")
        expected = os.environ.get("FAKE_PASSWORD", "")
        return bool(expected) and password == expected

    def set_password(self, password):
        log("FAKE_LOG", "sudo set_password")

    def forget_password(self):
        log("FAKE_LOG", "sudo forget_password")

    def start_privileged_session(self):
        log("FAKE_LOG", "sudo start_privileged_session")

    def stop_privileged_session(self):
        log("FAKE_LOG", "sudo stop_privileged_session")

    def get_env(self):
        return os.environ.copy()


sudo = FakeSudo()
_original_popen = subprocess.Popen


class LockedPopen(_original_popen):
    """The original Center's _LockedPopen, logging instead of locking."""

    def __init__(self, *popen_args, **kwargs):
        cmd = popen_args[0] if popen_args else kwargs.get("args")
        exe = ""
        if isinstance(cmd, (list, tuple)) and cmd:
            exe = str(cmd[0])
        elif isinstance(cmd, str):
            exe = cmd.split()[0] if cmd.strip() else ""
        wrapper = sudo.wrapper_path
        locks = bool(exe) and (exe == wrapper or exe.endswith(os.path.basename(wrapper)))
        if isinstance(cmd, str) and wrapper in cmd:
            locks = True
        if locks:
            log("FAKE_LOCK_LOG", "lock")
            super().__init__(*popen_args, **kwargs)

            def monitor():
                try:
                    self.wait()
                finally:
                    log("FAKE_LOCK_LOG", "unlock")
            threading.Thread(target=monitor, daemon=True).start()
        else:
            super().__init__(*popen_args, **kwargs)


subprocess.Popen = LockedPopen


def fixture(url):
    """The file examples/compare.sh serves for `url`, as its fake curl."""
    name = re.sub(r"[^A-Za-z0-9._-]", "_", url)
    path = os.path.join(os.environ.get("FAKE_HTTP", ""), name)
    log("FAKE_LOG", f"http {url}")
    if not os.path.isfile(path):
        raise urllib.error.HTTPError(url, 404, "Not Found", None, None)
    return path


def fake_urlopen(url, *popen_args, **kwargs):
    with open(fixture(url), "rb") as file:
        return io.BytesIO(file.read())


def fake_urlretrieve(url, filename=None, *more, **kwargs):
    shutil.copy(fixture(url), filename)
    return filename, None


urllib.request.urlopen = fake_urlopen
urllib.request.urlretrieve = fake_urlretrieve


# --- Load the widget module like the original _prepare_module ---------------

spec = importlib.util.spec_from_file_location("y-package_manager", args.widget)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def injected_underscore(message, widget_id=WIDGET_ID):
    return _translate_with_patterns_helper(message, widget_id)


module._ = injected_underscore
module.sudo_manager = sudo
module.translate_dialog = lambda dialog: _translate_dialog(dialog, WIDGET_ID)
module.translate_window = lambda window: _translate_window(window, WIDGET_ID)


# --- Steps (mirror examples/harness.rs) --------------------------------------

def q(text):
    """Rust's {:?} for the strings printed here."""
    return '"' + (text or "").replace("\\", "\\\\").replace('"', '\\"').replace(
        "\n", "\\n").replace("\r", "\\r").replace("\t", "\\t") + '"'


def ql(items):
    return "[" + ", ".join(q(item) for item in items) + "]"


def b(value):
    return "true" if value else "false"


def find_all(widget, kind, found):
    if isinstance(widget, kind):
        found.append(widget)
    child = widget.get_first_child()
    while child is not None:
        find_all(child, kind, found)
        child = child.get_next_sibling()
    return found


def render(widget, path):
    width, height = widget.get_width(), widget.get_height()
    snapshot = Gtk.Snapshot()
    Gtk.WidgetPaintable.new(widget).snapshot(snapshot, width, height)
    node = snapshot.to_node()
    renderer = widget.get_native().get_renderer()
    viewport = Graphene.Rect().init(0, 0, width, height)
    renderer.render_texture(node, viewport).save_to_png(path)
    print(f"rendered {width}x{height} to {path}")


def iterate(seconds):
    context = GLib.MainContext.default()
    until_time = time.monotonic() + seconds
    while time.monotonic() < until_time:
        while context.pending():
            context.iteration(False)
        time.sleep(0.002)


SETTLE = 1.5
PATIENCE = 30


def until(what, ready):
    start = time.monotonic()
    while not ready():
        if time.monotonic() - start > PATIENCE:
            raise RuntimeError(f"timed out waiting for {what}")
        iterate(0.05)


def message_dialogs():
    return [w for w in Gtk.Window.list_toplevels()
            if isinstance(w, Adw.MessageDialog) and w.get_visible()]


def print_dialogs():
    for dialog in message_dialogs():
        print(f"dialog heading={q(dialog.get_heading())} body={q(dialog.get_body())}")
        for row in find_all(dialog, Adw.ActionRow, []):
            print(f"  row {q(row.get_title())} {q(row.get_subtitle())}")
        for label in find_all(dialog, Gtk.Label, []):
            if label.has_css_class("monospace"):
                print(f"  item {q(label.get_label())}")


def stack_with(widget, name):
    for stack in find_all(widget, Gtk.Stack, []):
        if stack.get_child_by_name(name) is not None:
            return stack
    return None


def page(widget, name):
    stack = stack_with(widget, name)
    return (stack.get_visible_child_name() or "") if stack else ""


def label_text(label):
    return label.get_label() or ""


def button_text(button):
    return (button.get_label() or "") + ("" if button.get_sensitive() else "(off)")


def describe(widget):
    top = page(widget, "warning_view")
    line = f"state top={top}"
    if top != "main_view":
        return line
    content = page(widget, "search_view")
    line += f" content={content}"
    content_stack = stack_with(widget, "search_view")
    if content_stack is None:
        return line
    for toggle in find_all(widget, Gtk.ToggleButton, []):
        if toggle.get_icon_name() == "sidebar-show-right-symbolic":
            line += f" sidebar={b(toggle.get_active())}"
            break
    revealed = [b(r.get_reveal_child()) for r in find_all(widget, Gtk.Revealer, [])]
    line += f" revealers=[{','.join(revealed)}]"
    if content == "search_view":
        search = page(widget, "welcome")
        line += f" search={search}"
        if search == "results":
            shown = page(widget, "list")
            line += f" shown={shown}"
            if shown == "list":
                views = find_all(widget, Gtk.ListView, [])
                if views and views[0].get_model() is not None:
                    line += f" items={views[0].get_model().get_n_items()}"
                rows = find_all(views[0], Adw.ActionRow, []) if views else []
                for row in [r for r in rows if r.get_mapped()][:6]:
                    suffixes = [label_text(l) for l in find_all(row, Gtk.Label, [])
                                if l.get_visible() and l.has_css_class("dim-label")]
                    buttons = find_all(row, Gtk.Button, [])
                    button = (buttons[0].get_label() or "") if buttons else ""
                    line += f"\n  row {q(row.get_title())} {q(row.get_subtitle())} {ql(suffixes)} {q(button)}"
            else:
                list_stack = stack_with(widget, "list")
                detail = list_stack.get_child_by_name("detail") if list_stack else None
                if detail is not None:
                    texts = [label_text(l) for l in find_all(detail, Gtk.Label, [])
                             if l.get_visible() and label_text(l)]
                    shown_buttons = [button_text(bt) for bt in find_all(detail, Gtk.Button, [])
                                     if bt.get_visible() and bt.get_label() is not None]
                    line += f"\n  detail {ql(texts)} {ql(shown_buttons)}"
    elif content in ("progress_view", "info_view", "pkgbuild_view"):
        child = content_stack.get_child_by_name(content)
        if child is not None:
            texts = [label_text(l) for l in find_all(child, Gtk.Label, [])
                     if l.get_visible() and label_text(l)]
            shown_buttons = [button_text(bt) for bt in find_all(child, Gtk.Button, [])
                             if bt.get_visible()]
            line += f"\n  texts {ql(texts)} buttons {ql(shown_buttons)}"
            for view in find_all(child, Gtk.TextView, []):
                buffer = view.get_buffer()
                start, end = buffer.get_bounds()
                line += f"\n  text {q(buffer.get_text(start, end, True))}"
    return line


def search_entry(widget):
    entries = find_all(widget, Gtk.SearchEntry, [])
    return entries[0] if entries else None


def first_visible_button(root, label):
    for button in find_all(root, Gtk.Button, []):
        if button.get_label() == label and button.get_mapped():
            return button
    return None


def result_row(widget, name):
    views = find_all(widget, Gtk.ListView, [])
    if not views:
        return None
    for row in find_all(views[0], Adw.ActionRow, []):
        if row.get_title() == name and row.get_mapped():
            return row
    return None


def step(window, widget, current):
    name, _sep, value = current.partition("=")
    if name == "type":
        search_entry(widget).set_text(value)
    elif name == "check":
        checks = [c for c in find_all(widget, Gtk.CheckButton, []) if c.get_label() == value]
        if not checks:
            raise RuntimeError(f"no check {value!r}")
        checks[0].set_active(not checks[0].get_active())
    elif name == "click":
        button = first_visible_button(widget, value)
        if button is None:
            raise RuntimeError(f"no button {value!r}")
        button.emit("clicked")
    elif name == "row":
        views = find_all(widget, Gtk.ListView, [])
        rows = find_all(views[0], Adw.ActionRow, [])
        positions = [i for i, row in enumerate(rows) if row.get_title() == value]
        if not positions:
            raise RuntimeError(f"no row {value!r}")
        views[0].emit("activate", positions[0])
    elif name == "row-action":
        row = result_row(widget, value)
        if row is None:
            raise RuntimeError(f"no row {value!r}")
        find_all(row, Gtk.Button, [])[0].emit("clicked")
    elif name == "select":
        dialog = message_dialogs()[0]
        listbox = find_all(dialog, Gtk.ListBox, [])[0]
        rows = [r for r in find_all(listbox, Adw.ActionRow, []) if r.get_title() == value]
        if not rows:
            raise RuntimeError(f"no version {value!r}")
        listbox.select_row(rows[0])
    elif name == "activate":
        rows = [r for r in find_all(widget, Adw.ActionRow, [])
                if r.get_title() == value and r.get_mapped()]
        if not rows:
            raise RuntimeError(f"no action row {value!r}")
        rows[0].activate()
    elif name == "sidebar":
        for toggle in find_all(widget, Gtk.ToggleButton, []):
            if toggle.get_icon_name() == "sidebar-show-right-symbolic":
                toggle.set_active(not toggle.get_active())
                break
    elif name == "answer":
        until("a dialog", lambda: bool(message_dialogs()))
        iterate(0.5)
        print_dialogs()
        dialog = message_dialogs()[0]
        if "=" in value:
            response, password = value.split("=", 1)
            find_all(dialog, Gtk.PasswordEntry, [])[0].set_text(password)
            dialog.response(response)
            print(f"answered {response} with a password")
        else:
            dialog.response(value)
            print(f"answered {value}")
        iterate(0.3)
    elif name == "wait":
        until(value, lambda: page(widget, "search_view") == value)
    elif name == "wait-results":
        until("results", lambda: page(widget, "welcome") in ("results", "no_results"))
    elif name == "wait-done":
        back = _translate_with_patterns_helper("Back to Search", WIDGET_ID)

        def done():
            content = page(widget, "search_view")
            if content == "info_view":
                return True
            button = first_visible_button(widget, back)
            return content == "progress_view" and button is not None and button.get_sensitive()
        until("the end", done)
    elif name == "settle":
        iterate(SETTLE)
    elif name == "sleep":
        iterate(float(value or 0.5))
    elif name == "print":
        print(describe(widget))
        print_dialogs()
    elif name == "shot":
        render(window, value)
    elif name == "dialog-shot":
        render(message_dialogs()[0], value)
    elif name == "files":
        try:
            with open(os.path.expanduser("~/.config/linpama/config.json")) as file:
                config = file.read()
        except OSError:
            config = "(none)"
        print(f"config {q(config)}")
    else:
        raise RuntimeError(f"unknown step {name!r}")


failed = False


def activate(app):
    global failed
    Adw.StyleManager.get_default().set_color_scheme(
        Adw.ColorScheme.FORCE_DARK if args.dark else Adw.ColorScheme.FORCE_LIGHT)
    css = Gtk.CssProvider()
    with open(args.css) as style:
        css.load_from_string(style.read())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    window = Adw.ApplicationWindow(application=app)
    window.set_default_size(args.width, args.height)
    widget = module.LinexinPackageManager(hide_sidebar=False, window=window)
    print(f"widgetname={widget.widgetname!r} icon={widget.widgeticon} id={WIDGET_ID}")
    window.set_content(widget)
    window.present()

    def run():
        global failed
        try:
            iterate(SETTLE)
            for current in args.steps:
                step(window, widget, current)
            iterate(0.3)
            for dialog in message_dialogs():
                dialog.response("cancel")
        except Exception as error:  # noqa: BLE001
            print(f"reference: {error}", file=sys.stderr)
            failed = True
        app.quit()
        return False

    GLib.idle_add(run)


app = Adw.Application(flags=Gio.ApplicationFlags.NON_UNIQUE)
app.connect("activate", activate)
app.run([])
sys.exit(1 if failed else 0)
