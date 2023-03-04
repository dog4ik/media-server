use media_server::{serve_previews, serve_subs};
use warp::Filter;

#[tokio::main]
async fn main() {
    let previews = warp::path!("previews" / String / i32 / i32 / i32)
        .and(warp::get())
        .and_then(serve_previews);
    let subs = warp::path!("subs" / String / i32 / i32)
        .and(warp::get())
        .and_then(serve_subs);
    let routes = warp::any().and(subs).or(previews);
    warp::serve(routes).run(([127, 0, 0, 1], 5000)).await;
}
