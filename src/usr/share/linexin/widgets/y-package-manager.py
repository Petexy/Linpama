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

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Gtk, Adw, GLib, Pango, Gdk

APP_NAME = "linpama"
LOCALE_DIR = os.path.abspath("/usr/share/locale")
CONFIG_DIR = os.path.expanduser(f"~/.config/{APP_NAME}")
CONFIG_FILE = os.path.join(CONFIG_DIR, "config.json")

locale.setlocale(locale.LC_ALL, '')
locale.bindtextdomain(APP_NAME, LOCALE_DIR)
gettext.bindtextdomain(APP_NAME, LOCALE_DIR)
gettext.textdomain(APP_NAME)
_ = gettext.gettext

class LinexinPackageManager(Gtk.Box):
    def __init__(self, window=None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        
        self.widgetname = "Package Manager"
        self.widgeticon = "/usr/share/icons/github.petexy.linpama.svg" 
        
        self.set_margin_top(12)
        self.set_margin_bottom(50)
        self.set_margin_start(12)
        self.set_margin_end(12)
        
        self.window = window
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
        self.displayed_count = 0
        self.batch_size = 30
        
        self.available_flatpak_ids = []
        self.setup_appstream_icon_paths()
        threading.Thread(target=self.load_all_flatpak_ids, daemon=True).start()
        
        self.askpass_script = f"/tmp/{APP_NAME}-askpass.sh"
        self.sudo_wrapper = f"/tmp/{APP_NAME}-sudo.sh"
        
        self.content_stack = Gtk.Stack()
        self.content_stack.set_transition_type(Gtk.StackTransitionType.SLIDE_LEFT_RIGHT)
        self.content_stack.set_hexpand(True)
        self.content_stack.set_vexpand(True)
        self.append(self.content_stack)
        
        self.setup_warning_view()
        self.setup_search_view()
        self.setup_pkgbuild_view()
        self.setup_progress_view()
        self.setup_info_view()
        
        if self.should_show_warning():
            self.content_stack.set_visible_child_name("warning_view")
        else:
            self.content_stack.set_visible_child_name("search_view")

    def setup_custom_styles(self):
        css_provider = Gtk.CssProvider()
        css_provider.load_from_data(b"""
            .buttons_all {
                font-size: 14px;
                min-width: 200px;
                min-height: 40px;
            }
        """)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), 
            css_provider, 
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

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
            store_btn.connect("clicked", lambda x: subprocess.Popen(cmd))
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
        self.content_stack.add_named(warn_box, "warning_view")

    def on_warning_continue(self, btn):
        if self.dont_show_check.get_active():
            self.save_warning_preference(False)
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
        header_box.append(self.aur_check)
        
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
        
        spinner = Gtk.Spinner()
        spinner.set_size_request(48, 48)
        spinner.start()
        loading_box.append(spinner)
        
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

        self.results_scrolled = Gtk.ScrolledWindow()
        self.results_scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        self.results_scrolled.set_vexpand(True)
        self.results_scrolled.connect("edge-reached", self.on_scroll_edge_reached)
        
        self.results_listbox = Gtk.ListBox()
        self.results_listbox.add_css_class("boxed-list")
        self.results_listbox.set_selection_mode(Gtk.SelectionMode.NONE)
        
        self.results_scrolled.set_child(self.results_listbox)
        self.search_stack.add_named(self.results_scrolled, "results")
        
        search_box.append(self.search_stack)
        
        self.search_stack.set_visible_child_name("welcome")
        
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
            res = subprocess.run(cmd, capture_output=True, text=True, env={'LC_ALL': 'C'})
            
            if res.returncode == 0:
                self.available_flatpak_ids = [line.strip() for line in res.stdout.split('\n') if line.strip()]
        except Exception:
            pass

    def resolve_icon_name(self, package_name):
        icon_theme = Gtk.IconTheme.get_for_display(Gdk.Display.get_default())
        
        if icon_theme.has_icon(package_name):
            return package_name
            
        clean_name = re.sub(r'^(-bin|-git|-nightly|-stable|-beta)|(-bin|-git|-nightly|-stable|-beta)$', '', package_name)
        if clean_name != package_name and icon_theme.has_icon(clean_name):
            return clean_name
            
        pkg_lower = clean_name.lower()
        
        matches = []
        for fid in self.available_flatpak_ids:
            fid_lower = fid.lower()
            if fid_lower == pkg_lower or fid_lower.endswith(f".{pkg_lower}"):
                matches.append(fid)

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
        
        query = entry.get_text().strip()
        self.search_counter += 1
        current_search_id = self.search_counter
        
        if len(query) < 2:
            self.clear_results()
            self.search_stack.set_visible_child_name("welcome")
            return

        self.search_timer = GLib.timeout_add(400, self.trigger_search, query, current_search_id)

    def trigger_search(self, query, search_id):
        self.search_stack.set_visible_child_name("loading")
        self.search_in_progress = True
        self.all_search_results = []
        self.displayed_count = 0
        
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
            cmd = ["pacman", "-Ss", query]
            process = subprocess.run(cmd, capture_output=True, text=True, env={'LC_ALL': 'C'})
            
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
                        installed = "[installed]" in line
                        
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

        if self.aur_check.get_active():
            try:
                rpc_url = f"https://aur.archlinux.org/rpc/?v=5&type=search&arg={urllib.parse.quote(query)}"
                with urllib.request.urlopen(rpc_url, timeout=5) as response:
                    data = json.loads(response.read().decode())
                    
                    if search_id != self.search_counter: return
                    
                    if data.get("type") != "error" and data.get("results"):
                        for item in data["results"]:
                            if query.lower() in item['Name'].lower():
                                results.append({
                                    'name': item['Name'],
                                    'repo': 'AUR',
                                    'version': item['Version'],
                                    'installed': False,
                                    'desc': item.get('Description', ''),
                                    'is_aur': True
                                })
            except Exception as e:
                print(f"AUR search error: {e}")

        if search_id == self.search_counter:
            self.all_search_results = results
            GLib.idle_add(self.update_results_initial)

    def update_results_initial(self):
        self.search_in_progress = False
        self.clear_results()
        
        if not self.all_search_results:
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
        
        for pkg in batch:
            row = self.create_package_row(pkg)
            self.results_listbox.append(row)
        
        self.displayed_count = end_idx

    def clear_results(self):
        self.displayed_count = 0
        child = self.results_listbox.get_first_child()
        while child:
            next_child = child.get_next_sibling()
            self.results_listbox.remove(child)
            child = next_child

    def create_package_row(self, pkg):
        row = Adw.ActionRow()
        row.set_title(pkg['name'])
        row.set_subtitle(pkg['desc'])
        
        icon_name = self.resolve_icon_name(pkg['name'])
        icon_image = Gtk.Image.new_from_icon_name(icon_name)
        icon_image.set_pixel_size(32)
        row.add_prefix(icon_image)
        
        ver_text = f"{pkg['version']} ({pkg['repo']})"
        ver_label = Gtk.Label(label=ver_text)
        ver_label.add_css_class("dim-label")
        ver_label.set_margin_end(12)
        row.add_suffix(ver_label)
        
        if pkg.get('is_aur'):
            badge = Gtk.Label(label="AUR")
            badge.add_css_class("caption")
            badge.set_markup("<span background='#a40000' color='white' size='small'><b> AUR </b></span>")
            badge.set_margin_end(8)
            row.add_suffix(badge)

        action_btn = Gtk.Button()
        action_btn.set_valign(Gtk.Align.CENTER)
        
        if pkg['installed']:
            action_btn.set_label(_("Remove"))
            action_btn.add_css_class("destructive-action")
            action_btn.connect("clicked", lambda b, n=pkg['name']: self.initiate_remove(n))
        else:
            action_btn.set_label(_("Install"))
            action_btn.add_css_class("suggested-action")
            action_btn.connect("clicked", lambda b, p=pkg: self.initiate_install(p))
            
        row.add_suffix(action_btn)
        return row

    def initiate_install(self, pkg):
        self.action_type = "install"
        self.target_pkg = pkg
        
        if pkg.get('is_aur'):
            self.start_aur_review_process(pkg['name'])
        else:
            if not self.user_password:
                self.prompt_for_password(lambda: self.run_transaction(pkg['name']))
            else:
                self.run_transaction(pkg['name'])

    def initiate_remove(self, package_name):
        self.action_type = "remove"
        if not self.user_password:
            self.prompt_for_password(lambda: self.run_transaction(package_name))
        else:
            self.run_transaction(package_name)

    def start_aur_review_process(self, package_name):
        self.aur_temp_dir = tempfile.mkdtemp(prefix=f"{APP_NAME}_aur_")
        self.aur_pkg_name = package_name
        self.current_package_name = package_name
        
        self.content_stack.set_visible_child_name("progress_view")
        self.output_buffer.set_text(f"Fetching {package_name} from AUR...")
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
        if not self.user_password:
             self.prompt_for_password(lambda: self.run_aur_build())
        else:
             self.run_aur_build()

    def run_aur_build(self):
        self.setup_sudo_env()
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
        
        cmd = f"cd {self.aur_temp_dir} && export SUDO_ASKPASS='{self.askpass_script}' && makepkg -si --noconfirm"
        
        threading.Thread(target=self.execute_shell, args=(cmd, self.aur_pkg_name), daemon=True).start()

    def prompt_for_password(self, callback_success):
        root = self.get_root() or self.window
        
        action_label = _("install") if getattr(self, "action_type", "install") == "install" else _("remove")
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
                    self.user_password = pwd
                    callback_success()
            dialog.close()
            
        dialog.connect("response", on_response)
        entry.connect("activate", lambda w: dialog.response("unlock"))
        dialog.present()

    def setup_sudo_env(self):
        with open(self.askpass_script, "w") as f:
            f.write(f"#!/bin/sh\necho \"${APP_NAME.upper().replace('-', '_')}_SUDO_PW\"\n")
        os.chmod(self.askpass_script, 0o700)
        
        with open(self.sudo_wrapper, "w") as f:
            f.write(f"#!/bin/sh\nexport SUDO_ASKPASS='{self.askpass_script}'\nexec sudo -A \"$@\"\n")
        os.chmod(self.sudo_wrapper, 0o700)

    def run_transaction(self, package_name):
        self.setup_sudo_env()
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
        
        if self.action_type == "remove":
            self.progress_title.set_text(_("Removing {}...").format(package_name))
            self.lbl_progress_status.set_text(_("Removing package..."))
            cmd = f"{self.sudo_wrapper} pacman -Rns --noconfirm {package_name}"
        else:
            self.progress_title.set_text(_("Installing {}...").format(package_name))
            self.lbl_progress_status.set_text(_("Downloading and installing..."))
            cmd = f"{self.sudo_wrapper} pacman -S --noconfirm --needed {package_name}"
        
        threading.Thread(target=self.execute_shell, args=(cmd, package_name), daemon=True).start()

    def pulse_progress(self):
        self.progress_bar.pulse()
        return True

    def execute_shell(self, command, pkg_name):
        success = False
        try:
            env = os.environ.copy()
            if self.user_password:
                env_var_name = f"{APP_NAME.upper().replace('-', '_')}_SUDO_PW"
                env[env_var_name] = self.user_password
            
            self.current_process = subprocess.Popen(command, shell=True, 
                                     stdout=subprocess.PIPE, 
                                     stderr=subprocess.STDOUT, 
                                     text=True, env=env)
            
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
            self.info_icon.set_from_icon_name("emblem-ok")
            
            if self.action_type == "remove":
                 self.info_text.set_text(_("Successfully removed {}").format(pkg_name))
            else:
                 self.info_text.set_text(_("Successfully installed {}").format(pkg_name))
                 
            self.content_stack.set_visible_child_name("info_view")
            self.search_entry.set_text("") 
            self.clear_results()
        else:
            self.revealer_details.set_reveal_child(True)
            self.btn_details.set_label(_("Hide Details"))
            self.lbl_progress_status.set_text(_("Failed or Cancelled."))
            self.btn_back.set_sensitive(True)
            self.btn_cancel.set_visible(False)
            self.append_log(f"\n\n{_('Transaction Failed.')}")
            
        return False

    def on_back_clicked(self, btn):
        self.content_stack.set_visible_child_name("search_view")