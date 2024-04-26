# Media server / Library manager

Project designed to be media library with ability to search and download new content without much hassle.

### Key Features

- [x] Metadata fetching
- [x] Torrent client
- [x] Media transcoding

### Required dependencies

- `ffmpeg` and `ffprobe` are required.

### Supported metadata providers

- [TMDB](https://www.themoviedb.org/)
- [TVDB](https://thetvdb.com/)

### Supported torrent indexes

- TPB

### Build from source

1. Install required dependencies and init database with `init.sql` file.
2. Run `cargo b`. To build with tray icon for Windows run `cargo b --features windows-tray`

### Windows installation

1. Download installer from [releases](https://github.com/dog4ik/media-server/releases).
2. Run installer and follow instructions, run the server.
3. Add your show folders inside `C:\Users\%USERPROFILE%\AppData\Roaming\media-server\configuration.toml`
   show_folders field should look like `show_folders = ["C:\\Users\\<Username>\\Videos\\Shows"]`.
4. Restart the server and press Refresh Library in tray icon.

Configuration page and initial setup helper will come later.

### Browser codec support

Many videos might not work because of browser limited codecs support. Your options are to either transcode the video
or try using a different browser.
From my experience Microsoft Edge supports more audio codecs while chrome can play higher video profiles.

You can download [custom chromium build](https://github.com/cjw1115/enable-chromium-ac3-ec3-system-decoding) or build chromium yourself with build flag `enable_platform_ac3_eac3_audio` enabled.
This custom build can play almost any video format.

### Related projcets

- [Web UI](https://github.com/dog4ik/media-server-web)
