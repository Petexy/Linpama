#!/usr/bin/env python3
import gi
import subprocess
import threading
import gettext
import locale
import os
import re
import glob
import json
import urllib.request
import urllib.parse
import shutil
import tempfile
import signal
import html as html_module
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
try:
    gi.require_version("WebKit", "6.0")
    from gi.repository import WebKit
    WEBKIT_AVAILABLE = True
except Exception:
    WEBKIT_AVAILABLE = False
from gi.repository import Gtk, Adw, GLib, Pango, Gdk, Gio, GObject
APP_NAME = "linpama"
LOCALE_DIR = os.path.abspath("/usr/share/locale")
CONFIG_DIR = os.path.expanduser(f"~/.config/{APP_NAME}")
CONFIG_FILE = os.path.join(CONFIG_DIR, "config.json")
WIDE_LAYOUT_THRESHOLD = 900
WIDE_LAYOUT_SIDE_PADDING = 12
LEFT_PANE_MIN_WIDTH = 400
RIGHT_PANE_MIN_WIDTH = 300
LAYOUT_ANIMATION_DURATION = 350
locale.setlocale(locale.LC_ALL, '')
locale.bindtextdomain(APP_NAME, LOCALE_DIR)
gettext.bindtextdomain(APP_NAME, LOCALE_DIR)
gettext.textdomain(APP_NAME)
_ = gettext.gettext
class PackageObject(GObject.Object):
    def __init__(self, name, repo, version, installed, desc, is_aur):
        super().__init__()
        self.name = name
        self.repo = repo
        self.version = version
        self.installed = installed
        self.desc = desc
        self.is_aur = is_aur
