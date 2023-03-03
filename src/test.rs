#[cfg(test)]
mod tests {
    use crate::{process_file, scan};
    use std::{path::PathBuf, str::FromStr};

    #[test]
    fn test_scan() {
        let input = vec![PathBuf::from(
            "/home/dog4ik/Documents/dev/rust/worktrees/warp/test",
        )];
        let thing = scan(input);
        thing
            .iter()
            .for_each(|y| println!("{} {} {}", y.title, y.season, y.episode));
    }
    #[tokio::test]
    async fn process_file_test() {
        let path =
            PathBuf::from_str("/home/dog4ik/Documents/dev/rust/worktrees/warp/test/video.mkv")
                .unwrap();
        let data = process_file(&path).await.unwrap();
        println!("{:?}", data.streams[0])
    }
}
