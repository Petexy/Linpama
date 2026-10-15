# Maintainer: Petexy <https://github.com/Petexy>

pkgname=linpama
pkgver=2.0.0.r
pkgrel=2
pkgdesc="Linexin's Pacman and AUR Wrapper"
url='https://github.com/Petexy'
arch=('x86_64')
license=('GPL-3.0')
depends=(
  'python-gobject'
  'gtk4'
  'libadwaita'
  'linexin-center'
  'wget'
  'webkitgtk-6.0'
)

package() {
    cd "${srcdir}"

    find . -mindepth 1 -type f | while IFS= read -r _file; do
        local _dest="${_file#./}"
        if [[ "${_dest}" == usr/bin/* ]]; then
            install -Dm755 "${_file}" "${pkgdir}/${_dest}"
        else
            install -Dm644 "${_file}" "${pkgdir}/${_dest}"
        fi
    done
}
