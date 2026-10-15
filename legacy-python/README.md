# Linpama

<p align="center">
  <img src="https://i.ibb.co/V0rSgx74/github-petexy-linpama.png" alt="Linpama" with="200" height="200"/>
</p>

**Linpama** is a modern, lightweight graphical package manager built for Linexin, but also working under any Arch-based Linux. Built with **Python**, **GTK4**, and **Libadwaita**, it provides a cohesive interface for managing system packages via `pacman` and community packages via the **AUR** (Arch User Repository).

It distinguishes itself with a focus on safe and stable package management habits while providing powerful tools for advanced users.

## ✨ Features

* **Unified Search:** seamlessly search your repositories and the AUR simultaneously.
* **Modern UI:** Native GNOME look and feel using GTK4 and Libadwaita.
* **AUR Integration:**
    * Search and install AUR packages.
    * **Security First:** Integrated **PKGBUILD reviewer** allows you to inspect build scripts before installation to check for malicious code.
    * Automated dependency resolution and building via `makepkg`.
* **System Stability Warning:** A proactive onboarding screen that advises users on the risks of direct system package installation and suggests using Flatpaks for GUI applications to prevent dependency conflicts.
* **Transaction Transparency:** View real-time, detailed logs of install and remove processes.
* **Smart Icons:** Automatically resolves and displays package icons using system themes and AppStream data.

## 📦 Prerequisites

Linpama is designed for Arch Linux and its derivatives. Before running the application, ensure the following dependencies are installed:

```bash
sudo pacman -S python python-gobject gtk4 libadwaita base-devel git
