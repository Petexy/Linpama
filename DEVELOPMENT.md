# Developing Linpama (Package Manager)

How this repository is built, tested and put together. What it is and how
to install and use it: [README.md](README.md).

The Package Manager widget of Linexin Center (linpama), rewritten in Rust as a
native widget (`y-package_manager.so`) for the
[Rust Linexin Center](https://github.com/Petexy/Linexin-Center). It replaces the Python
`y-package_manager.py`.

It has two faces sharing what is asked of pacman, the AUR, the Arch Wiki and
the Arch Linux Archive (`widget/pacman.rs`, `http.rs`, `wiki.rs`, `runner.rs`):

- **GTK** (every desktop but LineXinBar, and LineXinBar without lxb-toolkit):
  identical to the Python widget — the stability warning (with GNOME Software or
  Discover offered instead), the package search with the AUR, the installed
  packages listed when the search is empty, a package's page with its Arch Wiki
  article (WebKit), Change Version / Install Older Version (repositories, the
  package cache and the Arch Linux Archive), building AUR packages after
  reviewing their PKGBUILD, the progress page with its log and Cancel, and the
  actions pane (refresh the repositories, clear the cache, remove orphans,
  remove the database lock) beside the list or, below 900 px, sliding over it.
  Privileged commands go through the Center's password prompt and sudo helper
  (the Center is locked while they run).
- **LineXinBar**: the same in LineXinBar's own language, driven by
  controller, keyboard and pointer: the stability warning (Software Hub is
  offered instead of the system's packages there), a search typed into a
  field (LineXinBar's on-screen keyboard rises over it with a controller in
  hand) with the AUR switch, the results with their own icons, descriptions,
  versions and whether they are installed (Options installs or removes one), a
  package's page (install, remove, change the version or install an older one
  from the cache or the Arch Linux Archive, its Arch Wiki article as text and
  a row that opens it), an AUR package's PKGBUILD to read before it is built
  (paru, or makepkg), the four actions, and every command's progress with
  Cancel, its result and its whole log. Privileged commands go through
  `pkexec`, so the password is asked by the session's polkit agent (on
  LineXinBar, the shell's own panel with its on-screen keyboard). Typing needs
  an lxb-toolkit that lets a page take text (`lxb_page_taking_text`,
  lxb-toolkit 0.9.4); with an older one the page lists the installed packages where
  the search would be.

Web requests use `curl` (the Python widget used urllib); failures are still
worded like urllib's.

## Build

The crate takes the Linexin Center's crates (`linexin-plugin-api`, `linexin-lxb`, `linexin-localization`)
from [its repository](https://github.com/Petexy/Linexin-Center), so a clone builds on its own. To build
against a local checkout of the Center instead, to change both at once, say so
in `.cargo/config.toml` (not committed):

```toml
[patch."https://github.com/Petexy/Linexin-Center"]
linexin-plugin-api = { path = "/path/to/Linexin-Center/crates/plugin-api" }
linexin-lxb = { path = "/path/to/Linexin-Center/crates/lxb" }
linexin-localization = { path = "/path/to/Linexin-Center/crates/localization" }
```

```sh
cargo build --release --lib
makepkg -si                 # the linpama package
```

## Install layout

```text
/usr/share/linexin/widgets/y-package_manager.so
/usr/share/linexin/widgets/localization/<lang>/package_manager_dictionary.py
/usr/share/icons/github.petexy.linpama.svg
```

`data/` holds every file of the Python package except the widget itself. The
stability warning's choice is kept in `~/.config/linpama/config.json`, as
before.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --lib --examples && examples/compare.sh
```

`examples/compare.sh` plays the same steps (typing a search, opening a
package, clicking, answering dialogs) on this widget (`examples/harness.rs`)
and on the original Python widget (`examples/python_reference.py`) in a private
headless KWin, and compares every pair of renders pixel by pixel, with the
lines both print, the commands both ran and the web addresses both read:
English and Polish, dark mode, the warning, the installed list, searches with
and without the AUR, a package's page with and without a wiki article,
installs and removals that succeed or fail, a wrong password, changing
versions, the four actions (orphans found or not) and building from the AUR.
Nothing is installed and nothing leaves the machine: pacman, paru, git,
flatpak, curl and the sudo wrapper are fakes first on `PATH`, answering from
fixtures, and the Python widget's urllib reads the same fixtures.

With `LXB_LIBRARY_DIR` naming a folder whose `liblxb_app.so` has
`lxb_app_shot_after` and lets a page take text, the same script plays the
LineXinBar page as well (`examples/lxb_run.rs`, what is typed scripted with
`LINEXIN_LXB_TYPED`): searches typed and rubbed out, the AUR searched and a
package built after its PKGBUILD, installs, removals, a version from the
archive, a package's wiki article, the four actions, and the warning turned
off; a fake `pkexec` runs the fake pacman, and what was run is checked.

`LINPAMA_LXB_SHOT_DIR=DIR cargo test --lib page_shots` draws the LineXinBar
page to `DIR/idle.png` and `DIR/working.png` (needs `liblxb_app.so`).

The comparison needs the original Python widget, installed where its package
puts it or named with `PYTHON_WIDGET=FILE`; it finds this checkout's build
wherever cargo put it (`CARGO_TARGET_DIR` counts), and the Center's stylesheet
comes with the Center's crates.

## Differences from the original

- The password is checked off the GTK thread (the original blocked the
  window while sudo answered).
- Starting a second progress page no longer leaves the first one's pulse
  timer running (the bar pulsed twice as fast).
- The dictionaries gained "Installed" and "Not installed" (a package's
  status, which the original showed in English in every language).
