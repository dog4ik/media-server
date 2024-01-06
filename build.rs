use std::{fs, path::PathBuf, str::FromStr};

use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions};

const APP_NAME: &str = "media_server";

fn init_prod_storage() -> PathBuf {
    dirs::data_dir().unwrap().join(APP_NAME)
}

fn init_debug_storage() -> PathBuf {
    PathBuf::from(".").canonicalize().unwrap()
}

#[tokio::main]
async fn main() {
    let is_prod = !cfg!(debug_assertions);

    let store_path = if is_prod {
        init_prod_storage()
    } else {
        init_debug_storage()
    };
    let db_folder = store_path.join("db");

    fs::create_dir_all(&db_folder).unwrap();
    fs::create_dir_all(store_path.join("resources")).unwrap();
    let db_path = db_folder.join("database.sqlite");

    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&db_path)
        .unwrap();
    let db_url = format!("sqlite://{}", db_path.to_string_lossy().to_string());
    let mut connection = SqliteConnectOptions::from_str(&db_url)
        .unwrap()
        .connect()
        .await
        .unwrap();
    let init_query = fs::read_to_string("init.sql").unwrap();
    sqlx::query(&init_query)
        .execute(&mut connection)
        .await
        .unwrap();
}
