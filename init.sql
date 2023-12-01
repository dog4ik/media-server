CREATE TABLE IF NOT EXISTS shows (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    title TEXT NOT NULL, 
                                    release_date TEXT NOT NULL,
                                    poster TEXT,
                                    blur_data TEXT,
                                    backdrop TEXT,
                                    rating FLOAT NOT NULL,
                                    plot TEXT NOT NULL,
                                    original_language TEXT NOT NULL,
                                    UNIQUE (metadata_id, metadata_provider));
CREATE TABLE IF NOT EXISTS seasons (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    show_id INTEGER NOT NULL,
                                    number INTEGER NOT NULL,
                                    release_date TEXT NOT NULL,
                                    rating FLOAT NOT NULL,
                                    plot TEXT NOT NULL,
                                    poster TEXT,
                                    blur_data TEXT,
                                    UNIQUE (metadata_id, metadata_provider),
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episodes (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    video_id INTEGER NOT NULL UNIQUE,
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    season_id INTEGER NOT NULL,
                                    title TEXT NOT NULL, 
                                    number INTEGER NOT NULL,
                                    plot TEXT NOT NULL,
                                    poster TEXT NOT NULL,
                                    blur_data TEXT,
                                    release_date TEXT NOT NULL,
                                    rating FLOAT NOT NULL,
                                    UNIQUE (metadata_id, metadata_provider),
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE,
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS movies (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    title TEXT NOT NULL,
                                    blur_data TEXT,
                                    backdrop TEXT,
                                    plot TEXT NOT NULL,
                                    rating FLOAT NOT NULL,
                                    poster TEXT,
                                    original_language TEXT NOT NULL,
                                    release_date TEXT NOT NULL,
                                    UNIQUE (metadata_id, metadata_provider),
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS videos (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    path TEXT NOT NULL UNIQUE,
                                    hash TEXT NOT NULL,
                                    local_title TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    duration INTEGER NOT NULL,
                                    video_codec TEXT,
                                    audio_codec TEXT,
                                    resolution TEXT,
                                    scan_date DATETIME DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS variants (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    video_id INTEGER NOT NULL,
                                    path TEXT NOT NULL UNIQUE,
                                    hash TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    duration INTEGER NOT NULL,
                                    video_codec TEXT,
                                    audio_codec TEXT,
                                    resolution TEXT,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS subtitles (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    language TEXT NOT NULL,
                                    hash TEXT NOT NULL,
                                    path TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    video_id INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
