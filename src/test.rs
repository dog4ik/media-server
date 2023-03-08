#[cfg(test)]
mod tests {
    use crate::scan;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_scan() {
        let input = vec![PathBuf::from(std::env::var("LIBRARY_PATH").unwrap())];
    }
}
