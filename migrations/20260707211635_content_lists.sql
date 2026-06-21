create table if not exists lists (
  id integer not null primary key autoincrement,
  name text not null,
  description text,
  kind text not null default 'user',
  created_at datetime default current_timestamp not null,
  updated_at datetime default current_timestamp not null
);

create table if not exists list_items (
  id integer not null primary key autoincrement,
  list_id integer not null,
  metadata_id integer not null,
  -- Watchlist-only: whether the user has seen the latest release for this item.
  release_viewed boolean,
  created_at datetime default current_timestamp not null,
  unique (list_id, metadata_id),
  foreign key (list_id) references lists (id) on delete cascade,
  foreign key (metadata_id) references metadata (id)
);

create trigger if not exists list_items_touch_insert
after insert on list_items begin
  update lists set updated_at = current_timestamp where id = new.list_id;
end;

create trigger if not exists list_items_touch_delete
after delete on list_items begin
  update lists set updated_at = current_timestamp where id = old.list_id;
end;

create trigger if not exists list_items_touch_update
after update on list_items begin
  update lists set updated_at = current_timestamp where id in (new.list_id, old.list_id);
end;

insert into lists (id, name, kind) values
  (1, 'Saved', 'saved'),
  (2, 'Watchlist', 'watchlist');
