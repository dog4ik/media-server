use std::collections::HashMap;

pub trait BucketItem {
    fn title(&self) -> &str;
    fn year(&self) -> Option<u16>;
}

fn normalizie_title(title: &str) -> String {
    title.to_lowercase()
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub struct BucketKey {
    pub title: String,
    pub year: Option<u16>,
}

pub fn bucket_items<T>(items: impl IntoIterator<Item = T>) -> HashMap<BucketKey, Vec<T>>
where
    T: BucketItem,
{
    let mut buckets: HashMap<_, Vec<_>> = HashMap::new();
    for item in items.into_iter() {
        buckets
            .entry(BucketKey {
                title: normalizie_title(&item.title()),
                year: item.year(),
            })
            .or_default()
            .push(item);
    }
    buckets
}
