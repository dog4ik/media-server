use media_server::{serve_file::serve_file, serve_previews, serve_subs, Library};
use std::{path::PathBuf, str::FromStr};
use warp::{
    hyper::{Body, Response, StatusCode},
    Filter,
};

#[tokio::main]
async fn main() {
    let dirs =
        vec![PathBuf::from_str("/home/dog4ik/Documents/dev/rust/media-server/test").unwrap()];
    let library = Library::new(dirs).await;
    let clonned_lib = library.clone();
    let lib = library.clone();
    let title_filter = warp::path!(String / i32 / i32 / ..).map(
        move |title: String, season: i32, episode: i32| {
            let file = lib.items.iter().find(|item| {
                item.episode == episode as u8
                    && item.title == title.replace("-", " ")
                    && item.season == season as u8
            });
            println!("{:?} {:?} {:?}", title, season, episode);
            if let Some(file) = file {
                file.clone()
            } else {
                panic!("ayaya")
            }
        },
    );

    let previews = warp::path!("previews" / String / i32 / i32 / i32)
        .and(warp::get())
        .and_then(serve_previews);
    let subs = warp::any()
        .and(warp::get())
        .and(warp::path("subs"))
        .and(title_filter.clone())
        .and_then(|item| serve_subs(item, None));
    let subs_verbose = warp::any()
        .and(warp::get())
        .and(warp::path("subs"))
        .and(title_filter)
        .and(warp::path::param::<String>())
        .and_then(|ep, lang| {
            println!("{lang}");
            serve_subs(ep, Some(lang))
        });

    let video = warp::path!("videos" / String / i32 / i32)
        .and(warp::get())
        .and(warp::header::optional::<String>("Range"))
        .and_then({
            move |name: String, season: i32, episode: i32, range: Option<String>| {
                let lib = library.clone();
                let name = name.replace("-", " ");
                async move {
                    match lib.items.iter().find(|item| {
                        item.title == name
                            && item.season == season as u8
                            && item.episode == episode as u8
                    }) {
                        Some(ep) => serve_file(&ep.video_path, range).await,
                        None => Ok(Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::empty())
                            .unwrap()),
                    }
                }
            }
        });
    let library = warp::path!("library")
        .and(warp::get())
        .map(move || clonned_lib.as_json());
    let routes = warp::any()
        .and(subs_verbose)
        .or(subs)
        .or(previews)
        .or(video)
        .or(library);
    warp::serve(routes).run(([127, 0, 0, 1], 5000)).await;
}
