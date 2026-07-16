-- Cast subquery per content row; single-column so rows keep insertion
-- (billing) order within a metadata_id
create index if not exists roles_metadata_id_idx on roles (metadata_id);
-- Content listing filtered by actor
create index if not exists roles_actor_id_idx on roles (actor_id, metadata_id);

-- Actor lookups
create index if not exists actors_external_id_idx on actors (external_metadata_provider, external_metadata_id);
create index if not exists actors_imdb_id_idx on actors (imdb_id) where imdb_id is not null;

-- metadata.id back-references
-- Single-column to preserve insertion order inside json_group_array results
create index if not exists external_ids_metadata_id_idx on external_ids (metadata_id);
create index if not exists content_genres_metadata_id_idx on content_genres (metadata_id);
create index if not exists list_items_metadata_id_idx on list_items (metadata_id);
create index if not exists videos_metadata_id_idx on videos (metadata_id);
create index if not exists shows_metadata_id_idx on shows (metadata_id);
create index if not exists seasons_metadata_id_idx on seasons (metadata_id);
create index if not exists episodes_metadata_id_idx on episodes (metadata_id);
create index if not exists movies_metadata_id_idx on movies (metadata_id);
create index if not exists content_downloads_metadata_id_idx on content_downloads (metadata_id);
create index if not exists torrent_files_metadata_id_idx on torrent_files (metadata_id);

-- Show tree traversal: seasons of a show, episodes of a season, both also looked up by number
create index if not exists seasons_show_id_idx on seasons (show_id, number);
create index if not exists episodes_season_id_idx on episodes (season_id, number);

-- History cursor pagination ordered by update_time
create index if not exists history_update_time_idx on history (update_time);

-- Subtitles of a video
create index if not exists subtitles_video_id_idx on subtitles (video_id);

-- Files of a torrent
create index if not exists torrent_files_torrent_id_idx on torrent_files (torrent_id);
create index if not exists content_downloads_torrent_id_idx on content_downloads (torrent_id);

-- Case-insensitive title lookup used when matching library files to shows
create index if not exists metadata_title_nocase_idx on metadata (title collate nocase);
