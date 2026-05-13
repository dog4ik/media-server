drop table genres;
create table content_genres_new (
  id integer not null primary key autoincrement,
  genre_id integer not null,
  metadata_id integer not null,
  foreign key (metadata_id) references metadata (id));

insert into content_genres_new select * from content_genres;

drop table content_genres;

alter table content_genres_new rename to content_genres;
