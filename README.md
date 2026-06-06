# Media server

The project is designed to let you explore, download and watch content using a single app.

## How it works

### 1. You Tell It Where Your Files Are

When setting up your media server, you'll point it to the folders where your files live. For example:

- A folder named Movies for your films
- A folder named TV Shows with subfolders for each series

You can keep using the same folder structure you already have. The server just needs to know where to look.

### 2. It Scans and Organizes Everything

Once you add your folders, the server scans them and matches each file with information from online databases
This makes your collection look polished and professional, like something from Netflix or Spotify.
It also simplifies the management of your library and provides additional nice features, like intro skip in TV Shows.

### 3. You Stream to Any Device

After setup, you can browse and play your content from almost anywhere:

- Smart TVs
- Web browsers
- Mobile apps

Feel free to try out demo [here](https://demo.provod.rs)

## Supported metadata providers

- [TMDB](https://www.themoviedb.org/)
- [TVDB](https://thetvdb.com/)

## Supported torrent indexes

- TPB
- RuTracker

# Installation

## Arch Linux

Install from the AUR (e.g. `paru -S media-server`), or build the
[PKGBUILD](https://github.com/dog4ik/media-server-aur) manually with `makepkg -si`.

## Ubuntu / Debian

Download the `.deb` from the [latest release](https://github.com/dog4ik/media-server/releases)
and install it:

```sh
sudo apt install ./media-server_*_amd64.deb
```

This installs the necessary components. Start it with:

```sh
sudo systemctl enable --now media-server
```

## Windows

1. Download the installer (`media-server-setup-win-x64.exe`) from the
   [latest release](https://github.com/dog4ik/media-server/releases).
2. Run it and follow the instructions.

## Docker

You can build and run docker container

`docker build -t media-server:latest .`

`docker run --name media-server -p 6969:6969 media-server:latest`

An container image is published to Docker Hub as `dog4ik/media-server`.

## Build from source

The MSRV is **1.96.0**.

### Dependencies

`ffmpeg` and `ffprobe` are required at runtime. An ffmpeg build with `--enable-chromaprint` is required for the intro-detection feature.

- **Arch Linux:** `pacman -S pkgconf ffmpeg clang`
- **Ubuntu/Debian:** `apt install pkg-config ffmpeg clang libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libavdevice-dev libswscale-dev`

### Build

```sh
cargo build --release
```

To build with the Windows tray menu run: `cargo build --release --features windows-tray`.

## API Documentation

OpenAPI documentation can be found [here](https://demo.provod.rs/swagger-ui)

## Related projects

- [Web UI](https://github.com/dog4ik/media-server-web)
