# Media server

The project is designed to be a media server with easy searching and downloading any media content.

It uses different metadata providers to find or group local media files in shows, seasons and movies.
The goal is to let you search for a movie/show and download it in a single step.

## How to use

### 1. You Tell It Where Your Files Are

When setting up your media server, you'll point it to the folders where your files live. For example:

- A folder named Movies for your films
- A folder named TV Shows with subfolders for each series

You can keep using the same folder structure you already use. The server just needs to know where to look.

### 2. It Scans and Organizes Everything

Once you add your folders, the server scans them and matches each file with information from online databases. It tries to identify:

- Movie titles and posters
- Tv shows, episodes and seasons.

This process is called metadata fetching and makes your collection look polished and professional, like something from Netflix or Spotify.

It simplifies the management of your library and provides additional nice features, like intro skip in TV Shows.

### 3. You Stream to Any Device

After setup, you can browse and play your content from almost anywhere:

- Smart TVs
- Web browsers
- Mobile apps
- Streaming sticks (like Roku, Fire Stick, or Chromecast)

Feel free to try out demo [here](https://demo.provod.rs)

## Key Features

- [x] Metadata fetching
- [x] Torrent client
- [x] Media transcoding
- [x] UPnP capabilities

## Supported metadata providers

- [TMDB](https://www.themoviedb.org/)
- [TVDB](https://thetvdb.com/)

## Supported torrent indexes

- TPB
- RuTracker

## Build from source

#### Required dependencies

- `ffmpeg` and `ffprobe` are required. `--enable-chromaprint` ffmpeg build flag is required for intro detection feature.

##### Arch Linux

`pacman -S pkgconf ffmpeg clang`

##### Ubuntu

`apt install pkg-config ffmpeg libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libavdevice-dev libswscale-dev clang`

Nightly version of rust is required

1. Install sqlx-cli with `cargo install sqlx-cli`
2. Set `DATABASE_URL` environment variable to `sqlite://db/database.sqlite`.
3. Create database directory `db`
4. Install required dependencies and run database migrations `sqlx database setup`.
5. Run `cargo build --release`. To build with tray icon for Windows run `cargo build --release --features windows-tray`

## Windows installation

1. Download installer from [releases](https://github.com/dog4ik/media-server/releases).
2. Run installer and follow instructions

## Browser codec support

Many videos might not work because of browser limited codecs support. Your options are to either transcode / remux video
or try using a different browser.

You can download [Electron Client](https://github.com/dog4ik/media-server-electron/releases), built with flag `enable_platform_ac3_eac3_audio` enabled.
It supports a wider range of formats natively, eliminating the need for transcoding / remuxing.


## API Documentation

OpenAPI documentation can be found [here](https://demo.provod.rs/swagger-ui)

## Related projects

- [Web UI](https://github.com/dog4ik/media-server-web)