class LinexinPackageManager(Gtk.Box):
    def __init__(self, hide_sidebar=False, window=None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.widgetname = "Package Manager"
        self.widgeticon = "/usr/share/icons/github.petexy.linpama.svg" 
        self.set_margin_top(12)
        self.set_margin_bottom(50)
        self.set_margin_start(12)
        self.set_margin_end(12)
        self.window = window
        self.hide_sidebar = hide_sidebar
        self.user_password = None
        self.process_in_progress = False
        self.pulse_timer_id = None
        self.current_package_name = ""
        self.current_process = None
        self.setup_custom_styles()
        self.search_timer = None
        self.search_counter = 0
        self.search_in_progress = False
        self.all_search_results = []
        self.store = Gio.ListStore(item_type=PackageObject)
        self.displayed_count = 0
        self.batch_size = 50
        self.available_flatpak_ids = []
        self.flatpak_suffix_map = {}
        self.setup_appstream_icon_paths()
        threading.Thread(target=self.load_all_flatpak_ids, daemon=True).start()
        self.wide_layout_enabled = None
        self.last_measured_width = 0
        self.top_stack = Gtk.Stack()
        self.top_stack.set_transition_type(Gtk.StackTransitionType.CROSSFADE)
        self.top_stack.set_hexpand(True)
        self.top_stack.set_vexpand(True)
        self.main_layout_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.main_layout_box.set_hexpand(True)
        self.main_layout_box.set_vexpand(True)
        self.content_hbox = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        self.content_hbox.set_hexpand(True)
        self.content_hbox.set_vexpand(True)
        self.right_revealer = Gtk.Revealer()
        self.right_revealer.set_transition_type(Gtk.RevealerTransitionType.SLIDE_LEFT)
        self.right_revealer.set_transition_duration(LAYOUT_ANIMATION_DURATION)
        self.right_revealer.set_reveal_child(True)
        self.right_revealer.set_hexpand(False)
        self.right_revealer.set_vexpand(True)
        self.content_stack = Gtk.Stack()
        self.content_stack.set_transition_type(Gtk.StackTransitionType.SLIDE_LEFT_RIGHT)
        self.content_stack.set_hexpand(True)
        self.content_stack.set_vexpand(True)
        self.append(self.top_stack)
        self.setup_warning_view()
        self.setup_search_view()
        self.setup_pkgbuild_view()
        self.setup_progress_view()
        self.setup_info_view()
        self.setup_right_pane()
        self.update_adaptive_layout(force=True)
        GLib.timeout_add(200, self._monitor_adaptive_layout)
        self.connect("unrealize", self._on_unrealize)
        if self.should_show_warning():
            self.top_stack.set_visible_child_name("warning_view")
        else:
            self.top_stack.set_visible_child_name("main_view")
            self.content_stack.set_visible_child_name("search_view")
    def setup_custom_styles(self):
        css_provider = Gtk.CssProvider()
        css_provider.load_from_data(b"""
            .buttons_all {
                font-size: 14px;
                min-width: 200px;
                min-height: 40px;
            }
            .rounded-list {
                border-radius: 12px;
            }
        """)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), 
            css_provider, 
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )
    def _on_unrealize(self, widget):
        if WEBKIT_AVAILABLE and hasattr(self, 'detail_webview'):
            try:
                self.detail_webview.stop_loading()
                self.detail_webview.load_uri("about:blank")
                self.detail_webview.terminate_web_process()
            except Exception:
                pass
    def should_show_warning(self):
        if not os.path.exists(CONFIG_FILE):
            return True
        try:
            with open(CONFIG_FILE, 'r') as f:
                config = json.load(f)
                return config.get("show_warning", True)
        except Exception:
            return True
    def save_warning_preference(self, show_warning):
        try:
            os.makedirs(CONFIG_DIR, exist_ok=True)
            config = {}
            if os.path.exists(CONFIG_FILE):
                with open(CONFIG_FILE, 'r') as f:
                    config = json.load(f)
            config["show_warning"] = show_warning
            with open(CONFIG_FILE, 'w') as f:
                json.dump(config, f)
        except Exception as e:
            print(f"Failed to save config: {e}")
    def get_app_store_info(self):
        desktop = os.environ.get("XDG_CURRENT_DESKTOP", "").upper()
        if "GNOME" in desktop:
            return "gnome-software", _("Open GNOME Software instead")
        elif "KDE" in desktop:
            return "plasma-discover", _("Open Discover instead")
        if self.command_exists("gnome-software"):
            return "gnome-software", _("Open GNOME Software instead")
        elif self.command_exists("plasma-discover"):
            return "plasma-discover", _("Open Discover instead")
        return None, None
    def command_exists(self, cmd):
        return subprocess.call(["which", cmd], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) == 0
    def setup_warning_view(self):
        warn_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=15)
        warn_box.set_valign(Gtk.Align.CENTER)
        warn_box.set_halign(Gtk.Align.CENTER)
        warn_box.set_margin_start(30)
        warn_box.set_margin_end(30)
        icon = Gtk.Image.new_from_icon_name("dialog-warning")
        icon.set_pixel_size(64)
        icon.add_css_class("warning")
        warn_box.append(icon)
        title = Gtk.Label(label=_("System Stability Warning"))
        title.add_css_class("title-2")
        warn_box.append(title)
        desc_text = _(
            "Installing system packages directly can lead to conflicts and system instability.\n\n"
            "It is highly recommended to use <b>Flatpaks</b> for applications, as they are isolated "
            "and will not break your core system."
        )
        desc = Gtk.Label(label=desc_text)
        desc.set_use_markup(True)
        desc.set_wrap(True)
        desc.set_justify(Gtk.Justification.CENTER)
        desc.add_css_class("body")
        warn_box.append(desc)
        btn_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        btn_box.set_margin_top(20)
        btn_box.set_halign(Gtk.Align.CENTER) 
        cmd, label = self.get_app_store_info()
        if cmd:
            store_btn = Gtk.Button(label=label)
            store_btn.add_css_class("suggested-action")
            store_btn.add_css_class("buttons_all")
            store_btn.set_halign(Gtk.Align.CENTER) 
            store_btn.connect("clicked", lambda x: subprocess.Popen([cmd]))
            btn_box.append(store_btn)
        continue_btn = Gtk.Button(label=_("I Understand, Continue"))
        continue_btn.add_css_class("flat")
        continue_btn.add_css_class("buttons_all")
        continue_btn.set_halign(Gtk.Align.CENTER)
        continue_btn.connect("clicked", self.on_warning_continue)
        btn_box.append(continue_btn)
        self.dont_show_check = Gtk.CheckButton(label=_("Do not show this warning again"))
        self.dont_show_check.set_halign(Gtk.Align.CENTER)
        btn_box.append(self.dont_show_check)
        warn_box.append(btn_box)
        self.top_stack.add_named(warn_box, "warning_view")
    def on_warning_continue(self, btn):
        if self.dont_show_check.get_active():
            self.save_warning_preference(False)
        self.top_stack.set_visible_child_name("main_view")
        self.content_stack.set_visible_child_name("search_view")
    def setup_search_view(self):
        search_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        search_box.set_margin_start(30)
        search_box.set_margin_end(30)
        header_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        self.search_entry = Gtk.SearchEntry()
        self.search_entry.set_placeholder_text(_("Search for packages..."))
        self.search_entry.set_hexpand(True)
        self.search_entry.connect("search-changed", self.on_search_changed)
        header_box.append(self.search_entry)
        self.aur_check = Gtk.CheckButton(label=_("Search AUR"))
        self.aur_check.set_tooltip_text(_("Search Arch User Repository (Unstable/Community packages)"))
        self.aur_check.connect("toggled", lambda b: self.on_search_changed(self.search_entry))
        header_box.append(self.aur_check)
        self.compact_sidebar_btn = Gtk.ToggleButton()
        self.compact_sidebar_btn.set_icon_name("sidebar-show-right-symbolic")
        self.compact_sidebar_btn.set_tooltip_text(_("Show Actions"))
        self.compact_sidebar_btn.set_active(True)
        self._sidebar_toggled_handler = self.compact_sidebar_btn.connect("toggled", self._on_sidebar_btn_toggled)
        header_box.append(self.compact_sidebar_btn)
        search_box.append(header_box)
        self.search_stack = Gtk.Stack()
        self.search_stack.set_transition_type(Gtk.StackTransitionType.CROSSFADE)
        self.search_stack.set_vexpand(True)
        welcome_page = Adw.StatusPage()
        welcome_page.set_icon_name("system-search-symbolic")
        welcome_page.set_title(_("Search Packages"))
        welcome_page.set_description(_("Enter a package name above to search the repositories."))
        welcome_page.set_vexpand(True)
        self.search_stack.add_named(welcome_page, "welcome")
        loading_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        loading_box.set_valign(Gtk.Align.CENTER)
        loading_box.set_halign(Gtk.Align.CENTER)
        self.spinner = Gtk.Spinner()
        self.spinner.set_size_request(48, 48)
        self.spinner.start()
        loading_box.append(self.spinner)
        loading_lbl = Gtk.Label(label=_("Searching..."))
        loading_lbl.add_css_class("title-4")
        loading_box.append(loading_lbl)
        self.search_stack.add_named(loading_box, "loading")
        no_results_page = Adw.StatusPage()
        no_results_page.set_icon_name("edit-find-symbolic")
        no_results_page.set_title(_("No Results Found"))
        no_results_page.set_description(_("Try refining your search terms."))
        no_results_page.set_vexpand(True)
        self.search_stack.add_named(no_results_page, "no_results")
        self.list_detail_stack = Gtk.Stack()
        self.list_detail_stack.set_transition_duration(300)
        self.list_detail_stack.set_vexpand(True)
        self.results_scrolled = Gtk.ScrolledWindow()
        self.results_scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        self.results_scrolled.set_vexpand(True)
        self.results_scrolled.add_css_class("rounded-list")
        self.results_scrolled.set_overflow(Gtk.Overflow.HIDDEN)
        self.results_scrolled.connect("edge-reached", self.on_scroll_edge_reached)
        self.selection_model = Gtk.NoSelection(model=self.store)
        factory = Gtk.SignalListItemFactory()
        factory.connect("setup", self.setup_list_item)
        factory.connect("bind", self.bind_list_item)
        self.results_listview = Gtk.ListView(model=self.selection_model, factory=factory)
        self.results_listview.set_single_click_activate(True)
        self.results_listview.connect("activate", self.on_listview_item_activate)
        self.results_scrolled.set_child(self.results_listview)
        self.list_detail_stack.add_named(self.results_scrolled, "list")
        self._build_detail_page()
        self.search_stack.add_named(self.list_detail_stack, "results")
        search_box.append(self.search_stack)
        self.on_search_changed(self.search_entry)
        self.content_stack.add_named(search_box, "search_view")
    def setup_pkgbuild_view(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        box.set_margin_start(30)
        box.set_margin_end(30)
        box.set_margin_top(10)
        box.set_margin_bottom(10)
        header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        icon = Gtk.Image.new_from_icon_name("text-x-script")
        icon.set_pixel_size(32)
        header.append(icon)
        lbl = Gtk.Label(label=_("Review PKGBUILD"))
        lbl.add_css_class("title-3")
        header.append(lbl)
        box.append(header)
        desc = Gtk.Label(label=_("You are about to build a package from the AUR. Please review the build script carefully for malicious code."))
        desc.set_wrap(True)
        desc.set_halign(Gtk.Align.START)
        box.append(desc)
        self.pkgbuild_buffer = Gtk.TextBuffer()
        self.pkgbuild_view = Gtk.TextView.new_with_buffer(self.pkgbuild_buffer)
        self.pkgbuild_view.set_editable(False)
        self.pkgbuild_view.set_monospace(True)
        self.pkgbuild_view.set_wrap_mode(Gtk.WrapMode.NONE)
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_child(self.pkgbuild_view)
        scrolled.set_vexpand(True)
        scrolled.set_min_content_height(300)
        frame = Gtk.Frame()
        frame.set_child(scrolled)
        box.append(frame)
        actions = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        actions.set_halign(Gtk.Align.END)
        btn_cancel = Gtk.Button(label=_("Cancel"))
        btn_cancel.add_css_class("buttons_all")
        btn_cancel.connect("clicked", self.on_pkgbuild_cancel)
        actions.append(btn_cancel)
        btn_proceed = Gtk.Button(label=_("Proceed to Build"))
        btn_proceed.add_css_class("suggested-action")
        btn_proceed.add_css_class("buttons_all")
        btn_proceed.connect("clicked", self.on_pkgbuild_proceed)
        actions.append(btn_proceed)
        box.append(actions)
        self.content_stack.add_named(box, "pkgbuild_view")
    def setup_progress_view(self):
        progress_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=15)
        progress_box.set_margin_start(30)
        progress_box.set_margin_end(30)
        progress_box.set_valign(Gtk.Align.CENTER)
        header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self.progress_title = Gtk.Label(label=_("Processing..."))
        self.progress_title.add_css_class("title-2")
        header.append(self.progress_title)
        progress_box.append(header)
        self.progress_bar = Gtk.ProgressBar()
        self.progress_bar.set_pulse_step(0.1)
        self.progress_bar.set_hexpand(True)
        progress_box.append(self.progress_bar)
        self.lbl_progress_status = Gtk.Label(label=_("Please wait..."))
        self.lbl_progress_status.set_halign(Gtk.Align.START)
        self.lbl_progress_status.add_css_class("dim-label")
        progress_box.append(self.lbl_progress_status)
        self.btn_details = Gtk.Button(label=_("Show Details"))
        self.btn_details.add_css_class("flat")
        self.btn_details.connect("clicked", self.on_toggle_details)
        progress_box.append(self.btn_details)
        self.revealer_details = Gtk.Revealer()
        self.revealer_details.set_transition_type(Gtk.RevealerTransitionType.SLIDE_DOWN)
        self.output_buffer = Gtk.TextBuffer()
        self.output_textview = Gtk.TextView.new_with_buffer(self.output_buffer)
        self.output_textview.set_editable(False)
        self.output_textview.set_monospace(True)
        self.output_textview.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_child(self.output_textview)
        scrolled.set_min_content_height(200)
        scrolled.set_vexpand(True)
        frame = Gtk.Frame()
        frame.set_child(scrolled)
        self.revealer_details.set_child(frame)
        progress_box.append(self.revealer_details)
        btn_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        btn_box.set_halign(Gtk.Align.END)
        self.btn_cancel = Gtk.Button(label=_("Cancel"))
        self.btn_cancel.add_css_class("destructive-action")
        self.btn_cancel.add_css_class("buttons_all")
        self.btn_cancel.connect("clicked", self.on_cancel_clicked)
        btn_box.append(self.btn_cancel)
        self.btn_back = Gtk.Button(label=_("Back to Search"))
        self.btn_back.add_css_class("buttons_all")
        self.btn_back.connect("clicked", self.on_back_clicked)
        self.btn_back.set_sensitive(False)
        btn_box.append(self.btn_back)
        progress_box.append(btn_box)
        self.content_stack.add_named(progress_box, "progress_view")
    def on_toggle_details(self, btn):
        if self.revealer_details.get_reveal_child():
            self.revealer_details.set_reveal_child(False)
            btn.set_label(_("Show Details"))
        else:
            self.revealer_details.set_reveal_child(True)
            btn.set_label(_("Hide Details"))
    def on_cancel_clicked(self, btn):
        if self.current_process:
            self.append_log(f"\n{_('--- Cancelling operation... ---')}\n")
            try:
                self.current_process.send_signal(signal.SIGINT)
            except Exception as e:
                print(f"Error sending signal: {e}")
    def setup_info_view(self):
        info_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=20)
        info_box.set_valign(Gtk.Align.CENTER)
        info_box.set_halign(Gtk.Align.CENTER)
        self.info_icon = Gtk.Image()
        self.info_icon.set_pixel_size(64)
        info_box.append(self.info_icon)
        self.info_text = Gtk.Label()
        self.info_text.add_css_class("title-3")
        info_box.append(self.info_text)
        self.btn_view_log = Gtk.Button(label=_("View Log"))
        self.btn_view_log.add_css_class("flat")
        self.btn_view_log.connect("clicked", self.on_view_log_clicked)
        info_box.append(self.btn_view_log)
        btn_return = Gtk.Button(label=_("Search Again"))
        btn_return.add_css_class("buttons_all")
        btn_return.connect("clicked", self.on_back_clicked)
        info_box.append(btn_return)
        self.content_stack.add_named(info_box, "info_view")
    def on_view_log_clicked(self, btn):
        self.content_stack.set_visible_child_name("progress_view")
        self.revealer_details.set_reveal_child(True)
        self.btn_details.set_label(_("Hide Details"))
        self.progress_bar.set_visible(False)
        self.lbl_progress_status.set_text(_("Transaction Log"))
        self.btn_back.set_sensitive(True)
        self.btn_cancel.set_visible(False)
        if self.action_type == "remove":
            self.progress_title.set_text(_("Removed {}").format(self.current_package_name))
        else:
            self.progress_title.set_text(_("Installed {}").format(self.current_package_name))
    def setup_appstream_icon_paths(self):
        icon_theme = Gtk.IconTheme.get_for_display(Gdk.Display.get_default())
        base_path = "/var/lib/flatpak/appstream"
        found_paths = []
        if os.path.exists(base_path):
            search_pattern = os.path.join(base_path, "*", "*", "active", "icons")
            icon_roots = glob.glob(search_pattern)
            for root in icon_roots:
                for size in ["64x64", "128x128", "64", "128"]:
                    icon_dir = os.path.join(root, size)
                    if os.path.exists(icon_dir):
                        found_paths.append(icon_dir)
                        icon_theme.add_search_path(icon_dir)
    def load_all_flatpak_ids(self):
        try:
            cmd = ["flatpak", "remote-ls", "--app", "--columns=application"]
            res = subprocess.run(cmd, capture_output=True, text=True, env={**os.environ, 'LC_ALL': 'C'})
            if res.returncode == 0:
                self.available_flatpak_ids = [line.strip() for line in res.stdout.split('\n') if line.strip()]
                for fid in self.available_flatpak_ids:
                    suffix = fid.split('.')[-1].lower()
                    if suffix not in self.flatpak_suffix_map:
                        self.flatpak_suffix_map[suffix] = []
                    self.flatpak_suffix_map[suffix].append(fid)
        except Exception:
            pass
    def resolve_icon_name(self, package_name):
        icon_theme = Gtk.IconTheme.get_for_display(Gdk.Display.get_default())
        if icon_theme.has_icon(package_name):
            return package_name
        clean_name = re.sub(r'(-bin|-git|-nightly|-stable|-beta)$', '', package_name)
        if clean_name != package_name and icon_theme.has_icon(clean_name):
            return clean_name
        pkg_lower = clean_name.lower()
        matches = self.flatpak_suffix_map.get(pkg_lower, [])
        for fid in matches:
            if icon_theme.has_icon(fid):
                return fid
        known_mappings = {
            "ttf-google-fonts-git": "preferences-desktop-font",
            "noto-fonts": "preferences-desktop-font",
            "base-devel": "applications-engineering",
            "linux": "system-run",
            "networkmanager": "network-workgroup",
            "code": "visual-studio-code",
            "steam-native-runtime": "steam"
        }
        if package_name in known_mappings:
            return known_mappings[package_name]
        return "package-x-generic"
    def on_search_changed(self, entry):
        if self.search_timer:
            GLib.source_remove(self.search_timer)
            self.search_timer = None
        if self.list_detail_stack.get_visible_child_name() == "detail":
            self.list_detail_stack.set_transition_type(Gtk.StackTransitionType.SLIDE_RIGHT)
            self.list_detail_stack.set_visible_child_name("list")
        query = entry.get_text().strip()
        self.search_counter += 1
        current_search_id = self.search_counter
        self.search_timer = GLib.timeout_add(400, self.trigger_search, query, current_search_id)
    def trigger_search(self, query, search_id):
        self.search_stack.set_visible_child_name("loading")
        self.spinner.start()
        self.search_in_progress = True
        self.all_search_results = []
        self.displayed_count = 0
        self.store.remove_all()
        threading.Thread(
            target=self.perform_search, 
            args=(query, search_id), 
            daemon=True
        ).start()
        return False
    def perform_search(self, query, search_id):
        if search_id != self.search_counter: return
        results = []
        try:
            if not query:
                cmd = ["pacman", "-Qs"]
            else:
                cmd = ["pacman", "-Ss", query]
            process = subprocess.run(cmd, capture_output=True, text=True, env={**os.environ, 'LC_ALL': 'C'})
            if search_id != self.search_counter: return
            if process.returncode == 0 and process.stdout:
                lines = process.stdout.strip().split('\n')
                current_pkg = None
                for line in lines:
                    if not line.startswith('    '):
                        if current_pkg: results.append(current_pkg)
                        parts = line.split(' ')
                        full_name = parts[0]
                        version = parts[1]
                        repo, name = full_name.split('/') if '/' in full_name else ("local", full_name)
                        installed = not query or "[installed]" in line
                        current_pkg = {
                            'name': name, 'repo': repo,
                            'version': version, 'installed': installed,
                            'desc': "",
                            'is_aur': False
                        }
                    else:
                        if current_pkg: current_pkg['desc'] = line.strip()
                if current_pkg: results.append(current_pkg)
        except Exception as e:
            print(f"Repo search error: {e}")
        installed_pkgs = set()
        try:
            p_q = subprocess.run(["pacman", "-Qq"], capture_output=True, text=True, env={**os.environ, 'LC_ALL': 'C'})
            if p_q.returncode == 0:
                installed_pkgs = set(p_q.stdout.strip().split('\n'))
        except:
            pass
        existing_names = {r['name'] for r in results}
        if query and self.aur_check.get_active():
            try:
                rpc_url = f"https://aur.archlinux.org/rpc/?v=5&type=search&arg={urllib.parse.quote(query)}"
                with urllib.request.urlopen(rpc_url, timeout=5) as response:
                    data = json.loads(response.read().decode())
                    if search_id != self.search_counter: return
                    if data.get("type") != "error" and data.get("results"):
                        for item in data["results"]:
                            pkg_name = item['Name']
                            if query.lower() in pkg_name.lower() and pkg_name not in existing_names:
                                is_installed = pkg_name in installed_pkgs
                                results.append({
                                    'name': pkg_name,
                                    'repo': 'AUR',
                                    'version': item['Version'],
                                    'installed': is_installed,
                                    'desc': item.get('Description', ''),
                                    'is_aur': True
                                })
            except Exception as e:
                print(f"AUR search error: {e}")
        if search_id == self.search_counter:
            if query:
                q = query.lower()
                def sort_key(r):
                    name = r['name'].lower()
                    if name == q:
                        return (0, name)
                    elif name.startswith(q):
                        return (1, name)
                    elif q in name:
                        return (2, name)
                    else:
                        return (3, name)
                results.sort(key=sort_key)
            self.all_search_results = results
            GLib.idle_add(self.update_results_initial)
    def update_results_initial(self):
        self.search_in_progress = False
        self.spinner.stop()
        if not self.all_search_results:
            self.store.remove_all()
            self.search_stack.set_visible_child_name("no_results")
            return
        self.search_stack.set_visible_child_name("results")
        self.load_more_results()
    def on_scroll_edge_reached(self, scrolled, pos):
        if pos == Gtk.PositionType.BOTTOM:
            self.load_more_results()
    def load_more_results(self):
        total = len(self.all_search_results)
        if self.displayed_count >= total:
            return
        end_idx = min(self.displayed_count + self.batch_size, total)
        batch = self.all_search_results[self.displayed_count:end_idx]
        new_items = []
        for pkg in batch:
            new_items.append(PackageObject(
                pkg['name'], pkg['repo'], pkg['version'], 
                pkg['installed'], pkg['desc'], pkg.get('is_aur', False)
            ))
        self.store.splice(self.store.get_n_items(), 0, new_items)
        self.displayed_count = end_idx
    def _build_detail_page(self):
        detail_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        detail_box.set_vexpand(True)

        # ── Toolbar: back button + package name ──
        toolbar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        toolbar.set_margin_top(6)
        toolbar.set_margin_start(6)
        toolbar.set_margin_end(6)
        toolbar.set_margin_bottom(6)
        back_btn = Gtk.Button()
        back_btn.set_icon_name("go-previous-symbolic")
        back_btn.add_css_class("flat")
        back_btn.add_css_class("circular")
        back_btn.set_tooltip_text(_("Back"))
        back_btn.connect("clicked", self.on_detail_back_clicked)
        toolbar.append(back_btn)
        self.detail_header_title = Gtk.Label()
        self.detail_header_title.add_css_class("title-4")
        self.detail_header_title.set_ellipsize(Pango.EllipsizeMode.END)
        self.detail_header_title.set_hexpand(True)
        self.detail_header_title.set_halign(Gtk.Align.START)
        toolbar.append(self.detail_header_title)
        detail_box.append(toolbar)
        detail_box.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        scrolled = Gtk.ScrolledWindow()
        # ── App header (fixed, not scrolled) ──
        app_header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=20)
        app_header.set_valign(Gtk.Align.START)
        app_header.set_margin_top(24)
        app_header.set_margin_bottom(16)
        app_header.set_margin_start(24)
        app_header.set_margin_end(24)

        self.detail_icon = Gtk.Image()
        self.detail_icon.set_pixel_size(96)
        self.detail_icon.set_valign(Gtk.Align.START)
        app_header.append(self.detail_icon)

        meta_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        meta_box.set_valign(Gtk.Align.CENTER)
        meta_box.set_hexpand(True)

        self.detail_name_label = Gtk.Label()
        self.detail_name_label.add_css_class("title-1")
        self.detail_name_label.set_halign(Gtk.Align.START)
        self.detail_name_label.set_wrap(True)
        meta_box.append(self.detail_name_label)

        self.detail_version_label = Gtk.Label()
        self.detail_version_label.add_css_class("dim-label")
        self.detail_version_label.set_halign(Gtk.Align.START)
        meta_box.append(self.detail_version_label)

        self.detail_status_label = Gtk.Label()
        self.detail_status_label.set_halign(Gtk.Align.START)
        meta_box.append(self.detail_status_label)

        self.detail_aur_badge = Gtk.Label()
        self.detail_aur_badge.set_markup("<span background='#a40000' color='white' size='small'><b> AUR </b></span>")
        self.detail_aur_badge.set_halign(Gtk.Align.START)
        meta_box.append(self.detail_aur_badge)

        app_header.append(meta_box)

        btn_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        btn_box.set_valign(Gtk.Align.CENTER)

        self.detail_btn_size_group = Gtk.SizeGroup(mode=Gtk.SizeGroupMode.HORIZONTAL)

        self.detail_version_btn = Gtk.Button()
        self.detail_version_btn.set_label(_("Change Version"))
        self.detail_version_btn.add_css_class("pill")
        self.detail_version_btn.connect("clicked", self.on_detail_change_version_clicked)
        self.detail_btn_size_group.add_widget(self.detail_version_btn)
        btn_box.append(self.detail_version_btn)

        self.detail_install_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)

        self.detail_install_older_btn = Gtk.Button()
        self.detail_install_older_btn.set_label(_("Install Older Version"))
        self.detail_install_older_btn.add_css_class("pill")
        self.detail_install_older_btn.connect("clicked", self.on_detail_change_version_clicked)
        self.detail_install_row.append(self.detail_install_older_btn)

        self.detail_action_btn = Gtk.Button()
        self.detail_action_btn.add_css_class("pill")
        self.detail_action_btn.connect("clicked", self.on_detail_action_clicked)
        self.detail_btn_size_group.add_widget(self.detail_action_btn)
        self.detail_install_row.append(self.detail_action_btn)

        btn_box.append(self.detail_install_row)

        app_header.append(btn_box)

        detail_box.append(app_header)
        detail_box.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # ── Short pacman description ──
        self.detail_desc_label = Gtk.Label()
        self.detail_desc_label.set_wrap(True)
        self.detail_desc_label.set_halign(Gtk.Align.START)
        self.detail_desc_label.add_css_class("body")
        self.detail_desc_label.set_margin_top(12)
        self.detail_desc_label.set_margin_bottom(12)
        self.detail_desc_label.set_margin_start(24)
        self.detail_desc_label.set_margin_end(24)
        detail_box.append(self.detail_desc_label)
        detail_box.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # ── Wiki section header ──
        self.detail_wiki_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.detail_wiki_box.set_vexpand(True)
        self.detail_wiki_url = None
        self.detail_wiki_btn = Gtk.Button()
        self.detail_wiki_btn.add_css_class("flat")
        self.detail_wiki_btn.set_margin_top(4)
        self.detail_wiki_btn.set_margin_bottom(4)
        self.detail_wiki_btn.set_margin_start(18)
        self.detail_wiki_btn.set_margin_end(18)
        wiki_btn_content = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        wiki_icon = Gtk.Image.new_from_icon_name("help-browser-symbolic")
        wiki_icon.set_pixel_size(16)
        wiki_btn_content.append(wiki_icon)
        wiki_title_lbl = Gtk.Label(label=_("Arch Wiki"))
        wiki_title_lbl.add_css_class("heading")
        wiki_title_lbl.set_hexpand(True)
        wiki_title_lbl.set_halign(Gtk.Align.START)
        wiki_btn_content.append(wiki_title_lbl)
        wiki_open_icon = Gtk.Image.new_from_icon_name("web-browser-symbolic")
        wiki_open_icon.set_pixel_size(14)
        wiki_btn_content.append(wiki_open_icon)
        self.detail_wiki_spinner = Gtk.Spinner()
        self.detail_wiki_spinner.set_size_request(16, 16)
        wiki_btn_content.append(self.detail_wiki_spinner)
        self.detail_wiki_btn.set_child(wiki_btn_content)
        self.detail_wiki_btn.connect("clicked", self._on_wiki_open_clicked)
        self.detail_wiki_box.append(self.detail_wiki_btn)

        if WEBKIT_AVAILABLE:
            settings = WebKit.Settings()
            settings.set_enable_javascript(False)
            settings.set_enable_media(False)
            ucm = WebKit.UserContentManager()
            ucm.add_style_sheet(
                WebKit.UserStyleSheet(
                    self._WIKI_CSS,
                    WebKit.UserContentInjectedFrames.ALL_FRAMES,
                    WebKit.UserStyleLevel.USER,
                    None, None
                )
            )
            self.detail_webview = WebKit.WebView(
                settings=settings,
                user_content_manager=ucm
            )
            self.detail_webview.set_background_color(Gdk.RGBA(0, 0, 0, 0))
            self.detail_webview.set_vexpand(True)
            self.detail_wiki_box.append(self.detail_webview)
        else:
            self.detail_wiki_label = Gtk.Label()
            self.detail_wiki_label.set_wrap(True)
            self.detail_wiki_label.set_halign(Gtk.Align.START)
            self.detail_wiki_label.add_css_class("body")
            self.detail_wiki_label.set_margin_top(12)
            self.detail_wiki_label.set_margin_start(24)
            self.detail_wiki_label.set_margin_end(24)
            self.detail_wiki_box.append(self.detail_wiki_label)

        detail_box.append(self.detail_wiki_box)
        self.list_detail_stack.add_named(detail_box, "detail")
    def show_package_detail(self, pkg):
        self.detail_current_pkg = pkg
        self.detail_header_title.set_label(pkg.name)
        self.detail_icon.set_from_icon_name(self.resolve_icon_name(pkg.name))
        self.detail_name_label.set_label(pkg.name)
        self.detail_version_label.set_label(f"{pkg.version}  ·  {pkg.repo}")
        self.detail_aur_badge.set_visible(pkg.is_aur)
        self.detail_version_btn.set_visible(pkg.installed and not pkg.is_aur)
        self.detail_install_older_btn.set_visible(not pkg.installed and not pkg.is_aur)
        if pkg.installed:
            self.detail_status_label.set_markup(f"<span foreground='#33d17a'>● {_('Installed')}</span>")
            self.detail_action_btn.set_label(_("Remove"))
            self.detail_action_btn.remove_css_class("suggested-action")
            self.detail_action_btn.add_css_class("destructive-action")
        else:
            self.detail_status_label.set_markup(f"<span foreground='gray'>○ {_('Not installed')}</span>")
            self.detail_action_btn.set_label(_("Install"))
            self.detail_action_btn.remove_css_class("destructive-action")
            self.detail_action_btn.add_css_class("suggested-action")
        self.detail_desc_label.set_label(pkg.desc or _("No description available."))
        self._wiki_fetch_id = getattr(self, '_wiki_fetch_id', 0) + 1
        self.detail_wiki_box.set_visible(True)
        self.detail_wiki_spinner.start()
        if WEBKIT_AVAILABLE:
            self.detail_webview.load_uri("about:blank")
        threading.Thread(
            target=self._fetch_wiki_description,
            args=(pkg.name, self._wiki_fetch_id),
            daemon=True
        ).start()
        self.list_detail_stack.set_transition_type(Gtk.StackTransitionType.SLIDE_LEFT)
        self.list_detail_stack.set_visible_child_name("detail")
    def _fetch_wiki_description(self, pkg_name, fetch_id):
        candidates = [pkg_name]
        titled = pkg_name.replace('-', ' ').title().replace(' ', '-')
        if titled not in candidates:
            candidates.append(titled)
        plain_titled = pkg_name.replace('-', ' ').title()
        if plain_titled not in candidates:
            candidates.append(plain_titled)
        for title in candidates:
            try:
                api_url = (
                    "https://wiki.archlinux.org/api.php?"
                    "action=parse&prop=text&format=json&redirects=1"
                    f"&page={urllib.parse.quote(title)}"
                )
                with urllib.request.urlopen(api_url, timeout=10) as resp:
                    data = json.loads(resp.read().decode())
                if "error" in data:
                    continue
                body_html = data.get("parse", {}).get("text", {}).get("*", "")
                page_title = data.get("parse", {}).get("title", title)
                if body_html:
                    wiki_url = f"https://wiki.archlinux.org/title/{urllib.parse.quote(page_title)}"
                    GLib.idle_add(self._on_wiki_fetched, body_html, fetch_id, wiki_url)
                    return
            except Exception:
                pass
        # Fallback: try OpenSearch with progressively shorter prefixes of the
        # package name (e.g. nvidia-open → nvidia-open, then nvidia) so that
        # packages like nvidia-open resolve to the "NVIDIA" wiki page.
        parts = pkg_name.split('-')
        seen_search_terms = set()
        for i in range(len(parts), 0, -1):
            term = '-'.join(parts[:i])
            if term in seen_search_terms:
                continue
            seen_search_terms.add(term)
            try:
                search_url = (
                    "https://wiki.archlinux.org/api.php?"
                    "action=opensearch&format=json&redirects=resolve&limit=1"
                    f"&search={urllib.parse.quote(term)}"
                )
                with urllib.request.urlopen(search_url, timeout=10) as resp:
                    results = json.loads(resp.read().decode())
                # results = [query, [titles], [descriptions], [urls]]
                titles = results[1] if len(results) > 1 else []
                if not titles:
                    continue
                top_title = titles[0]
                api_url = (
                    "https://wiki.archlinux.org/api.php?"
                    "action=parse&prop=text&format=json&redirects=1"
                    f"&page={urllib.parse.quote(top_title)}"
                )
                with urllib.request.urlopen(api_url, timeout=10) as resp:
                    data = json.loads(resp.read().decode())
                if "error" in data:
                    continue
                body_html = data.get("parse", {}).get("text", {}).get("*", "")
                page_title = data.get("parse", {}).get("title", top_title)
                if body_html:
                    wiki_url = f"https://wiki.archlinux.org/title/{urllib.parse.quote(page_title)}"
                    GLib.idle_add(self._on_wiki_fetched, body_html, fetch_id, wiki_url)
                    return
            except Exception:
                pass
        GLib.idle_add(self._on_wiki_fetched, None, fetch_id, None)
    _WIKI_CSS = """
        html, body {
            background: transparent !important;
            font-family: Cantarell, sans-serif !important;
            margin: 0 !important;
            padding: 0 16px !important;
            font-size: 15px !important;
            line-height: 1.6 !important;
        }
        /* Hide related-articles box and TOC */
        .archwiki-template-meta-related-articles,
        .toc, #toc, .mw-editsection { display: none !important; }

        a { text-decoration: none !important; }
        a:hover { text-decoration: underline !important; }

        h1, h2, h3, h4, h5 { margin-top: 1.2em !important; }
        h2 { padding-bottom: 4px !important; }

        code { padding: 1px 5px !important; }
        pre, code, kbd, .mw-highlight { border-radius: 6px !important; }
        pre, .mw-highlight {
            padding: 12px 16px !important;
            overflow-x: auto !important;
            line-height: 1.6 !important;
        }
        pre code { background: transparent !important; border: none !important;
                   padding: 0 !important; }

        table.wikitable { border-collapse: collapse !important; }
        table.wikitable td, table.wikitable th { padding: 6px 10px !important; }

        .archwiki-template-box,
        .archwiki-template-box-note,
        .archwiki-template-box-warning,
        .archwiki-template-box-tip {
            border-radius: 4px !important;
            padding: 10px 14px !important;
            margin: 12px 0 !important;
        }

        img { max-width: 100% !important; }
        dl { margin-left: 1em !important; }
        dt { font-weight: bold !important; }

        /* ---- Dark mode ---- */
        @media (prefers-color-scheme: dark) {
            html, body { color: #e0e0e0 !important; }
            a { color: #78aeed !important; }
            a:visited { color: #a78aee !important; }
            h1, h2, h3, h4, h5 { color: #ffffff !important; }
            h2 { border-bottom: 1px solid rgba(255,255,255,0.12) !important; }
            dt { color: #ffffff !important; }

            pre, code, kbd, .mw-highlight {
                background: rgba(255,255,255,0.07) !important;
                color: #d8d8d8 !important;
                border: 1px solid rgba(255,255,255,0.1) !important;
            }

            table.wikitable { border: 1px solid rgba(255,255,255,0.15) !important; }
            table.wikitable th {
                background: rgba(255,255,255,0.08) !important;
                color: #ffffff !important;
            }
            table.wikitable td, table.wikitable th {
                border: 1px solid rgba(255,255,255,0.15) !important;
                color: #e0e0e0 !important;
            }

            .archwiki-template-box,
            .archwiki-template-box-note,
            .archwiki-template-box-warning,
            .archwiki-template-box-tip {
                background: rgba(255,255,255,0.05) !important;
                border-left: 3px solid rgba(255,255,255,0.3) !important;
                color: #e0e0e0 !important;
            }
            .archwiki-template-box-warning {
                border-left-color: rgba(220,80,40,0.8) !important;
                background: rgba(220,60,30,0.08) !important;
            }
            .archwiki-template-box-note {
                border-left-color: rgba(80,150,230,0.8) !important;
                background: rgba(60,120,230,0.08) !important;
            }
            .archwiki-template-box-tip {
                border-left-color: rgba(60,190,80,0.8) !important;
                background: rgba(40,170,60,0.08) !important;
            }
        }

        /* ---- Light mode ---- */
        @media (prefers-color-scheme: light) {
            html, body { color: #1a1a1a !important; }
            a { color: #1a73e8 !important; }
            a:visited { color: #6a1b9a !important; }
            h1, h2, h3, h4, h5 { color: #1a1a1a !important; }
            h2 { border-bottom: 1px solid rgba(0,0,0,0.12) !important; }
            dt { color: #1a1a1a !important; }

            pre, code, kbd, .mw-highlight {
                background: rgba(0,0,0,0.05) !important;
                color: #2a2a2a !important;
                border: 1px solid rgba(0,0,0,0.1) !important;
            }

            table.wikitable { border: 1px solid rgba(0,0,0,0.15) !important; }
            table.wikitable th {
                background: rgba(0,0,0,0.06) !important;
                color: #1a1a1a !important;
            }
            table.wikitable td, table.wikitable th {
                border: 1px solid rgba(0,0,0,0.15) !important;
                color: #1a1a1a !important;
            }

            .archwiki-template-box,
            .archwiki-template-box-note,
            .archwiki-template-box-warning,
            .archwiki-template-box-tip {
                background: rgba(0,0,0,0.04) !important;
                border-left: 3px solid rgba(0,0,0,0.2) !important;
                color: #1a1a1a !important;
            }
            .archwiki-template-box-warning {
                border-left-color: rgba(200,50,20,0.7) !important;
                background: rgba(220,60,30,0.06) !important;
            }
            .archwiki-template-box-note {
                border-left-color: rgba(30,100,210,0.7) !important;
                background: rgba(30,100,210,0.06) !important;
            }
            .archwiki-template-box-tip {
                border-left-color: rgba(30,150,50,0.7) !important;
                background: rgba(30,150,50,0.06) !important;
            }
        }
    """
    def _on_wiki_fetched(self, html_or_none, fetch_id, wiki_url):
        if fetch_id != getattr(self, '_wiki_fetch_id', None):
            return
        self.detail_wiki_spinner.stop()
        self.detail_wiki_url = wiki_url
        if not html_or_none:
            self.detail_wiki_box.set_visible(False)
            return
        self.detail_wiki_box.set_visible(True)
        if WEBKIT_AVAILABLE:
            full_html = (
                "<!DOCTYPE html><html><head>"
                "<meta charset='utf-8'>"
                f"<style>{self._WIKI_CSS}</style>"
                "</head><body>"
                f"{html_or_none}"
                "</body></html>"
            )
            self.detail_webview.load_html(full_html, "https://wiki.archlinux.org/")
        else:
            clean = re.sub(r'<[^>]+>', '', html_or_none)
            clean = html_module.unescape(clean)
            clean = re.sub(r'\s+', ' ', clean).strip()
            self.detail_wiki_label.set_label(clean[:2000] if len(clean) > 2000 else clean)
    def _on_wiki_open_clicked(self, btn):
        if self.detail_wiki_url:
            Gtk.show_uri(self.get_root(), self.detail_wiki_url, Gdk.CURRENT_TIME)
    def on_detail_back_clicked(self, btn):
        self.list_detail_stack.set_transition_type(Gtk.StackTransitionType.SLIDE_RIGHT)
        self.list_detail_stack.set_visible_child_name("list")
    def on_detail_action_clicked(self, btn):
        pkg = self.detail_current_pkg
        pkg_dict = {
            'name': pkg.name, 'repo': pkg.repo, 'version': pkg.version,
            'installed': pkg.installed, 'desc': pkg.desc, 'is_aur': pkg.is_aur
        }
        if pkg.installed:
            self.initiate_remove(pkg.name)
        else:
            self.initiate_install(pkg_dict)
    def on_detail_change_version_clicked(self, btn):
        pkg = self.detail_current_pkg
        btn.set_sensitive(False)
        threading.Thread(
            target=self._fetch_available_versions,
            args=(pkg.name,),
            daemon=True
        ).start()
    def _fetch_available_versions(self, pkg_name):
        versions = []
        installed_ver = None
        # Get currently installed version
        try:
            result = subprocess.run(
                ["pacman", "-Q", pkg_name],
                capture_output=True, text=True,
                env={**os.environ, 'LC_ALL': 'C'}
            )
            if result.returncode == 0:
                parts = result.stdout.strip().split()
                if len(parts) >= 2:
                    installed_ver = parts[1]
        except Exception:
            pass
        # Get current repo version
        try:
            result = subprocess.run(
                ["pacman", "-Sl"],
                capture_output=True, text=True,
                env={**os.environ, 'LC_ALL': 'C'}
            )
            if result.returncode == 0:
                for line in result.stdout.strip().split('\n'):
                    parts = line.split()
                    if len(parts) >= 3 and parts[1] == pkg_name:
                        repo = parts[0]
                        ver = parts[2]
                        is_current = (ver == installed_ver)
                        versions.append({"source": repo, "ver": ver, "current": is_current, "url": None})
        except Exception:
            pass
        # Check local cache
        cache_dir = "/var/cache/pacman/pkg"
        seen_versions = {v["ver"] for v in versions}
        if os.path.isdir(cache_dir):
            try:
                pattern = re.compile(
                    rf'^{re.escape(pkg_name)}-(\d.+?)-(x86_64|any)\.pkg\.tar\.(zst|xz|gz)$'
                )
                for fname in os.listdir(cache_dir):
                    m = pattern.match(fname)
                    if m:
                        ver = m.group(1)
                        if ver not in seen_versions:
                            is_current = (ver == installed_ver)
                            versions.append({"source": _("cache"), "ver": ver, "current": is_current,
                                             "url": os.path.join(cache_dir, fname)})
                            seen_versions.add(ver)
            except Exception:
                pass
        # Fetch from Arch Linux Archive
        try:
            first_letter = pkg_name[0].lower()
            archive_url = f"https://archive.archlinux.org/packages/{first_letter}/{pkg_name}/"
            with urllib.request.urlopen(archive_url, timeout=10) as resp:
                html = resp.read().decode()
            pkg_pattern = re.compile(
                rf'href="({re.escape(pkg_name)}-([^"]+?)-(x86_64|any)\.pkg\.tar\.[a-z]+)"'
            )
            for match in pkg_pattern.finditer(html):
                fname = urllib.parse.unquote(match.group(1))
                ver = urllib.parse.unquote(match.group(2))
                if ver not in seen_versions:
                    is_current = (ver == installed_ver)
                    dl_url = archive_url + match.group(1)
                    versions.append({"source": _("archive"), "ver": ver, "current": is_current,
                                     "url": dl_url})
                    seen_versions.add(ver)
        except Exception:
            pass
        # Sort by version descending (newest first) using pacman's vercmp if available
        try:
            def ver_sort_key(v):
                result = subprocess.run(
                    ["vercmp", v["ver"], "0"],
                    capture_output=True, text=True
                )
                return v["ver"]
            versions.sort(key=lambda v: v["ver"], reverse=True)
        except Exception:
            pass
        GLib.idle_add(self._show_version_dialog, pkg_name, versions)
    def _show_version_dialog(self, pkg_name, versions):
        root = self.get_root() or self.window
        if not versions:
            dialog = Adw.MessageDialog(
                heading=_("No Versions Found"),
                body=_("No alternative versions found for {}.").format(pkg_name),
                transient_for=root
            )
            dialog.add_response("ok", _("OK"))
            translate_dialog(dialog)
            dialog.present()
            return
        dialog = Adw.MessageDialog(
            heading=_("Change Version"),
            body=_("Select a version for {}:").format(pkg_name),
            transient_for=root
        )
        dialog.add_response("cancel", _("Cancel"))
        listbox = Gtk.ListBox()
        listbox.set_selection_mode(Gtk.SelectionMode.SINGLE)
        listbox.add_css_class("boxed-list")
        for v in versions:
            row = Adw.ActionRow()
            row.set_title(v["ver"])
            row.set_subtitle(v["source"])
            if v["current"]:
                badge = Gtk.Label(label=_("Current"))
                badge.add_css_class("dim-label")
                badge.set_valign(Gtk.Align.CENTER)
                row.add_suffix(badge)
            row._version_info = v
            listbox.append(row)
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_min_content_height(150)
        scrolled.set_max_content_height(300)
        scrolled.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scrolled.set_child(listbox)
        dialog.set_extra_child(scrolled)
        dialog.add_response("install", _("Install Selected"))
        dialog.set_response_appearance("install", Adw.ResponseAppearance.SUGGESTED)
        def on_response(dlg, response):
            if response == "install":
                selected_row = listbox.get_selected_row()
                if selected_row:
                    v = selected_row._version_info
                    if v["url"] and v["url"].startswith("http"):
                        # Archive URL - install via URL
                        target = v["url"]
                    elif v["url"]:
                        # Local cache file path
                        target = v["url"]
                    else:
                        # Repo version
                        target = f"{v['source']}/{pkg_name}"
                    self.action_type = "install"
                    self.prompt_for_password(lambda: self._install_specific_version(pkg_name, target))
            dlg.close()
        dialog.connect("response", on_response)
        translate_dialog(dialog)
        self.detail_version_btn.set_sensitive(True)
        self.detail_install_older_btn.set_sensitive(True)
        dialog.present()
    def _install_specific_version(self, pkg_name, target):
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text("")
        self.btn_back.set_sensitive(False)
        self.process_in_progress = True
        self.current_package_name = pkg_name
        self.progress_bar.set_visible(True)
        self.revealer_details.set_reveal_child(False)
        self.btn_details.set_label(_("Show Details"))
        self.btn_cancel.set_visible(True)
        self.progress_title.set_text(_("Installing {}...").format(pkg_name))
        self.lbl_progress_status.set_text(_("Changing version..."))
        self.pulse_timer_id = GLib.timeout_add(100, self.pulse_progress)
        if target.startswith("http"):
            # Download from ALA to temp dir first, then install locally
            # to avoid PGP signature issues with expired keys
            threading.Thread(target=self._download_and_install, args=(pkg_name, target), daemon=True).start()
        else:
            sudo_wrap = sudo_manager.wrapper_path
            if target.startswith("/"):
                cmd = [sudo_wrap, "pacman", "-U", "--noconfirm", target]
            else:
                cmd = [sudo_wrap, "pacman", "-S", "--noconfirm", target]
            threading.Thread(target=self.execute_shell, args=(cmd, pkg_name), daemon=True).start()

    def _download_and_install(self, pkg_name, url):
        import tempfile
        tmp_dir = tempfile.mkdtemp(prefix="linpama_")
        filename = urllib.parse.unquote(url.rsplit("/", 1)[-1])
        local_path = os.path.join(tmp_dir, filename)
        try:
            GLib.idle_add(self.lbl_progress_status.set_text, _("Downloading package..."))
            urllib.request.urlretrieve(url, local_path)
        except Exception as e:
            GLib.idle_add(self._download_failed, pkg_name, str(e), tmp_dir)
            return
        sudo_wrap = sudo_manager.wrapper_path
        cmd = [sudo_wrap, "pacman", "-U", "--noconfirm", local_path]
        self.execute_shell(cmd, pkg_name)
        # Cleanup temp dir
        try:
            import shutil
            shutil.rmtree(tmp_dir, ignore_errors=True)
        except Exception:
            pass

    def _download_failed(self, pkg_name, error, tmp_dir):
        self.process_in_progress = False
        self.btn_back.set_sensitive(True)
        self.btn_cancel.set_visible(False)
        self.progress_bar.set_visible(False)
        if self.pulse_timer_id:
            GLib.source_remove(self.pulse_timer_id)
            self.pulse_timer_id = None
        self.progress_title.set_text(_("Download Failed"))
        self.lbl_progress_status.set_text(error)
        try:
            import shutil
            shutil.rmtree(tmp_dir, ignore_errors=True)
        except Exception:
            pass
    def on_listview_item_activate(self, listview, position):
        pkg = self.store.get_item(position)
        if pkg:
            self.show_package_detail(pkg)
    def setup_list_item(self, factory, list_item):
        row = Adw.ActionRow()
        icon = Gtk.Image()
        icon.set_pixel_size(32)
        row.add_prefix(icon)
        row.icon_widget = icon 
        ver_label = Gtk.Label()
        ver_label.add_css_class("dim-label")
        ver_label.set_margin_end(12)
        row.add_suffix(ver_label)
        row.ver_label = ver_label
        badge = Gtk.Label(label="AUR")
        badge.add_css_class("caption")
        badge.set_markup("<span background='#a40000' color='white' size='small'><b> AUR </b></span>")
        badge.set_margin_end(8)
        row.add_suffix(badge)
        row.badge = badge
        action_btn = Gtk.Button()
        action_btn.set_valign(Gtk.Align.CENTER)
        row.add_suffix(action_btn)
        row.action_btn = action_btn
        list_item.set_child(row)
    def bind_list_item(self, factory, list_item):
        row = list_item.get_child()
        pkg = list_item.get_item()
        row.set_title(pkg.name)
        row.set_subtitle(pkg.desc)
        icon_name = self.resolve_icon_name(pkg.name)
        row.icon_widget.set_from_icon_name(icon_name)
        ver_text = f"{pkg.version} ({pkg.repo})"
        row.ver_label.set_label(ver_text)
        row.badge.set_visible(pkg.is_aur)
        if pkg.installed:
            row.action_btn.set_label(_("Remove"))
            row.action_btn.remove_css_class("suggested-action")
            row.action_btn.add_css_class("destructive-action")
        else:
            row.action_btn.set_label(_("Install"))
            row.action_btn.remove_css_class("destructive-action")
            row.action_btn.add_css_class("suggested-action")
        row.action_btn.current_pkg = pkg
        if not getattr(row.action_btn, "connected", False):
            row.action_btn.connect("clicked", self.on_item_action_clicked)
            row.action_btn.connected = True
    def on_item_action_clicked(self, btn):
        pkg = btn.current_pkg
        pkg_dict = {
            'name': pkg.name,
            'repo': pkg.repo,
            'version': pkg.version,
            'installed': pkg.installed,
            'desc': pkg.desc,
            'is_aur': pkg.is_aur
        }
        if pkg.installed:
            self.initiate_remove(pkg.name)
        else:
            self.initiate_install(pkg_dict)
    def clear_results(self):
        self.displayed_count = 0
        self.store.remove_all()
    def initiate_install(self, pkg):
        self.action_type = "install"
        self.target_pkg = pkg
        if pkg.get('is_aur'):
            self.start_aur_review_process(pkg['name'])
        else:
            self.prompt_for_password(lambda: self.run_transaction(pkg['name']))
    def initiate_remove(self, package_name):
        self.action_type = "remove"
        self.prompt_for_password(lambda: self.run_transaction(package_name))
    def on_refresh_repos_clicked(self, btn):
        self.action_type = "refresh"
        self.prompt_for_password(lambda: self.run_repo_update())
    def run_repo_update(self):
        threading.Thread(target=self._update_repo_thread, daemon=True).start()
    def _update_repo_thread(self):
        self.repo_update_error = None
        if sudo_manager:
            sudo_manager.start_privileged_session()
        try:
             sudo_wrap = sudo_manager.wrapper_path
             update_cmd = [sudo_wrap, "pacman", "-Sy"]
             env = sudo_manager.get_env()
             env['LC_ALL'] = 'C'
             proc = subprocess.run(update_cmd, capture_output=True, text=True, env=env)
             if proc.returncode != 0:
                 err_msg = proc.stderr
                 if "Sorry, try again" in err_msg or "sudo: no password was provided" in err_msg:
                     self.repo_update_error = _("Incorrect sudo password. Please try again.")
                 else:
                     self.repo_update_error = _("Repository update failed")
                     print(f"Repo update stderr: {proc.stderr}")
        except Exception as e:
             self.repo_update_error = str(e)
             print(f"Repo update failed: {e}")
        
        if sudo_manager:
            sudo_manager.stop_privileged_session()
        GLib.idle_add(self._on_repo_update_finished)
    def _on_repo_update_finished(self):
        if sudo_manager:
            sudo_manager.forget_password()
        self.user_password = None
        if self.repo_update_error:
            dialog = Adw.MessageDialog(
                heading=_("Update Failed"),
                body=self.repo_update_error,
                transient_for=self.get_root() or self.window
            )
            dialog.add_response("ok", _("OK"))
            translate_dialog(dialog)
            dialog.present()
    def on_clear_cache_clicked(self, btn):
        self.action_type = "clear_cache"
        self.prompt_for_password(lambda: self.run_clear_cache())

    def run_clear_cache(self):
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text("")
        self.btn_back.set_sensitive(False)
        self.process_in_progress = True
        self.current_package_name = _("cache")
        self.progress_bar.set_visible(True)
        self.revealer_details.set_reveal_child(False)
        self.btn_details.set_label(_("Show Details"))
        self.btn_cancel.set_visible(True)
        self.progress_title.set_text(_("Clearing Package Cache..."))
        self.lbl_progress_status.set_text(_("Removing cached packages..."))
        self.pulse_timer_id = GLib.timeout_add(100, self.pulse_progress)
        sudo_wrap = sudo_manager.wrapper_path
        cmds = [[sudo_wrap, "pacman", "-Scc", "--noconfirm"]]
        paru_cmds = []
        if shutil.which("paru"):
            paru_cmds.append(["paru", "-Scc", "--noconfirm"])
        threading.Thread(target=self._run_sequential_cmds, args=(cmds, _("cache"), paru_cmds), daemon=True).start()

    def on_remove_orphans_clicked(self, btn):
        self.action_type = "remove_orphans"
        self.prompt_for_password(lambda: self.run_remove_orphans())

    def run_remove_orphans(self):
        threading.Thread(target=self._find_orphans_thread, daemon=True).start()

    def _find_orphans_thread(self):
        try:
            result = subprocess.run(["pacman", "-Qdtq"], capture_output=True, text=True)
            orphans = result.stdout.strip()
            GLib.idle_add(self._show_orphans_confirmation, orphans)
        except Exception as e:
            GLib.idle_add(self._show_orphans_confirmation, "")

    def _show_orphans_confirmation(self, orphans):
        root = self.get_root() or self.window
        if not orphans:
            dialog = Adw.MessageDialog(
                heading=_("No Orphans Found"),
                body=_("There are no orphan packages to remove."),
                transient_for=root
            )
            dialog.add_response("ok", _("OK"))
            translate_dialog(dialog)
            dialog.present()
            return
        orphan_list = orphans.split("\n")
        dialog = Adw.MessageDialog(
            heading=_("Remove Orphan Packages"),
            body=_("The following {} packages will be removed:").format(len(orphan_list)),
            transient_for=root
        )
        dialog.add_response("cancel", _("Cancel"))
        dialog.add_response("remove", _("Remove All"))
        dialog.set_response_appearance("remove", Adw.ResponseAppearance.DESTRUCTIVE)
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_min_content_height(150)
        scrolled.set_max_content_height(300)
        scrolled.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        pkg_list_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        pkg_list_box.set_margin_start(8)
        pkg_list_box.set_margin_end(8)
        for pkg in orphan_list:
            lbl = Gtk.Label(label=pkg)
            lbl.set_halign(Gtk.Align.START)
            lbl.add_css_class("monospace")
            pkg_list_box.append(lbl)
        scrolled.set_child(pkg_list_box)
        dialog.set_extra_child(scrolled)
        def on_response(dialog, response):
            if response == "remove":
                self._start_orphan_removal(orphan_list)
            dialog.close()
        dialog.connect("response", on_response)
        translate_dialog(dialog)
        dialog.present()

    def _start_orphan_removal(self, orphan_list):
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text("")
        self.btn_back.set_sensitive(False)
        self.process_in_progress = True
        self.current_package_name = _("orphans")
        self.progress_bar.set_visible(True)
        self.revealer_details.set_reveal_child(False)
        self.btn_details.set_label(_("Show Details"))
        self.btn_cancel.set_visible(True)
        self.progress_title.set_text(_("Removing Orphan Packages..."))
        self.lbl_progress_status.set_text(_("Removing orphans..."))
        self.pulse_timer_id = GLib.timeout_add(100, self.pulse_progress)
        threading.Thread(target=self._remove_orphans_thread, args=(orphan_list,), daemon=True).start()

    def _remove_orphans_thread(self, orphan_list):
        success = False
        if sudo_manager:
            sudo_manager.start_privileged_session()
        try:
            env = sudo_manager.get_env()
            env['LC_ALL'] = 'C'
            sudo_wrap = sudo_manager.wrapper_path
            cmd = [sudo_wrap, "pacman", "-Rns", "--noconfirm"] + orphan_list
            self.current_process = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                                                    stderr=subprocess.STDOUT, text=True, env=env)
            for line in iter(self.current_process.stdout.readline, ''):
                if line:
                    GLib.idle_add(self.append_log, line)
            self.current_process.stdout.close()
            rc = self.current_process.wait()
            success = (rc == 0)
        except Exception as e:
            GLib.idle_add(self.append_log, f"\nError: {e}")
            success = False
        GLib.idle_add(self.on_process_finished, success, _("orphans"))

    def on_remove_db_lock_clicked(self, btn):
        self.action_type = "remove_db_lock"
        self.prompt_for_password(lambda: self.run_remove_db_lock())

    def run_remove_db_lock(self):
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text("")
        self.btn_back.set_sensitive(False)
        self.process_in_progress = True
        self.current_package_name = _("db lock")
        self.progress_bar.set_visible(True)
        self.revealer_details.set_reveal_child(False)
        self.btn_details.set_label(_("Show Details"))
        self.btn_cancel.set_visible(False)
        self.progress_title.set_text(_("Removing Database Lock..."))
        self.lbl_progress_status.set_text(_("Removing /var/lib/pacman/db.lck..."))
        self.pulse_timer_id = GLib.timeout_add(100, self.pulse_progress)
        threading.Thread(target=self._remove_db_lock_thread, daemon=True).start()

    def _remove_db_lock_thread(self):
        success = False
        if sudo_manager:
            sudo_manager.start_privileged_session()
        try:
            sudo_wrap = sudo_manager.wrapper_path
            env = sudo_manager.get_env()
            env['LC_ALL'] = 'C'
            lock_path = "/var/lib/pacman/db.lck"
            if not os.path.exists(lock_path):
                GLib.idle_add(self.append_log, _("No database lock file found. Nothing to do.\n"))
                success = True
            else:
                proc = subprocess.run([sudo_wrap, "rm", "-f", lock_path],
                                      capture_output=True, text=True, env=env)
                if proc.returncode == 0:
                    GLib.idle_add(self.append_log, _("Database lock file removed successfully.\n"))
                    success = True
                else:
                    GLib.idle_add(self.append_log, proc.stderr)
                    success = False
        except Exception as e:
            GLib.idle_add(self.append_log, f"\nError: {e}")
            success = False
        GLib.idle_add(self.on_process_finished, success, _("db lock"))

    def _run_sequential_cmds(self, cmds, label, user_cmds=None):
        success = True
        if sudo_manager:
            sudo_manager.start_privileged_session()
        try:
            env = sudo_manager.get_env()
            env['LC_ALL'] = 'C'
            for cmd in cmds:
                self.current_process = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                                                        stderr=subprocess.STDOUT, text=True, env=env)
                for line in iter(self.current_process.stdout.readline, ''):
                    if line:
                        GLib.idle_add(self.append_log, line)
                self.current_process.stdout.close()
                rc = self.current_process.wait()
                if rc != 0:
                    success = False
        except Exception as e:
            GLib.idle_add(self.append_log, f"\nError: {e}")
            success = False
        if user_cmds:
            try:
                user_env = os.environ.copy()
                user_env['LC_ALL'] = 'C'
                for cmd in user_cmds:
                    self.current_process = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                                                            stderr=subprocess.STDOUT, text=True, env=user_env)
                    for line in iter(self.current_process.stdout.readline, ''):
                        if line:
                            GLib.idle_add(self.append_log, line)
                    self.current_process.stdout.close()
                    rc = self.current_process.wait()
                    if rc != 0:
                        success = False
            except Exception as e:
                GLib.idle_add(self.append_log, f"\nError: {e}")
                success = False
        GLib.idle_add(self.on_process_finished, success, label)

    def start_aur_review_process(self, package_name):
        self.aur_temp_dir = tempfile.mkdtemp(prefix=f"{APP_NAME}_aur_")
        self.aur_pkg_name = package_name
        self.current_package_name = package_name
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text(_("Fetching {} from AUR...").format(package_name))
        self.btn_back.set_sensitive(False)
        self.btn_cancel.set_visible(True)
        self.progress_bar.pulse()
        self.lbl_progress_status.set_text(_("Cloning repository..."))
        def clone_task():
            try:
                cmd = ["git", "clone", f"https://aur.archlinux.org/{package_name}.git", self.aur_temp_dir]
                self.current_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                self.current_process.wait()
                if self.current_process.returncode == 0:
                    pkgbuild_path = os.path.join(self.aur_temp_dir, "PKGBUILD")
                    if os.path.exists(pkgbuild_path):
                        with open(pkgbuild_path, 'r') as f:
                            content = f.read()
                        GLib.idle_add(self.show_pkgbuild_content, content)
                    else:
                        GLib.idle_add(self.show_aur_error, "PKGBUILD not found in cloned repository.")
                else:
                    GLib.idle_add(self.show_aur_error, "Clone failed or cancelled.")
            except Exception as e:
                GLib.idle_add(self.show_aur_error, f"Error: {e}")
        threading.Thread(target=clone_task, daemon=True).start()
    def show_pkgbuild_content(self, content):
        self.pkgbuild_buffer.set_text(content)
        self.content_stack.set_visible_child_name("pkgbuild_view")
        self.current_process = None
    def show_aur_error(self, message):
        self.output_buffer.set_text(message)
        self.btn_back.set_sensitive(True)
        self.btn_cancel.set_visible(False)
    def on_pkgbuild_cancel(self, btn):
        if hasattr(self, 'aur_temp_dir') and os.path.exists(self.aur_temp_dir):
            shutil.rmtree(self.aur_temp_dir)
        self.content_stack.set_visible_child_name("search_view")
    def on_pkgbuild_proceed(self, btn):
        self.prompt_for_password(lambda: self.run_aur_build())
    def run_aur_build(self):
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text("")
        self.progress_title.set_text(_("Building {}...").format(self.aur_pkg_name))
        self.progress_bar.set_visible(True)
        self.revealer_details.set_reveal_child(False)
        self.btn_details.set_label(_("Show Details"))
        self.lbl_progress_status.set_text(_("Compiling package..."))
        self.btn_cancel.set_visible(True)
        self.pulse_timer_id = GLib.timeout_add(100, self.pulse_progress)
        self.current_package_name = self.aur_pkg_name
        use_paru = shutil.which("paru") is not None
        if use_paru:
             cmd = ["paru", "-S", "--noconfirm", self.aur_pkg_name]
             cwd = None 
        else:
             cmd = ["makepkg", "-si", "--noconfirm"]
             cwd = self.aur_temp_dir
        env_extra = {
            'PACMAN_AUTH': sudo_manager.wrapper_path,
            'SUDO_ASKPASS': sudo_manager.askpass_script  
        }
        threading.Thread(target=self.execute_shell, args=(cmd, self.aur_pkg_name, cwd, env_extra), daemon=True).start()
    def prompt_for_password(self, callback_success):
        root = self.get_root() or self.window
        action_label = _("install") if getattr(self, "action_type", "install") == "install" else _("remove")
        action_type = getattr(self, "action_type", "install")
        if action_type == "refresh":
             body_text = _("Please enter your password to update repositories.")
        elif action_type == "clear_cache":
             body_text = _("Please enter your password to clear the package cache.")
        elif action_type == "remove_orphans":
             body_text = _("Please enter your password to remove orphan packages.")
        elif action_type == "remove_db_lock":
             body_text = _("Please enter your password to remove the database lock file.")
        else:
             body_text = _("Please enter your password to {} this package.").format(action_label)
        dialog = Adw.MessageDialog(
            heading=_("Authentication Required"),
            body=body_text,
            transient_for=root
        )
        dialog.add_response("cancel", _("Cancel"))
        dialog.add_response("unlock", _("Unlock"))
        dialog.set_response_appearance("unlock", Adw.ResponseAppearance.SUGGESTED)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        entry = Gtk.PasswordEntry()
        entry.set_property("placeholder-text", _("Password"))
        box.append(entry)
        dialog.set_extra_child(box)
        def on_response(dialog, response):
            if response == "unlock":
                pwd = entry.get_text()
                if pwd:
                    if sudo_manager.validate_password(pwd):
                        sudo_manager.set_password(pwd)
                        self.user_password = pwd
                        callback_success()
                    else:
                        err_dialog = Adw.MessageDialog(
                            heading=_("Authentication Failed"),
                            body=_("Incorrect password."),
                            transient_for=self.get_root() or self.window
                        )
                        err_dialog.add_response("ok", _("OK"))
                        translate_dialog(err_dialog)
                        err_dialog.present()
            dialog.close()
        dialog.connect("response", on_response)
        entry.connect("activate", lambda w: dialog.response("unlock"))
        translate_dialog(dialog)
        dialog.present()
    def run_transaction(self, package_name):
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text("")
        self.btn_back.set_sensitive(False)
        self.process_in_progress = True
        self.current_package_name = package_name
        self.progress_bar.set_visible(True)
        self.revealer_details.set_reveal_child(False)
        self.btn_details.set_label(_("Show Details"))
        self.btn_cancel.set_visible(True)
        self.pulse_timer_id = GLib.timeout_add(100, self.pulse_progress)
        sudo_wrap = sudo_manager.wrapper_path
        if self.action_type == "remove":
            self.progress_title.set_text(_("Removing {}...").format(package_name))
            self.lbl_progress_status.set_text(_("Removing package..."))
            cmd = [sudo_wrap, "pacman", "-Rns", "--noconfirm", package_name]
        else:
            self.progress_title.set_text(_("Installing {}...").format(package_name))
            self.lbl_progress_status.set_text(_("Downloading and installing..."))
            cmd = [sudo_wrap, "pacman", "-S", "--noconfirm", "--needed", package_name]
        threading.Thread(target=self.execute_shell, args=(cmd, package_name), daemon=True).start()
    def pulse_progress(self):
        self.progress_bar.pulse()
        return True
    def execute_shell(self, command, pkg_name, cwd=None, env_extra=None):
        success = False
        if sudo_manager:
            sudo_manager.start_privileged_session()
        try:
            env = sudo_manager.get_env()
            env['LC_ALL'] = 'C'
            if self.user_password:
                pass
            if env_extra:
                env.update(env_extra)
            self.current_process = subprocess.Popen(command, 
                                     stdout=subprocess.PIPE, 
                                     stderr=subprocess.STDOUT, 
                                     text=True, env=env, cwd=cwd)
            for line in iter(self.current_process.stdout.readline, ''):
                if line:
                    GLib.idle_add(self.append_log, line)
            self.current_process.stdout.close()
            rc = self.current_process.wait()
            success = (rc == 0)
        except Exception as e:
            GLib.idle_add(self.append_log, f"\nError: {e}")
            success = False
        GLib.idle_add(self.on_process_finished, success, pkg_name)
    def append_log(self, text):
        end = self.output_buffer.get_end_iter()
        self.output_buffer.insert(end, text)
        return False
    def on_process_finished(self, success, pkg_name):
        if sudo_manager:
            sudo_manager.stop_privileged_session()
            sudo_manager.forget_password()
        self.user_password = None
        self.process_in_progress = False
        self.current_process = None
        if self.pulse_timer_id:
            GLib.source_remove(self.pulse_timer_id)
            self.pulse_timer_id = None
        if hasattr(self, 'aur_temp_dir') and os.path.exists(self.aur_temp_dir):
            try:
                shutil.rmtree(self.aur_temp_dir)
            except: pass
        if success:
            self.info_icon.set_from_icon_name("object-select-symbolic")
            if self.action_type == "remove":
                 self.info_text.set_text(_("Successfully removed {}").format(pkg_name))
            elif self.action_type == "clear_cache":
                 self.info_text.set_text(_("Package cache cleared successfully."))
            elif self.action_type == "remove_orphans":
                 self.info_text.set_text(_("Orphan packages removed successfully."))
            elif self.action_type == "remove_db_lock":
                 self.info_text.set_text(_("Database lock removed successfully."))
            else:
                 self.info_text.set_text(_("Successfully installed {}").format(pkg_name))
            self.content_stack.set_visible_child_name("info_view")
            self.search_entry.set_text("")
        else:
            self.revealer_details.set_reveal_child(True)
            self.btn_details.set_label(_("Hide Details"))
            start, end = self.output_buffer.get_bounds()
            log_content = self.output_buffer.get_text(start, end, True)
            status_msg = _("Failed or Cancelled.")
            if "Sorry, try again" in log_content or "incorrect password" in log_content.lower() or "sudo: no password was provided" in log_content.lower():
                status_msg = _("Incorrect sudo password. Please try again.")
            self.lbl_progress_status.set_text(status_msg)
            self.btn_back.set_sensitive(True)
            self.btn_cancel.set_visible(False)
            self.append_log(f"\n\n{_('Transaction Failed.')}")
        return False
    def on_back_clicked(self, btn):
        self.content_stack.set_visible_child_name("search_view")

    def _on_sidebar_btn_toggled(self, btn):
        active = btn.get_active()
        if self.wide_layout_enabled:
            self.right_revealer.set_reveal_child(active)
        else:
            self.compact_panel_revealer.set_reveal_child(active)

    def _on_compact_panel_close(self, btn):
        self.compact_sidebar_btn.set_active(False)

    def setup_right_pane(self):
        self.right_pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.right_pane.set_hexpand(False)
        self.right_pane.set_vexpand(True)
        self.right_pane.set_valign(Gtk.Align.FILL)
        self.right_pane.set_margin_end(16)
        top_actions_group = Adw.PreferencesGroup()
        top_actions_group.set_title(_("Actions"))

        row_refresh = Adw.ActionRow()
        row_refresh.set_title(_("Refresh Repositories"))
        row_refresh.set_subtitle(_("Sync package databases"))
        row_refresh.add_prefix(Gtk.Image.new_from_icon_name("view-refresh-symbolic"))
        row_refresh.set_activatable(True)
        row_refresh.connect("activated", self.on_refresh_repos_clicked)
        top_actions_group.add(row_refresh)

        self.right_pane.append(top_actions_group)

        right_pane_spacer = Gtk.Box()
        right_pane_spacer.set_vexpand(True)
        self.right_pane.append(right_pane_spacer)

        bottom_actions_group = Adw.PreferencesGroup()

        row_cache = Adw.ActionRow()
        row_cache.set_title(_("Clear Package Cache"))
        row_cache.set_subtitle(_("Free disk space from cached packages"))
        row_cache.add_prefix(Gtk.Image.new_from_icon_name("user-trash-symbolic"))
        row_cache.set_activatable(True)
        row_cache.connect("activated", self.on_clear_cache_clicked)
        bottom_actions_group.add(row_cache)

        row_orphans = Adw.ActionRow()
        row_orphans.set_title(_("Remove Orphans"))
        row_orphans.set_subtitle(_("Remove unused dependency packages"))
        row_orphans.add_prefix(Gtk.Image.new_from_icon_name("edit-clear-all-symbolic"))
        row_orphans.set_activatable(True)
        row_orphans.connect("activated", self.on_remove_orphans_clicked)
        bottom_actions_group.add(row_orphans)

        row_db_lock = Adw.ActionRow()
        row_db_lock.set_title(_("Remove DB Lock"))
        row_db_lock.set_subtitle(_("Delete /var/lib/pacman/db.lck"))
        row_db_lock.add_prefix(Gtk.Image.new_from_icon_name("channel-insecure-symbolic"))
        row_db_lock.set_activatable(True)
        row_db_lock.connect("activated", self.on_remove_db_lock_clicked)
        bottom_actions_group.add(row_db_lock)

        self.right_pane.append(bottom_actions_group)
        # Compact overlay panel (slides from right on top of content)
        self.compact_panel_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.compact_panel_box.set_hexpand(False)
        self.compact_panel_box.set_vexpand(True)
        self.compact_panel_box.set_valign(Gtk.Align.FILL)
        self.compact_panel_box.set_halign(Gtk.Align.END)
        self.compact_panel_box.set_size_request(RIGHT_PANE_MIN_WIDTH, -1)
        self.compact_panel_box.add_css_class("background")
        compact_header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        compact_header.set_halign(Gtk.Align.END)
        compact_header.set_margin_top(4)
        compact_header.set_margin_end(4)
        compact_close_btn = Gtk.Button.new_from_icon_name("window-close-symbolic")
        compact_close_btn.add_css_class("flat")
        compact_close_btn.add_css_class("circular")
        compact_close_btn.set_tooltip_text(_("Close"))
        compact_close_btn.connect("clicked", self._on_compact_panel_close)
        compact_header.append(compact_close_btn)
        self.compact_panel_box.append(compact_header)
        compact_center = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        compact_center.set_valign(Gtk.Align.FILL)
        compact_center.set_halign(Gtk.Align.CENTER)
        compact_center.set_vexpand(True)
        compact_center.set_margin_start(WIDE_LAYOUT_SIDE_PADDING)
        compact_center.set_margin_end(WIDE_LAYOUT_SIDE_PADDING)
        compact_top_actions_group = Adw.PreferencesGroup()
        compact_top_actions_group.set_title(_("Actions"))

        compact_row_refresh = Adw.ActionRow()
        compact_row_refresh.set_title(_("Refresh Repositories"))
        compact_row_refresh.set_subtitle(_("Sync package databases"))
        compact_row_refresh.add_prefix(Gtk.Image.new_from_icon_name("view-refresh-symbolic"))
        compact_row_refresh.set_activatable(True)
        compact_row_refresh.connect("activated", self.on_refresh_repos_clicked)
        compact_top_actions_group.add(compact_row_refresh)

        compact_center.append(compact_top_actions_group)

        compact_pane_spacer = Gtk.Box()
        compact_pane_spacer.set_vexpand(True)
        compact_center.append(compact_pane_spacer)

        compact_bottom_actions_group = Adw.PreferencesGroup()

        compact_row_cache = Adw.ActionRow()
        compact_row_cache.set_title(_("Clear Package Cache"))
        compact_row_cache.set_subtitle(_("Free disk space from cached packages"))
        compact_row_cache.add_prefix(Gtk.Image.new_from_icon_name("user-trash-symbolic"))
        compact_row_cache.set_activatable(True)
        compact_row_cache.connect("activated", self.on_clear_cache_clicked)
        compact_bottom_actions_group.add(compact_row_cache)

        compact_row_orphans = Adw.ActionRow()
        compact_row_orphans.set_title(_("Remove Orphans"))
        compact_row_orphans.set_subtitle(_("Remove unused dependency packages"))
        compact_row_orphans.add_prefix(Gtk.Image.new_from_icon_name("edit-clear-all-symbolic"))
        compact_row_orphans.set_activatable(True)
        compact_row_orphans.connect("activated", self.on_remove_orphans_clicked)
        compact_bottom_actions_group.add(compact_row_orphans)

        compact_row_db_lock = Adw.ActionRow()
        compact_row_db_lock.set_title(_("Remove DB Lock"))
        compact_row_db_lock.set_subtitle(_("Delete /var/lib/pacman/db.lck"))
        compact_row_db_lock.add_prefix(Gtk.Image.new_from_icon_name("channel-insecure-symbolic"))
        compact_row_db_lock.set_activatable(True)
        compact_row_db_lock.connect("activated", self.on_remove_db_lock_clicked)
        compact_bottom_actions_group.add(compact_row_db_lock)

        compact_center.append(compact_bottom_actions_group)

        self.compact_panel_box.append(compact_center)
        self.compact_panel_revealer = Gtk.Revealer()
        self.compact_panel_revealer.set_transition_type(Gtk.RevealerTransitionType.SLIDE_LEFT)
        self.compact_panel_revealer.set_transition_duration(250)
        self.compact_panel_revealer.set_reveal_child(False)
        self.compact_panel_revealer.set_halign(Gtk.Align.END)
        self.compact_panel_revealer.set_vexpand(True)
        self.compact_panel_revealer.set_child(self.compact_panel_box)
        self.compact_overlay = Gtk.Overlay()
        self.compact_overlay.set_hexpand(True)
        self.compact_overlay.set_vexpand(True)
        self.compact_overlay.add_overlay(self.compact_panel_revealer)
        # Build the permanent widget tree (no reparenting ever)
        self.right_revealer.set_child(self.right_pane)
        self.content_hbox.append(self.content_stack)
        self.content_hbox.append(self.right_revealer)
        self.compact_overlay.set_child(self.content_hbox)
        self.main_layout_box.append(self.compact_overlay)
        self.top_stack.add_named(self.main_layout_box, "main_view")

    def get_right_pane_min_width(self):
        return RIGHT_PANE_MIN_WIDTH

    def _monitor_adaptive_layout(self):
        width = self.get_width()
        if width <= 0 and self.window:
            width = self.window.get_width()
        if width > 0 and width != self.last_measured_width:
            self.last_measured_width = width
            self.update_adaptive_layout(current_width=width)
        return True

    def update_adaptive_layout(self, force=False, current_width=None):
        width = current_width if current_width is not None else self.get_width()
        if width <= 0 and self.window:
            width = self.window.get_width()
        use_wide_layout = width > WIDE_LAYOUT_THRESHOLD if width > 0 else False
        if not force and use_wide_layout == self.wide_layout_enabled:
            return False
        self.wide_layout_enabled = use_wide_layout
        if use_wide_layout:
            self.set_margin_start(0)
            self.set_margin_end(0)
            self.content_stack.set_margin_start(12)
            self.content_stack.set_margin_end(0)
            self.compact_panel_revealer.set_reveal_child(False)
            self.compact_sidebar_btn.handler_block(self._sidebar_toggled_handler)
            self.compact_sidebar_btn.set_active(True)
            self.compact_sidebar_btn.handler_unblock(self._sidebar_toggled_handler)
            self.right_revealer.set_reveal_child(True)
        else:
            self.set_margin_start(12)
            self.set_margin_end(12)
            self.content_stack.set_margin_start(0)
            self.content_stack.set_margin_end(0)
            self.right_revealer.set_reveal_child(False)
            self.compact_panel_revealer.set_reveal_child(False)
            self.compact_sidebar_btn.handler_block(self._sidebar_toggled_handler)
            self.compact_sidebar_btn.set_active(False)
            self.compact_sidebar_btn.handler_unblock(self._sidebar_toggled_handler)
        return False
