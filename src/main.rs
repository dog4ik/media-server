use warp::Filter;

#[tokio::main]
async fn main() {
    let hello = warp::path!("hello" / String).map(|name| format!("Hello, {}!", name));
    let dir = warp::path("static").and(warp::fs::dir("test"));
    let routes = warp::any().and(hello.or(dir));
    warp::serve(routes).run(([127, 0, 0, 1], 5000)).await;
}
