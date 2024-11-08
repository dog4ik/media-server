#[derive(Debug, Clone)]
pub enum UpnpClass {
    Container(Option<ContainerType>),
    Item(Option<ItemType>),
    VendorDefined(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub enum ContainerType {
    Person(Option<PersonType>),
    PlaylistContainer,
    Album(Option<AlbumType>),
    Genre(Option<GenreType>),
    ChannelGroup(Option<ChannelGroupType>),
    EpgContainer,
    StorageSystem,
    StorageVolume,
    StorageFolder,
    BookmarkFolder,
}

#[derive(Debug, Clone, Copy)]
pub enum PersonType {
    MusicArtist,
}

#[derive(Debug, Clone, Copy)]
pub enum AlbumType {
    MusicAlbum,
    PhotoAlbum,
}

#[derive(Debug, Clone, Copy)]
pub enum GenreType {
    MusicGenre,
    MovieGenre,
}

#[derive(Debug, Clone, Copy)]
pub enum ChannelGroupType {
    AudioChannelGroup,
    VideoChannelGroup,
}

#[derive(Debug, Clone, Copy)]
pub enum ItemType {
    ImageItem(Option<ImageItemType>),
    AudioItem(Option<AudioItemType>),
    VideoItem(Option<VideoItemType>),
    PlaylistItem,
    TextItem,
    BookmarkItem,
}

#[derive(Debug, Clone, Copy)]
pub enum ImageItemType {
    Photo,
}

#[derive(Debug, Clone, Copy)]
pub enum AudioItemType {
    MusicTrack,
    AudioBroadcast,
    AudioBook,
}

#[derive(Debug, Clone, Copy)]
pub enum VideoItemType {
    Movie,
    VideoBroadcast,
    MusicVideoClip,
}

impl UpnpClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            UpnpClass::Container(container_type) => match container_type {
                None => "object.container",
                Some(ContainerType::Album(album_type)) => match album_type {
                    None => "object.container.album",
                    Some(AlbumType::MusicAlbum) => "object.container.album.musicAlbum",
                    Some(AlbumType::PhotoAlbum) => "object.container.album.photoAlbum",
                },
                Some(ContainerType::Genre(genre_type)) => match genre_type {
                    None => "object.container.genre",
                    Some(GenreType::MusicGenre) => "object.container.genre.musicGenre",
                    Some(GenreType::MovieGenre) => "object.container.genre.movieGenre",
                },
                Some(ContainerType::Person(person_type)) => match person_type {
                    None => "object.container.person",
                    Some(PersonType::MusicArtist) => "object.container.person.musicArtist",
                },
                Some(ContainerType::ChannelGroup(channel_group_type)) => match channel_group_type {
                    None => "object.container.channelGroup",
                    Some(ChannelGroupType::AudioChannelGroup) => {
                        "object.container.channelGroup.audioChannelGroup"
                    }
                    Some(ChannelGroupType::VideoChannelGroup) => {
                        "object.container.channelGroup.videoChannelGroup"
                    }
                },
                Some(ContainerType::PlaylistContainer) => "object.container.playlistContainer",
                Some(ContainerType::BookmarkFolder) => "object.container.bookmarkFolder",
                Some(ContainerType::StorageFolder) => "object.container.storageFolder",
                Some(ContainerType::StorageVolume) => "object.container.storageVolume",
                Some(ContainerType::StorageSystem) => "object.container.storageSystem",
                Some(ContainerType::EpgContainer) => "object.container.epgContainer",
            },
            UpnpClass::Item(item_type) => match item_type {
                None => "object.item",
                Some(ItemType::VideoItem(video_item_type)) => match video_item_type {
                    None => "object.item.videoItem",
                    Some(VideoItemType::Movie) => "object.item.videoItem.movie",
                    Some(VideoItemType::VideoBroadcast) => "object.item.videoItem.videoBroadcast",
                    Some(VideoItemType::MusicVideoClip) => "object.item.videoItem.musicVideoClip",
                },
                Some(ItemType::AudioItem(audio_item_type)) => match audio_item_type {
                    None => "object.item.audioItem",
                    Some(AudioItemType::AudioBook) => "object.item.audioItem.audioBook",
                    Some(AudioItemType::MusicTrack) => "object.item.audioItem.musicTrack",
                    Some(AudioItemType::AudioBroadcast) => "object.item.audioItem.audioBroadcast",
                },
                Some(ItemType::ImageItem(image_item_type)) => match image_item_type {
                    None => "object.item.imageItem",
                    Some(ImageItemType::Photo) => "object.item.imageItem.photo",
                },
                Some(ItemType::TextItem) => "object.item.textItem",
                Some(ItemType::PlaylistItem) => "object.item.playlistItem",
                Some(ItemType::BookmarkItem) => "object.item.bookmarkItem",
            },
            UpnpClass::VendorDefined(s) => s,
        }
    }
}
