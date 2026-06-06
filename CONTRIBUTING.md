# Contributing

## Project layout

The application is split across repositories:

- [media-server](https://github.com/dog4ik/media-server)
- [media-server-web](https://github.com/dog4ik/media-server-web)
- [media-server-installer](https://github.com/dog4ik/media-server-installer)
- [media-server-aur](https://github.com/dog4ik/media-server-aur).

### Working with the database / sqlx

You only need `sqlx-cli` and a database when you **change a SQL query**. Install it
with `cargo install sqlx-cli`.

```sh
mkdir db && sqlx database setup # create db/ and apply migrations (needs sqlx-cli)
cargo sqlx prepare              # regenerate the .sqlx cache after changing a query
```

## Release process

**The server's minor version must match the web client's minor version.** Patch
versions may differ. When the server is built for release it fetches the latest
web-client release whose tag starts with the server's `vMAJOR.MINOR`.
Keep the two repos minor versions in lockstep when releasing.

1. Tag the web client (`vX.Y.Z`) first so its `dist.tar.gz` release exists.
2. Make sure the server's `Cargo.toml` version shares the same `X.Y` minor.
3. Tag the server `vX.Y.Z` and push the tag. `release.yaml` builds and uploads the
   `.deb` and Windows installer.
4. For Arch, bump and push the [media-server-aur](https://github.com/dog4ik/media-server-aur)
   PKGBUILD (see its README).
