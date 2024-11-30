CREATE TABLE IF NOT EXISTS torrents(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    bencoded_info BLOB NOT NULL,
                                    bitfield BLOB NOT NULL, 
                                    save_location TEXT NOT NULL, 
                                    enabled_files BLOB NOT NULL, 
                                    trackers TEXT NOT NULL,
                                    info_hash BLOB NOT NULL,
                                    added_at DATETIME DEFAULT CURRENT_TIMESTAMP);
