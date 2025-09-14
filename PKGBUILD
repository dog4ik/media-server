pkgname=media-server
pkgver=0.4.5
pkgrel=1
pkgdesc="Self-hosted media server"
arch=('x86_64')
url="https://github.com/dog4ik/media-server"
license=('GPL3')
depends=('openssl' 'systemd')
makedepends=('cargo' 'sqlite' 'npm' 'sqlx-cli')
install='media-server.install'
source=(
  "$pkgname::git+https://github.com/dog4ik/media-server.git#branch=dev"
  "media-server-web::git+https://github.com/dog4ik/media-server-web.git"
)
sha256sums=('SKIP' 'SKIP')

build() {
  # fix sqlx build
  CFLAGS+=" -ffat-lto-objects"
  cd "$srcdir/$pkgname"

  # setup sqlite database
  export DATABASE_URL=sqlite://db/database.sqlite
  mkdir -p db
  sqlx database setup

  # build backend
  rustup default nightly
  cargo build --release

  # build frontend
  cd "$srcdir/media-server-web"
  npm install
  npm run build
}

package() {
  # backend binary
  install -Dm755 "$srcdir/$pkgname/target/release/media-server" \
    "$pkgdir/usr/bin/media-server"

  # frontend dist
  install -d "$pkgdir/usr/share/media-server/dist"
  cp -r "$srcdir/media-server-web/dist/"* \
    "$pkgdir/usr/share/media-server/dist"

  # systemd unit
  install -Dm644 "$srcdir/$pkgname/media-server.service" \
    "$pkgdir/usr/lib/systemd/system/media-server.service"
}
