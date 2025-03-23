/// The upnp:class property is a required property and it indicates the class of the object.
///
/// Variants of this enum represent the hierarchy of the classes
#[derive(Debug, Clone)]
pub enum UpnpClass {
    /// This is a derived class of object used to represent a collection (container) of individual content
    /// objects and other collections of objects (nested containers).
    Container(Option<ContainerType>),
    /// This is a derived class of object used to represent individual content objects, that is: objects
    /// that do not contain other objects; for example, a music track on an audio CD.
    Item(Option<ItemType>),
    VendorDefined(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub enum ContainerType {
    /// A person instance represents an unordered collection of objects associated with a person.
    ///
    /// It may have a res property for playback of all items belonging to the person container. A person
    /// container can contain objects of class `album`, `item`, or `playlist`.
    ///
    /// The classes of objects a person container may actually contain is device-dependent.
    ///
    /// Additionally, the following allowed properties are recommended for this class:
    /// - [dc:language](super::properties::Language)
    Person(Option<PersonType>),
    /// `playlistContainer` instance represents a collection of objects.
    ///
    /// It is different from a musicAlbum container in the sense that a playlistContainer instance may contain a mix of
    /// audio, video and images and is typically created by a user, while an album container typically
    /// holds a fixed published sequence of songs (for example, an audio CD).
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:artist`
    /// - `upnp:genre`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - `upnp:producer`
    /// - `upnp:storageMedium`
    /// - [dc:description](super::properties::Description)
    /// - `dc:contributor`
    /// - [dc:date](super::properties::Date)
    /// - [dc:language](super::properties::Language)
    /// - `dc:rights`
    PlaylistContainer,
    /// An album instance represents an ordered collection of objects.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:storageMedium`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - [dc:description](super::properties::Description)
    /// - `dc:publisher`
    /// - `dc:contributor`
    /// - [dc:date](super::properties::Date)
    /// - `dc:relation`
    /// - `dc:rights`
    Album(Option<AlbumType>),
    /// A genre instance represents an unordered collection of objects that all belong to the same genre.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:genre`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - [dc:description](super::properties::Description)
    Genre(Option<GenreType>),
    /// A `channelGroup` container groups together a set of items that correspond to individual but
    /// related broadcast channels
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:channelGroupName`
    /// - `upnp:channelGroupName@id`
    /// - `upnp:epgProviderName`
    /// - `upnp:serviceProvider`
    /// - `upnp:icon`
    /// - `upnp:region`
    ChannelGroup(Option<ChannelGroupType>),
    /// An epgContainer instance (EPG container) is a program guide container which shall only
    /// contain objects for EPG information.
    ///
    /// Such as audio and video program items or other EPG containers to organize these program items
    /// The following allowed properties are recommended for this class:
    /// - `upnp:channelGroupName`
    /// - `upnp:channelGroupName@id`
    /// - `upnp:epgProviderName`
    /// - `upnp:serviceProvider`
    /// - `upnp:channelName`
    /// - `upnp:channelNr`
    /// - `upnp:channelID`
    /// - `upnp:channelID@type`
    /// - `upnp:channelID@distriNetworkName`
    /// - `upnp:channelID@distriNetworkID`
    /// - `upnp:radioCallSign`
    /// - `upnp:radioStationID`
    /// - `upnp:radioBand`
    /// - `upnp:callSign`
    /// - `upnp:networkAffiliation`
    /// - `upnp:serviceProvider`
    /// - `upnp:price`
    /// - `upnp:price@currency`
    /// - `upnp:payPerView`
    /// - `upnp:epgProviderName`
    /// - `upnp:icon`
    /// - `upnp:region`
    /// - [dc:language](super::properties::Language)
    /// - `dc:relation`
    /// - `upnp:dateTimeRange`
    EpgContainer,
    /// A storageSystem instance represents a potentially heterogeneous collection of storage media.
    ///
    /// A storageSystem may contain other objects, including storageSystem containers,
    /// storageVolume containers or storageFolder containers.
    ///
    /// A storageSystem shall either be a
    /// child of the root container or a child of another storageSystem container.
    /// Examples of storageSystem instances are:
    /// - a CD Jukebox
    /// - a Hard Disk Drive plus a CD in a combo device
    /// - a single CD
    ///
    /// The following required properties are defined for this class:
    /// - `upnp:storageTotal`
    /// - `upnp:storageUsed`
    /// - `upnp:storageFree`
    /// - `upnp:storageMaxPartition`
    /// - `upnp:storageMedium`
    StorageSystem,
    /// A storageVolume instance represents all, or a partition of, some physical storage unit of a
    /// single type (as indicated by the storageMedium property).
    ///
    /// The storageVolume container may
    /// be writable, indicating whether new items can be created as children of the storageVolume
    /// container.
    ///
    /// A storageVolume container may contain other objects, except a [ContainerType::StorageSystem]
    /// container or another [ContainerType::StorageVolume] container.
    ///
    /// A `storageVolume` container shall either be a
    /// child of the root container or a child of a storageSystem container.
    ///
    /// Examples of storageVolume instances are:
    /// - a Hard Disk Drive
    /// - a partition on a Hard Disk Drive
    /// - a CD-Audio disc
    /// - a Flash memory card
    /// The following allowed required are defined for this class:
    /// - `upnp:storageTotal`
    /// - `upnp:storageUsed`
    /// - `upnp:storageFree`
    /// - `upnp:storageMedium`
    StorageVolume,
    /// A storageFolder instance represents a collection of objects stored on some storage medium.
    ///
    /// The storageFolder container may be writable, indicating whether new items can be created as
    /// children of the storageFolder container or whether existing child items can be removed.
    ///
    /// If the parent container is not writable, then the storageFolder container itself cannot be writable.
    ///
    /// A storageFolder container may contain other objects, except a storageSystem container or a
    /// storageVolume container.
    ///
    /// A storageFolder container shall either be a child of the root
    /// container or a child of another [ContainerType::StorageSystem] container, a [ContainerType::StorageVolume] container
    /// or a [ContainerType::StorageFolder] container.
    ///
    /// Examples of storageFolder instances are:
    /// - a directory on a Hard Disk Drive
    /// - a directory on CD-Rom, etc.
    /// The following required properties are defined for this class:
    /// - `upnp:storageUsed`
    StorageFolder,
    /// A `bookmarkFolder` instance represents an unordered collection of objects that either belong to
    /// the [ItemType::BookmarkItem] class.
    ///
    /// Its derived classes or the
    /// [ContainerType::BookmarkFolder] class and its derived classes.
    ///
    /// A `bookmarkFolder` instance may appear anywhere in the `ContentDirectory` hierarchy.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:genre`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - [dc:description](super::properties::Description)
    BookmarkFolder,
}

#[derive(Debug, Clone, Copy)]
pub enum PersonType {
    /// A musicArtist instance is a person instance, where the person associated with the container is
    /// a music artist.
    ///
    /// A musicArtist container can contain objects of class [AlbumType::MusicAlbum],
    /// [AudioItemType::MusicTrack] or [VideoItemType::MusicVideoClip].
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:genre`
    /// - `upnp:artistDiscographyURI`
    MusicArtist,
}

#[derive(Debug, Clone, Copy)]
pub enum AlbumType {
    /// A `musicAlbum` instance is an album container that contains items of class [AudioItemType::MusicTrack]
    /// or sub-album containers of class `musicAlbum`.
    ///
    /// It can be used to model, for example, an audio-CD.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:artist`
    /// - `upnp:genre`
    /// - `upnp:producer`
    /// - [`upnp:albumArtURI`](super::properties::AlbumArtUri)
    /// - `upnp:toc`
    MusicAlbum,
    /// `photoAlbum` instance is an album container that contains items of class photo
    /// or sub-album containers of class `photoAlbum`
    ///
    /// There are no additional recommended properties.
    PhotoAlbum,
}

#[derive(Debug, Clone, Copy)]
pub enum GenreType {
    /// A musicGenre instance is a genre which is interpreted as a style of music.
    ///
    /// A musicGenre container can contain objects of class [PersonType::MusicArtist], [AlbumType::MusicAlbum],
    /// [ItemType::AudioItem] or sub-musicgenres of the same class (for example, Rock contains Alternative Rock).
    MusicGenre,
    /// A movieGenre instance is a genre container where the genre indicates a movie style.
    ///
    /// A `movieGenre` container can contain objects of class people, [ItemType::VideoItem] or sub-moviegenres of
    /// the same class (for example, Western contains Spaghetti Western).
    ///
    /// The classes of objects a `movieGenre` container may actually contain is device-dependent
    MovieGenre,
}

#[derive(Debug, Clone, Copy)]
pub enum ChannelGroupType {
    /// An `audioChannelGroup` container groups together a set of items that correspond to individual
    /// but related audio broadcast channels.
    ///
    /// An `audioChannelGroup` container shall only contain objects of class [AudioItemType::AudioBroadcast].
    AudioChannelGroup,
    /// A `videoChannelGroup` container groups together a set of items that correspond to individual
    /// but related video broadcast channels.
    ///
    /// A `videoChannelGroup` container shall only contain objects of class [VideoItemType::VideoBroadcast].
    VideoChannelGroup,
}

#[derive(Debug, Clone, Copy)]
pub enum ItemType {
    /// An imageItem instance represents a still image object.
    ///
    /// It typically has at least one res property.
    /// The following allowed properties are recommended for this class:
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - `upnp:storageMedium`
    /// - `upnp:rating`
    /// - [dc:description](super::properties::Description)
    /// - `dc:publisher`
    /// - [dc:date](super::properties::Date)
    /// - `dc:rights`
    ImageItem(Option<ImageItemType>),
    /// An audioItem instance represents content that is intended for listening.
    ///
    /// Movies, TV broadcasts, etc., that also contain an audio track are excluded from this definition; those
    /// objects are classified under videoItem.
    ///
    /// It typically has at least one res property.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:genre`
    /// - [dc:description](super::properties::Description)
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - `dc:publisher`
    /// - [dc:language](super::properties::Language)
    /// - `dc:relation`
    /// - `dc:rights`
    AudioItem(Option<AudioItemType>),
    /// A `videoItem` instance represents content intended for viewing (as a combination of video and
    /// audio).
    ///
    /// It typically has at least one res property.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:genre`
    /// - `upnp:genre@id`
    /// - `upnp:genre@type`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - `upnp:producer`
    /// - `upnp:rating`
    /// - `upnp:actor`
    /// - `upnp:director`
    /// - [dc:description](super::properties::Description)
    /// - `dc:publisher`
    /// - [dc:language](super::properties::Language)
    /// - `dc:relation`
    /// - `upnp:playbackCount`
    /// - `upnp:lastPlaybackTime`
    /// - `upnp:lastPlaybackPosition`
    /// - `upnp:recordedDayOfWeek`
    /// - `upnp:srsRecordScheduleID`
    VideoItem(Option<VideoItemType>),
    /// A `playlistItem` instance represents a playable sequence of resources.
    ///
    /// It is different from musicAlbum in the sense that a playlistItem may contain a mix of audio, video and images
    /// and is typically created by a user, while an album is typically a fixed published sequence of
    /// songs (for example, an audio CD).
    ///
    /// A `playlistItem` is required to have a res property for
    /// playback of the whole sequence. This res property is a reference to a playlist file authored
    /// outside of the ContentDirectory service (for example, an external M3U file).
    ///
    /// Rendering the `playlistItem` has the semantics defined by the playlist’s resource (for example, ordering,
    /// transition effects, etc.).
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:artist`
    /// - `upnp:genre`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - `upnp:storageMedium`
    /// - [dc:description](super::properties::Description)
    /// - [dc:date](super::properties::Date)
    /// - [dc:language](super::properties::Language)
    PlaylistItem,
    /// A `textItem` instance represents a content intended for reading.
    ///
    /// It typically has at least one res property
    /// - `upnp:author`
    /// - `res@protection`
    /// - [upnp:longDescription](super::properties::LongDescription)
    /// - `upnp:storageMedium`
    /// - `upnp:rating`
    /// - [dc:description](super::properties::Description)
    /// - `dc:publisher`
    /// - `dc:contributor`
    /// - [dc:date](super::properties::Date)
    /// - `dc:relation`
    /// - [dc:language](super::properties::Language)
    /// - `dc:rights`
    TextItem,
    /// A `bookmarkItem` instance represents a piece of data that can be used to recover previous
    /// state information of a `AVTransport` and a `RenderingControl` service instance.
    ///
    /// A `bookmarkItem` instance can be located in any container but all bookmark items in the `ContentDirectory`
    /// service shall be accessible within one of the defined bookmark subtrees
    ///
    /// The following properties are either required or recommended for this class:
    /// `upnp:bookmarkedObjectID upnp (Required)
    /// `upnp:neverPlayable upnp (Allowed)
    /// `upnp:deviceUDN upnp (Required)
    /// `upnp:serviceType upnp (Required)
    /// `upnp:serviceId upnp (Required)
    /// - [dc:date](super::properties::Date) (Allowed)
    /// `upnp:stateVariableCollection upnp (Required)
    BookmarkItem,
}

#[derive(Debug, Clone, Copy)]
pub enum ImageItemType {
    /// A `Photo` instance represents a photo object (as opposed to, for example, an icon).
    ///
    /// It typically has at least one res property.
    /// The following allowed properties are recommended for this class:
    /// - `upnp:album`
    Photo,
}

#[derive(Debug, Clone, Copy)]
pub enum AudioItemType {
    /// A `musicTrack` instance represents music audio content. (as opposed to, for example, a news broadcast or an audio book)
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:artist`
    /// - `upnp:album`
    /// - `upnp:originalTrackNumber`
    /// - `upnp:playlist`
    /// - `upnp:storageMedium`
    /// - `dc:contributor`
    /// - [dc:date](super::properties::Date)
    MusicTrack,
    /// An audioBroadcast instance represents a continuous stream from an audio broadcast (as
    /// opposed to, for example, a song or an audio book).
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:region`
    /// - `upnp:radioCallSign`
    /// - `upnp:radioStationID`
    /// - `upnp:radioBand`
    /// - `upnp:channelNr`
    /// - `upnp:signalStrength`
    /// - `upnp:signalLocked`
    /// - `upnp:tuned`
    /// - `upnp:recordable`
    AudioBroadcast,
    /// An `audioBook` instance represents audio content that is the narration of a book (as opposed
    /// to, for example, a news broadcast or a song).
    ///
    /// The following allowed properties are recommended for this class:
    /// upnp:storageMedium upnp A
    /// - `upnp:producer`
    /// - `dc:contributor`
    /// - [dc:date](super::properties::Date)
    AudioBook,
}

#[derive(Debug, Clone, Copy)]
pub enum VideoItemType {
    /// `Movie` instance represents content that is a movie (as opposed to, for example, a
    /// continuous TV broadcast or a music video clip).
    ///
    /// It typically has at least one res property.
    /// - `upnp:storageMedium`
    /// - `upnp:DVDRegionCode`
    /// - `upnp:channelName`
    /// - `upnp:scheduledStartTime`
    /// - `upnp:scheduledEndTime`
    /// - `upnp:scheduledDuration`
    /// - [upnp:programTitle](super::properties::ProgramTitle)
    /// - [upnp:seriesTitle](super::properties::SeriesTitle)
    /// - [upnp:episodeCount](super::properties::EpisodeCount)
    /// - [upnp:episodeNumber](super::properties::EpisodeNumber)
    /// - [upnp:episodeSeason](super::properties::EpisodeSeason)
    Movie,
    /// A `videoBroadcast` instance represents a continuous stream from a video broadcast (for
    /// example, a conventional TV channel or a Webcast).
    ///
    /// It typically has at least one res property.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:icon`
    /// - `upnp:region`
    /// - `upnp:channelNr`
    /// - `upnp:signalStrength`
    /// - `upnp:signalLocked`
    /// - `upnp:tuned`
    /// - `upnp:recordable`
    /// - `upnp:callSign`
    /// - `upnp:price`
    /// - `upnp:payPerView`
    VideoBroadcast,
    /// musicVideoClip instance represents video content that is a clip supporting a song (as
    /// opposed to, for example, a continuous TV broadcast or a movie).
    ///
    /// It typically has at least one res property.
    ///
    /// This class is derived from the videoItem class and inherits the properties defined
    /// by that class.
    ///
    /// The following allowed properties are recommended for this class:
    /// - `upnp:artist`
    /// - `upnp:storageMedium`
    /// - `upnp:album`
    /// - `upnp:scheduledStartTime`
    /// - `upnp:scheduledStopTime`
    /// - `upnp:director`
    /// - `dc:contributor`
    /// - [dc:date](super::properties::Date)
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
