# Linpama

<p align="center">
  <img src="https://i.ibb.co/V0rSgx74/github-petexy-linpama.png" alt="Linpama" width="200" height="200"/>
</p>

**Linpama** is a modern, lightweight graphical package manager built for
Linexin, also working on any Arch-based Linux. It is a widget for
[Linexin Center](https://github.com/Petexy/Linexin-Center) that gives one
interface for system packages through `pacman` and community packages from the
**AUR** (Arch User Repository).

It focuses on safe and stable package management habits while providing
powerful tools for advanced users.

This version is a rewrite in **Rust** (GTK4 + Libadwaita) and a drop-in
replacement for the Python Linpama: same package, same place in Linexin Center,
same look.

## ✨ Features

* **Unified Search:** Search your repositories and the AUR at the same time.
  With an empty search, your installed packages are listed.
* **Modern UI:** Native GNOME look and feel using GTK4 and Libadwaita.
* **AUR Integration:**
    * Search and install AUR packages.
    * **Security First:** Review a package's **PKGBUILD** before it is built,
      to check for malicious code.
    * Building and dependency handling through `paru` (or `makepkg`).
* **Package Pages:** Every package has its own page with its details and its
  **Arch Wiki** article.
* **Change Version:** Install an older version of a package from your package
  cache or the Arch Linux Archive.
* **Maintenance Tools:** Refresh the repositories, clear the package cache,
  remove orphaned packages and remove a stale database lock.
* **System Stability Warning:** A first-run screen explains the risks of
  installing apps as system packages and suggests Flatpaks (through your
  software center) for GUI applications.
* **Transaction Transparency:** Follow every install and removal with its full,
  live log, and cancel it if you need to.
* **Smart Icons:** Shows package icons from your icon theme and AppStream data.
* **LineXinBar Ready:** On a LineXinBar desktop Linpama is a controller-friendly
  page with a search field for the on-screen keyboard; passwords are asked
  through the desktop's own prompt.

## 📦 Installation

**On Linexin** Linpama is already installed. Open **Package Manager** from
Linexin Center.

**On other Arch-based distros** install [Linexin Center](https://github.com/Petexy/Linexin-Center)
first, then build and install Linpama:

```bash
sudo pacman -S --needed base-devel git
git clone https://github.com/Petexy/Linpama.git
cd Linpama
makepkg -si
```

To open it on its own:

```bash
linexin-center -w y-package_manager
```

## 📦 Prerequisites

Linpama is designed for Arch Linux and its derivatives. The package pulls in:

* Linexin Center 4.0 or newer
* GTK 4, Libadwaita and WebKitGTK (`webkitgtk-6.0`, for Arch Wiki articles)
* `curl` and `git`

To build AUR packages you also need `base-devel`; `paru` is used when it is
installed.

## 🔧 Building from Source

```bash
cargo build --release --lib
```

Cargo fetches Linexin Center's crates from its repository while building.
Tests, how to build against a local Linexin Center checkout and how the widget
works inside are described in [DEVELOPMENT.md](DEVELOPMENT.md).

## Licence

Copyright (C) 2026 Petexy

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. It is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the GNU General Public License in `LICENSE` for more
details.
