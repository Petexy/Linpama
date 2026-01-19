# Maintainer: Petexy <https://github.com/Petexy>

pkgname=linpama
pkgver=1.0.4.r
pkgrel=1
_currentdate=$(date +"%Y-%m-%d%H-%M-%S")
pkgdesc="Linexin's Pacman and AUR Wrapper"
url='https://github.com/Petexy'
arch=(x86_64)
license=('GPL-3.0')
depends=(
  python-gobject
  gtk4
  libadwaita
  linexin-center
  wget
)
makedepends=(
)

package() {
   mkdir -p ${pkgdir}/usr/share/linexin/widgets
   mkdir -p ${pkgdir}/usr/icons   
   cp -rf ${srcdir}/* ${pkgdir}/
}
