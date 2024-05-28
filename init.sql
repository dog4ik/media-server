CREATE TABLE IF NOT EXISTS shows (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    title TEXT NOT NULL, 
                                    release_date TEXT,
                                    poster TEXT,
                                    blur_data TEXT,
                                    backdrop TEXT,
                                    plot TEXT);

CREATE VIRTUAL TABLE IF NOT EXISTS shows_fts_idx USING fts5(title, plot, content='shows', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS shows_tbl_ai AFTER INSERT ON shows BEGIN
  INSERT INTO shows_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;
CREATE TRIGGER IF NOT EXISTS shows_tbl_ad AFTER DELETE ON shows BEGIN
  INSERT INTO shows_fts_idx(shows_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
END;
CREATE TRIGGER IF NOT EXISTS shows_tbl_au AFTER UPDATE ON shows BEGIN
  INSERT INTO shows_fts_idx(shows_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
  INSERT INTO shows_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;

CREATE TABLE IF NOT EXISTS seasons (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    show_id INTEGER NOT NULL,
                                    number INTEGER NOT NULL,
                                    release_date TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    blur_data TEXT,
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episodes (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    video_id INTEGER NOT NULL UNIQUE,
                                    season_id INTEGER NOT NULL,
                                    title TEXT NOT NULL, 
                                    number INTEGER NOT NULL,
                                    plot TEXT,
                                    poster TEXT,
                                    blur_data TEXT,
                                    release_date TEXT,
                                    FOREIGN KEY (video_id) REFERENCES videos (id),
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS movies (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    title TEXT NOT NULL,
                                    blur_data TEXT,
                                    backdrop TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    release_date TEXT,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);

CREATE VIRTUAL TABLE IF NOT EXISTS movies_fts_idx USING fts5(title, plot, content='movies', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS movies_tbl_ai AFTER INSERT ON movies BEGIN
  INSERT INTO movies_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;
CREATE TRIGGER IF NOT EXISTS movies_tbl_ad AFTER DELETE ON movies BEGIN
  INSERT INTO movies_fts_idx(movies_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
END;
CREATE TRIGGER IF NOT EXISTS movies_tbl_au AFTER UPDATE ON movies BEGIN
  INSERT INTO movies_fts_idx(movies_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
  INSERT INTO movies_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;

CREATE TABLE IF NOT EXISTS videos (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    path TEXT NOT NULL UNIQUE,
                                    size INTEGER NOT NULL,
                                    duration INTEGER NOT NULL,
                                    scan_date DATETIME DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS subtitles (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    language TEXT NOT NULL,
                                    hash TEXT NOT NULL,
                                    path TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    video_id INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    time INTEGER NOT NULL,
                                    is_finished BOOL NOT NULL,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    update_time DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS external_ids (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    metadata_provider TEXT NOT NULL,
                                    metadata_id TEXT NOT NULL,
                                    show_id INTEGER,
                                    season_id INTEGER,
                                    episode_id INTEGER,
                                    movie_id INTEGER,
                                    is_prime INTEGER NOT NULL,
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE,
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE,
                                    FOREIGN KEY (episode_id) REFERENCES episodes (id) ON DELETE CASCADE,
                                    FOREIGN KEY (movie_id) REFERENCES movies (id) ON DELETE CASCADE);
