CREATE TABLE IF NOT EXISTS shows (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    title TEXT NOT NULL, 
                                    release_date TEXT,
                                    poster TEXT,
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

CREATE TABLE IF NOT EXISTS seasons (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    show_id INTEGER NOT NULL,
                                    number INTEGER NOT NULL,
                                    release_date TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episodes (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    season_id INTEGER NOT NULL,
                                    title TEXT NOT NULL, 
                                    number INTEGER NOT NULL,
                                    plot TEXT,
                                    poster TEXT,
                                    release_date TEXT,
                                    duration INTEGER NOT NULL,
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS movies (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    title TEXT NOT NULL,
                                    backdrop TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    release_date TEXT,
                                    duration INTEGER NOT NULL);

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

CREATE TABLE IF NOT EXISTS videos (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    path TEXT NOT NULL UNIQUE,
                                    size INTEGER NOT NULL,
                                    episode_id INTEGER,
                                    movie_id INTEGER,
                                    is_prime BOOL NOT NULL,
                                    scan_date DATETIME DEFAULT CURRENT_TIMESTAMP,
                                    FOREIGN KEY (episode_id) REFERENCES episodes (id) ON DELETE SET NULL,
                                    FOREIGN KEY (movie_id) REFERENCES movies (id) ON DELETE SET NULL);
CREATE TABLE IF NOT EXISTS subtitles (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    language TEXT,
                                    external_path TEXT,
                                    video_id INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS history (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    time INTEGER NOT NULL,
                                    is_finished BOOL NOT NULL,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    update_time DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS external_ids (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
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
CREATE TABLE IF NOT EXISTS episode_intro (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    start_sec INTEGER NOT NULL,
                                    end_sec INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
