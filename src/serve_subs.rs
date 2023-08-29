use reqwest::StatusCode;

use crate::ShowFile;

pub async fn serve_subs(episode: ShowFile, lang: Option<String>) -> (StatusCode, Option<String>) {
    println!("{:?}", lang);
    let subs = episode.get_subtitles(lang).await;
    if let Some(subs) = subs {
        return (StatusCode::OK, Some(subs));
    } else {
        return (StatusCode::NO_CONTENT, None);
    }
}
