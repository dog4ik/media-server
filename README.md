# Media server

The project is designed to be a media library for easy searching and downloading any media content.

### Key Features

- [x] Metadata fetching
- [x] Torrent client
- [x] Media transcoding
- [x] UPnP capabilities

### Required dependencies

- `ffmpeg` and `ffprobe` are required. `--enable-chromaprint` ffmpeg build flag is required for intro detection feature.

### Supported metadata providers

- [TMDB](https://www.themoviedb.org/)
- [TVDB](https://thetvdb.com/)

### Supported torrent indexes

- TPB

### Build from source

1. Install rust and sqlx-cli `cargo install sqlx-cli`
2. Set `DATABASE_URL` environment variable to `sqlite://db/database.sqlite`.
3. Create database directory `db`
3. Install required dependencies and run database migrations `sqlx database create`, `sqlx migrate run`.
4. Run `cargo build`. To build with tray icon for Windows run `cargo b -r --features windows-tray`

### Windows installation

1. Download installer from [releases](https://github.com/dog4ik/media-server/releases).
2. Run installer and follow instructions, run the server.
3. Add shows/movies folders in settings

### Browser codec support

Many videos might not work because of browser limited codecs support. Your options are to either transcode the video
or try using a different browser.
From my experience Microsoft Edge supports more audio codecs while Chrome can play higher video profiles.

You can download [custom chromium build](https://github.com/cjw1115/enable-chromium-ac3-ec3-system-decoding) or build chromium yourself with build flag `enable_platform_ac3_eac3_audio` enabled.
This custom build can play almost any video format.
You can download custom electron build with Windows Installer, built with flag `enable_platform_ac3_eac3_audio` enabled

### Related projects

- [Web UI](https://github.com/dog4ik/media-server-web)
