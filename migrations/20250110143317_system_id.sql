CREATE TABLE IF NOT EXISTS system_id(
  id INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO system_id (id) VALUES (0);

CREATE TRIGGER system_id_movies_insert
AFTER INSERT ON movies
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_movies_update
AFTER UPDATE ON movies
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_movies_delete
AFTER DELETE ON movies
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;


CREATE TRIGGER system_id_shows_insert
AFTER INSERT ON shows
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_shows_update
AFTER UPDATE ON shows
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_shows_delete
AFTER DELETE ON shows
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;


CREATE TRIGGER system_id_seasons_insert
AFTER INSERT ON seasons
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_seasons_update
AFTER UPDATE ON seasons
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_seasons_delete
AFTER DELETE ON seasons
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_episodes_insert
AFTER INSERT ON episodes
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_episodes_update
AFTER UPDATE ON episodes
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;

CREATE TRIGGER system_id_episodes_delete
AFTER DELETE ON episodes
BEGIN
    UPDATE system_id
    SET id = id + 1;
END;
