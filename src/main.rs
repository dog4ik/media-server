use dotenv::dotenv;
use media_server::{serve_file::serve_file, serve_previews, serve_subs, Library};
use std::sync::{Arc, RwLock};
use std::{path::PathBuf, str::FromStr};
use warp::Filter;

#[tokio::main]
async fn main() {
    dotenv().ok();
    let library_dir = std::env::var("LIBRARY_PATH").unwrap();
    let dirs = vec![PathBuf::from_str(&library_dir).unwrap()];
    let library = Arc::new(RwLock::new(Library::new(dirs).await));
    let title_filter = warp::path!(String / i32 / i32 / ..).map({
        let library = library.clone();
        move |title: String, season: i32, episode: i32| {
            let library = library.read().unwrap();
            let file = library.items.iter().find(|item| {
                item.episode == episode as u8
                    && item.title == title.replace("-", " ")
                    && item.season == season as u8
            });
            println!("{:?} {:?} {:?}", title, season, episode);
            if let Some(file) = file {
                file.clone()
            } else {
                //TODO: fix this panic
                panic!("aayayaya")
            }
        }
    });

    let previews = warp::any()
        .and(warp::get())
        .and(warp::path("previews"))
        .and(title_filter.clone())
        .and(warp::path::param::<i32>())
        .and_then(serve_previews);
    let subs = warp::any()
        .and(warp::get())
        .and(warp::path("subs"))
        .and(title_filter.clone())
        .and_then(|item| serve_subs(item, None));
    let subs_verbose = warp::any()
        .and(warp::get())
        .and(warp::path("subs"))
        .and(title_filter.clone())
        .and(warp::path::param::<String>())
        .and_then(|ep, lang| {
            println!("{lang}");
            serve_subs(ep, Some(lang))
        });

    let video = warp::any()
        .and(warp::get())
        .and(warp::path("videos"))
        .and(title_filter.clone())
        .and(warp::header::optional::<String>("Range"))
        .and_then(serve_file);
    let summory = warp::path!("summary").map({
        let library = library.clone();
        move || library.read().unwrap().get_summary()
    });
    let library = warp::path!("library").and(warp::get()).map({
        let library = library.clone();
        move || library.read().unwrap().as_json()
    });
    let routes = warp::any()
        .and(subs_verbose)
        .or(subs)
        .or(previews)
        .or(video)
        .or(library)
        .or(summory);
    warp::serve(routes).run(([127, 0, 0, 1], 5000)).await;
}
