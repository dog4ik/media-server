use std::{fmt::Display, str::FromStr};

use anyhow::Context;
use quick_xml::events::BytesText;

use super::{
    action::{Action, OutArgument},
    service::Service,
    service_variables::{self, IntoUpnpValue, SVariable, StateVariableDescriptor},
    templates::{service_description::ServiceDescription, SpecVersion},
    urn::{ServiceType, UrnType, URN},
    IntoXml, XmlWriter,
};

#[derive(Debug, Clone)]
pub struct ContentDirectoryService {
    pub db: Db,
}

impl ContentDirectoryService {
    pub async fn browse(
        &self,
        object_id: OutArgument<ObjectID>,
        browse_flag: OutArgument<BrowseFlag>,
        filter: OutArgument<Filter>,
        start_index: OutArgument<Index>,
        requested_count: OutArgument<Count>,
        sort_criteria: OutArgument<SortCriteria>,
    ) -> anyhow::Result<(
        OutArgument<ArgResult>,
        OutArgument<Count>,
        OutArgument<Count>,
        OutArgument<UpdateID>,
    )> {
        let id = object_id.as_ref().as_str();
        let result = match id {
            "0" => properties::DidlResponse::root(),
            "shows" => {

            },
            "movies" => ,
        };

        let number_returned = result.len();
        let total_matches = result.len();
        let result = result.into_xml().unwrap();
        let update_id = 1;
        match browse_flag.as_ref() {
            BrowseFlag::BrowseDirectChildren => {

            }
            BrowseFlag::BrowseMetadata => {}
        };
        Ok((
            OutArgument::new("Result", result),
            OutArgument::new("NumberReturned", number_returned as u32),
            OutArgument::new("TotalMatches", total_matches as u32),
            OutArgument::new("UpdateID", update_id),
        ))
    }

    async fn browse_direct_children(id: &str) -> Result<properties::DidlResponse, AppError> {}
}

#[derive(Debug)]
/// This required state variable is introduced to provide type information for the BrowseFlag
/// argument in the Browse() action. A BrowseFlag argument specifies a browse option to be
/// used for browsing the ContentDirectory service
enum BrowseFlag {
    /// This is used to browse the direct children of a container (like folders or files).
    /// You're expected to return only the direct children
    /// of the container that was requested.
    BrowseDirectChildren,
    /// This is used to retrieve metadata for a specific object (such as a container or an item).
    /// You're expected to return metadata for the specific object requested.
    BrowseMetadata,
}

impl Display for BrowseFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowseFlag::BrowseDirectChildren => write!(f, "BrowseDirectChildren"),
            BrowseFlag::BrowseMetadata => write!(f, "BrowseMetadata"),
        }
    }
}

impl FromStr for BrowseFlag {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BrowseMetadata" => Ok(Self::BrowseMetadata),
            "BrowseDirectChildren" => Ok(Self::BrowseDirectChildren),
            _ => Err(anyhow::anyhow!("Unknown browse flag: {s}")),
        }
    }
}

impl IntoUpnpValue for BrowseFlag {
    const TYPE_NAME: service_variables::DataType = service_variables::DataType::String;

    fn into_value(&self) -> service_variables::Value {
        service_variables::Value::String(self.to_string())
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        value.parse()
    }
}

impl SVariable for BrowseFlag {
    type VarType = Self;
    const VAR_NAME: &str = "A_ARG_TYPE_BrowseFlag";
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["BrowseMetadata", "BrowseDirectChildren"]);
}

#[derive(Default, Debug)]
struct ContainerUpdateIDs;
impl SVariable for ContainerUpdateIDs {
    type VarType = String;
    const VAR_NAME: &str = "ContainerUpdateIDs";
    const SEND_EVENTS: bool = true;
}

#[derive(Default, Debug)]
struct SystemUpdateId;
impl SVariable for SystemUpdateId {
    type VarType = u32;
    const VAR_NAME: &str = "SystemUpdateID";
    const SEND_EVENTS: bool = true;
}

#[derive(Default, Debug)]
struct Count;
impl SVariable for Count {
    type VarType = u32;
    const VAR_NAME: &str = "A_ARG_TYPE_Count";
}

#[derive(Default, Debug)]
struct SortCriteria;
impl SVariable for SortCriteria {
    type VarType = String;
    const VAR_NAME: &str = "A_ARG_TYPE_SortCriteria";
}

#[derive(Default, Debug)]
struct SortCapabilities;
impl SVariable for SortCapabilities {
    type VarType = String;
    const VAR_NAME: &str = "SortCapabilities";
}

#[derive(Default, Debug)]
struct Index;
impl SVariable for Index {
    type VarType = u32;
    const VAR_NAME: &str = "A_ARG_TYPE_Index";
}

#[derive(Default, Debug)]
struct ObjectID;
impl SVariable for ObjectID {
    type VarType = String;
    const VAR_NAME: &str = "A_ARG_TYPE_ObjectID";
}

#[derive(Default, Debug)]
struct UpdateID;
impl SVariable for UpdateID {
    type VarType = u32;
    const VAR_NAME: &str = "A_ARG_TYPE_UpdateID";
}

#[derive(Default, Debug)]
struct ArgResult;
impl SVariable for ArgResult {
    type VarType = String;
    const VAR_NAME: &str = "A_ARG_TYPE_Result";
}

#[derive(Default, Debug)]
struct SearchCapabilities;
impl SVariable for SearchCapabilities {
    type VarType = String;
    const VAR_NAME: &str = "SearchCapabilities";
}

#[derive(Default, Debug)]
struct Filter;
impl SVariable for Filter {
    type VarType = String;
    const VAR_NAME: &str = "A_ARG_TYPE_Filter";
}

impl Service for ContentDirectoryService {
    const NAME: &str = "content_directory";
    const URN: URN = URN {
        version: 1,
        urn_type: UrnType::Service(ServiceType::ContentDirectory),
    };

    fn service_description() -> ServiceDescription {
        let variables = vec![
            StateVariableDescriptor::from_variable::<BrowseFlag>(),
            StateVariableDescriptor::from_variable::<ContainerUpdateIDs>(),
            StateVariableDescriptor::from_variable::<SystemUpdateId>(),
            StateVariableDescriptor::from_variable::<Count>(),
            StateVariableDescriptor::from_variable::<SortCriteria>(),
            StateVariableDescriptor::from_variable::<SortCapabilities>(),
            StateVariableDescriptor::from_variable::<Index>(),
            StateVariableDescriptor::from_variable::<ObjectID>(),
            StateVariableDescriptor::from_variable::<UpdateID>(),
            StateVariableDescriptor::from_variable::<ArgResult>(),
            StateVariableDescriptor::from_variable::<SearchCapabilities>(),
            StateVariableDescriptor::from_variable::<Filter>(),
        ];
        ServiceDescription {
            spec_version: SpecVersion::upnp_v2(),
            variables,
            actions: Self::actions(),
        }
    }

    fn actions() -> Vec<Action> {
        let mut browse = Action::empty("Browse");
        browse.add_input::<ObjectID>("ObjectID");
        browse.add_input::<BrowseFlag>("BrowseFlag");
        browse.add_input::<Filter>("Filter");
        browse.add_input::<Index>("StartingIndex");
        browse.add_input::<Count>("RequestedCount");
        browse.add_input::<SortCriteria>("SortCriteria");
        browse.add_output::<ArgResult>("Result");
        browse.add_output::<Count>("NumberReturned");
        browse.add_output::<Count>("TotalMatches");
        browse.add_output::<UpdateID>("UpdateID");
        let mut sort_capabilities = Action::empty("SortCapabilities");
        sort_capabilities.add_output::<SortCapabilities>("SortCaps");
        let mut system_update_id = Action::empty("GetSystemUpdateID");
        system_update_id.add_output::<SystemUpdateId>("Id");
        let mut search_capabilities = Action::empty("GetSearchCapabilities");
        search_capabilities.add_output::<SearchCapabilities>("SearchCaps");

        vec![
            browse,
            sort_capabilities,
            system_update_id,
            search_capabilities,
        ]
    }

    async fn control_handler(
        &mut self,
        action: super::action::ActionPayload,
    ) -> anyhow::Result<impl IntoArgumentList> {
        tracing::debug!("Got action: {name}", name = action.name());
        match action.name() {
            "Browse" => {
                let arg_object_id = action.find_argument("ObjectID")?;
                let browse_flag = action.find_argument("BrowseFlag")?;
                let arg_filter = action.find_argument("Filter")?;
                let arg_index = action.find_argument("StartingIndex")?;
                let arg_count = action.find_argument("RequestedCount")?;
                let arg_sort_criteria = action.find_argument("SortCriteria")?;

                tracing::debug!(
                    "Invoked browse action with arguments {}, {}, {}, {}, {}, {}",
                    arg_object_id,
                    browse_flag,
                    arg_filter,
                    arg_index,
                    arg_count,
                    arg_sort_criteria,
                );
                self.browse(
                    arg_object_id,
                    browse_flag,
                    arg_filter,
                    arg_index,
                    arg_count,
                    arg_sort_criteria,
                )
                .await
            }
            rest => Err(anyhow::anyhow!("unhandled action: {rest}")),
        }
    }
}

pub mod properties {
    use std::{any::TypeId, collections::HashMap};

    use quick_xml::{
        events::{BytesDecl, BytesStart, BytesText, Event},
        Writer,
    };
    use upnp_class::ItemType;

    use crate::upnp::IntoXml;

    use super::Resource;

    macro_rules! impl_basic_property {
        ($name:literal for $type:ident) => {
            impl ObjectProperty for $type {}
            impl IntoXml for $type {
                fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
                    use super::service_variables::IntoUpnpValue;
                    use quick_xml::events::BytesText;

                    let el = &self.0.into_value().to_string();
                    w.create_element($name)
                        .write_text_content(BytesText::new(el))?;
                    Ok(())
                }
            }
        };
        ($name:literal for multivalue $type:ident) => {
            impl ObjectProperty for $type {
                const MULTIVALUE: bool = true;
            }
            impl IntoXml for $type {
                fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
                    use super::service_variables::IntoUpnpValue;
                    use quick_xml::events::BytesText;

                    let el = &self.0.into_value().to_string();
                    w.create_element($name)
                        .write_text_content(BytesText::new(el))?;
                    Ok(())
                }
            }
        };
        (container only $name:literal for $type:ident) => {
            impl ContainerProperty for $type {}
            impl IntoXml for $type {
                fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
                    use super::service_variables::IntoUpnpValue;
                    use quick_xml::events::BytesText;

                    let el = &self.0.into_value().to_string();
                    w.create_element($name)
                        .write_text_content(BytesText::new(el))?;
                    Ok(())
                }
            }
        };
        (container only $name:literal for multivalue $type:ident) => {
            impl ContainerProperty for $type {
                const MULTIVALUE: bool = true;
            }
            impl IntoXml for $type {
                fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
                    use super::service_variables::IntoUpnpValue;
                    use quick_xml::events::BytesText;

                    let el = &self.0.into_value().to_string();
                    w.create_element($name)
                        .write_text_content(BytesText::new(el))?;
                    Ok(())
                }
            }
        };
        (item only $name:literal for $type:ident) => {
            impl ItemProperty for $type {}
            impl IntoXml for $type {
                fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
                    use super::service_variables::IntoUpnpValue;
                    use quick_xml::events::BytesText;

                    let el = &self.0.into_value().to_string();
                    w.create_element($name)
                        .write_text_content(BytesText::new(el))?;
                    Ok(())
                }
            }
        };
        (item only $name:literal for multivalue $type:ident) => {
            impl ItemProperty for $type {
                const MULTIVALUE: bool = true;
            }
            impl IntoXml for $type {
                fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
                    use super::service_variables::IntoUpnpValue;
                    use quick_xml::events::BytesText;

                    let el = &self.0.into_value().to_string();
                    w.create_element($name)
                        .write_text_content(BytesText::new(el))?;
                    Ok(())
                }
            }
        };
    }

    pub mod upnp_class {

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
                    UpnpClass::Container(None) => "object.container",
                    UpnpClass::Container(Some(ContainerType::Album(None))) => {
                        "object.container.album"
                    }
                    UpnpClass::Container(Some(ContainerType::Album(Some(
                        AlbumType::MusicAlbum,
                    )))) => "object.container.album.musicAlbum",
                    UpnpClass::Container(Some(ContainerType::Album(Some(
                        AlbumType::PhotoAlbum,
                    )))) => "object.container.album.photoAlbum",

                    UpnpClass::Container(Some(ContainerType::Genre(None))) => {
                        "object.container.genre"
                    }
                    UpnpClass::Container(Some(ContainerType::Genre(Some(
                        GenreType::MusicGenre,
                    )))) => "object.container.genre.musicGenre",
                    UpnpClass::Container(Some(ContainerType::Genre(Some(
                        GenreType::MovieGenre,
                    )))) => "object.container.genre.movieGenre",

                    UpnpClass::Container(Some(ContainerType::Person(None))) => {
                        "object.container.person"
                    }
                    UpnpClass::Container(Some(ContainerType::Person(Some(
                        PersonType::MusicArtist,
                    )))) => "object.container.person.musicArtist",

                    UpnpClass::Container(Some(ContainerType::ChannelGroup(None))) => {
                        "object.container.channelGroup"
                    }
                    UpnpClass::Container(Some(ContainerType::ChannelGroup(Some(
                        ChannelGroupType::AudioChannelGroup,
                    )))) => "object.container.channelGroup.audioChannelGroup",
                    UpnpClass::Container(Some(ContainerType::ChannelGroup(Some(
                        ChannelGroupType::VideoChannelGroup,
                    )))) => "object.container.channelGroup.videoChannelGroup",

                    UpnpClass::Container(Some(ContainerType::PlaylistContainer)) => {
                        "object.container.playlistContainer"
                    }

                    UpnpClass::Container(Some(ContainerType::BookmarkFolder)) => {
                        "object.container.bookmarkFolder"
                    }

                    UpnpClass::Container(Some(ContainerType::StorageFolder)) => {
                        "object.container.storageFolder"
                    }

                    UpnpClass::Container(Some(ContainerType::StorageVolume)) => {
                        "object.container.storageVolume"
                    }

                    UpnpClass::Container(Some(ContainerType::StorageSystem)) => {
                        "object.container.storageSystem"
                    }

                    UpnpClass::Container(Some(ContainerType::EpgContainer)) => {
                        "object.container.epgContainer"
                    }

                    UpnpClass::Item(None) => "object.item",

                    UpnpClass::Item(Some(ItemType::VideoItem(None))) => "object.item.videoItem",
                    UpnpClass::Item(Some(ItemType::VideoItem(Some(VideoItemType::Movie)))) => {
                        "object.item.videoItem.movie"
                    }
                    UpnpClass::Item(Some(ItemType::VideoItem(Some(
                        VideoItemType::VideoBroadcast,
                    )))) => "object.item.videoItem.videoBroadcast",
                    UpnpClass::Item(Some(ItemType::VideoItem(Some(
                        VideoItemType::MusicVideoClip,
                    )))) => "object.item.videoItem.musicVideoClip",

                    UpnpClass::Item(Some(ItemType::AudioItem(None))) => "object.item.audioItem",
                    UpnpClass::Item(Some(ItemType::AudioItem(Some(AudioItemType::AudioBook)))) => {
                        "object.item.audioItem.audioBook"
                    }
                    UpnpClass::Item(Some(ItemType::AudioItem(Some(AudioItemType::MusicTrack)))) => {
                        "object.item.audioItem.musicTrack"
                    }
                    UpnpClass::Item(Some(ItemType::AudioItem(Some(
                        AudioItemType::AudioBroadcast,
                    )))) => "object.item.audioItem.audioBroadcast",

                    UpnpClass::Item(Some(ItemType::ImageItem(None))) => "object.item.imageItem",
                    UpnpClass::Item(Some(ItemType::ImageItem(Some(ImageItemType::Photo)))) => {
                        "object.item.imageItem.photo"
                    }

                    UpnpClass::Item(Some(ItemType::TextItem)) => "object.item.textItem",

                    UpnpClass::Item(Some(ItemType::PlaylistItem)) => "object.item.playlistItem",

                    UpnpClass::Item(Some(ItemType::BookmarkItem)) => "object.item.bookmarkItem",

                    UpnpClass::VendorDefined(s) => s,
                }
            }
        }
    }

    /// Marker trait for object property
    pub trait ObjectProperty {
        const MULTIVALUE: bool = false;
    }
    /// Marker trait to restrict property only for containers
    pub trait ContainerProperty {
        const MULTIVALUE: bool = false;
    }
    /// Marker trait to restrict property only for items
    pub trait ItemProperty {
        const MULTIVALUE: bool = false;
    }

    impl<T: ObjectProperty> ContainerProperty for T {
        const MULTIVALUE: bool = T::MULTIVALUE;
    }
    impl<T: ObjectProperty> ItemProperty for T {
        const MULTIVALUE: bool = T::MULTIVALUE;
    }

    pub struct ItemBase {
        parent_id: String,
        id: String,
        restricted: bool,
        dc_title: String,
        upnp_class: Option<upnp_class::ItemType>,
        ref_id: Option<String>,
    }

    impl ItemBase {
        pub fn new(id: String, parent_id: String, title: String) -> Self {
            Self {
                parent_id,
                id,
                dc_title: title,
                restricted: true,
                upnp_class: None,
                ref_id: None,
            }
        }

        pub fn set_upnp_class(&mut self, upnp_class: Option<upnp_class::ItemType>) {
            self.upnp_class = upnp_class;
        }

        pub fn set_restricted(&mut self, restricted: bool) {
            self.restricted = restricted;
        }
    }

    pub struct Item {
        base: ItemBase,
        properties: HashMap<TypeId, Box<dyn IntoXml>>,
        multivalue_properties: HashMap<TypeId, Vec<Box<dyn IntoXml>>>,
    }

    impl Item {
        pub fn new(id: String, parent_id: String, title: String) -> Self {
            Self {
                base: ItemBase::new(id, parent_id, title),
                properties: HashMap::new(),
                multivalue_properties: HashMap::new(),
            }
        }

        fn set_property<T>(&mut self, p: T)
        where
            T: ItemProperty + IntoXml + 'static,
        {
            let type_id = std::any::TypeId::of::<T>();
            let value = Box::new(p);
            if T::MULTIVALUE {
                let entry = self.multivalue_properties.entry(type_id);
                match entry {
                    std::collections::hash_map::Entry::Occupied(mut occupied_entry) => {
                        occupied_entry.get_mut().push(value)
                    }
                    std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(vec![value]);
                    }
                }
            } else {
                self.properties.insert(type_id, value);
            }
        }

        pub fn unset_property<T>(&mut self)
        where
            T: ItemProperty + IntoXml + 'static,
        {
            let type_id = std::any::TypeId::of::<T>();
            if T::MULTIVALUE {
                self.multivalue_properties.remove(&type_id);
            } else {
                self.properties.remove(&type_id);
            }
        }
    }

    impl IntoXml for Item {
        fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
            let item_tag = BytesStart::new("item").with_attributes([
                ("id", self.base.id.as_str()),
                ("parentID", self.base.parent_id.as_str()),
                ("restricted", if self.base.restricted { "1" } else { "0" }),
            ]);

            let item_tag_end = item_tag.to_end().into_owned();
            w.write_event(Event::Start(item_tag))?;
            w.create_element("dc:title")
                .write_text_content(BytesText::new(&self.base.dc_title))?;
            w.create_element("upnp:class")
                .write_text_content(BytesText::new(
                    &upnp_class::UpnpClass::Item(self.base.upnp_class).as_str(),
                ))?;
            for property in self.properties.values() {
                property.write_xml(w)?;
            }
            for multi_value_property in self.multivalue_properties.values().flatten() {
                multi_value_property.write_xml(w)?;
            }

            w.write_event(Event::End(item_tag_end))?;
            Ok(())
        }
    }

    pub struct Container {
        base: ContainerBase,
        properties: HashMap<TypeId, Box<dyn IntoXml>>,
        multivalue_properties: HashMap<TypeId, Vec<Box<dyn IntoXml>>>,
    }

    pub struct ContainerBase {
        parent_id: String,
        id: String,
        restricted: bool,
        dc_title: String,
        upnp_class: Option<upnp_class::ContainerType>,
        searchable: Option<bool>,
        child_count: Option<usize>,
    }

    impl ContainerBase {
        pub fn new(id: String, parent_id: String, title: String) -> Self {
            Self {
                parent_id,
                id,
                restricted: true,
                dc_title: title,
                upnp_class: None,
                searchable: None,
                child_count: None,
            }
        }

        pub fn set_restricted(&mut self, restricted: bool) {
            self.restricted = restricted;
        }

        pub fn set_child_count(&mut self, child_count: Option<usize>) {
            self.child_count = child_count;
        }

        pub fn set_searchable(&mut self, searchable: Option<bool>) {
            self.searchable = searchable;
        }

        pub fn set_upnp_class(&mut self, upnp_class: upnp_class::ContainerType) {
            self.upnp_class = Some(upnp_class);
        }
    }

    impl Container {
        pub fn new(id: String, parent_id: String, title: String) -> Self {
            Self {
                base: ContainerBase::new(id, parent_id, title),
                properties: HashMap::new(),
                multivalue_properties: HashMap::new(),
            }
        }

        fn set_property<T>(&mut self, p: T)
        where
            T: ContainerProperty + IntoXml + 'static,
        {
            let type_id = std::any::TypeId::of::<T>();
            let value = Box::new(p);
            if T::MULTIVALUE {
                let entry = self.multivalue_properties.entry(type_id);
                match entry {
                    std::collections::hash_map::Entry::Occupied(mut occupied_entry) => {
                        occupied_entry.get_mut().push(value)
                    }
                    std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(vec![value]);
                    }
                }
            } else {
                self.properties.insert(type_id, value);
            }
        }

        pub fn unset_property<T>(&mut self)
        where
            T: ContainerProperty + IntoXml + 'static,
        {
            let type_id = std::any::TypeId::of::<T>();
            if T::MULTIVALUE {
                self.multivalue_properties.remove(&type_id);
            } else {
                self.properties.remove(&type_id);
            }
        }
    }

    impl IntoXml for Container {
        fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
            let mut container_tag = BytesStart::new("container").with_attributes([
                ("id", self.base.id.as_str()),
                ("parentID", self.base.parent_id.as_str()),
                ("restricted", if self.base.restricted { "1" } else { "0" }),
            ]);

            container_tag.extend_attributes(
                self.base
                    .searchable
                    .map(|x| ("searchable", if x { "1" } else { "0" })),
            );
            let child_count = self.base.child_count.map(|x| x.to_string());
            container_tag
                .extend_attributes(child_count.as_ref().map(|x| ("childCount", x.as_str())));

            let container_tag_end = container_tag.to_end().into_owned();
            w.write_event(Event::Start(container_tag))?;
            w.create_element("dc:title")
                .write_text_content(BytesText::new(&self.base.dc_title))?;
            w.create_element("upnp:class")
                .write_text_content(BytesText::new(
                    &upnp_class::UpnpClass::Container(self.base.upnp_class).as_str(),
                ))?;
            for property in self.properties.values() {
                property.write_xml(w)?;
            }
            for multivalue_property in self.multivalue_properties.values().flatten() {
                multivalue_property.write_xml(w)?;
            }

            w.write_event(Event::End(container_tag_end))?;
            Ok(())
        }
    }

    #[derive(Debug, Clone)]
    pub struct AlbumArtUri(pub String);
    impl_basic_property!("upnp:albumArtURI" for multivalue AlbumArtUri);

    #[derive(Debug, Clone, Copy)]
    pub struct StorageTotal(pub u64);
    impl_basic_property!(container only "upnp:storageTotal" for StorageTotal);

    impl ObjectProperty for Resource {
        const MULTIVALUE: bool = true;
    }

    pub struct DidlResponse {
        containers: Vec<Container>,
        items: Vec<Item>,
    }

    impl DidlResponse {
        pub fn len(&self) -> usize {
            self.items.len() + self.containers.len()
        }

        pub fn into_xml(&self) -> anyhow::Result<String> {
            let mut w = Writer::new(Vec::new());
            w.write_event(Event::Decl(BytesDecl::new("1.0", None, None)))?;
            let didl = BytesStart::new("DIDL-Lite").with_attributes([
            ("xmlns:dc", "http://purl.org/dc/elements/1.1/"),
            ("xmlns", "urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/"),
            ("xmlns:upnp", "urn:schemas-upnp-org:metadata-1-0/upnp/"),
            ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
            (
                "xsi:schemaLocation",
                r#"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/ http://www.upnp.org/schemas/av/didl-lite.xsd urn:schemas-upnp-org:metadata-1-0/upnp/ http://www.upnp.org/schemas/av/upnp.xsd"#,
            ),
        ]);
            let didl_end = didl.to_end().into_owned();
            w.write_event(Event::Start(didl))?;

            for object in &self.containers {
                object.write_xml(&mut w)?;
            }

            for object in &self.items {
                object.write_xml(&mut w)?;
            }

            w.write_event(Event::End(didl_end))?;

            Ok(String::from_utf8(w.into_inner())?)
        }

        pub fn root() -> Self {
            let shows = Container::new("shows".into(), 0.to_string(), "Shows".into());
            let movies = Container::new("movies".into(), 0.to_string(), "Movies".into());
            Self {
                containers: vec![shows, movies],
                items: vec![],
            }
        }
    }
}

pub mod res {
    use quick_xml::events::BytesText;

    use crate::upnp::IntoXml;

    #[derive(Debug)]
    struct ResourceProperty {
        key: &'static str,
        value: String,
    }

    #[derive(Debug)]
    pub struct Resource {
        uri: String,
        properties: Vec<ResourceProperty>,
    }

    impl Resource {
        pub fn new(uri: String, protocol_info: String) -> Self {
            Self {
                uri,
                properties: vec![ResourceProperty {
                    key: "protocolInfo",
                    value: protocol_info,
                }],
            }
        }
    }

    impl IntoXml for Resource {
        fn write_xml(&self, w: &mut crate::upnp::XmlWriter) -> quick_xml::Result<()> {
            w.create_element("res")
                .with_attributes(self.properties.iter().map(|v| (v.key, v.value.as_str())))
                .write_text_content(BytesText::new(&self.uri))?;
            Ok(())
        }
    }
}

#[derive(Debug)]
pub struct Resource {
    uri: String,
    protocol_info: String,
    import_uri: Option<String>,
    /// The size in bytes of the resource.
    size: Option<u64>,
    duration: Option<UpnpDuration>,
    protection: Option<String>,
    bitrate: Option<usize>,
    bits_per_sample: Option<usize>,
    sample_frequency: Option<usize>,
    nr_audio_channels: Option<usize>,
    resolution: Option<UpnpResolution>,
    color_depth: Option<usize>,
    tspec: Option<String>,
    allowed_use: Option<String>,
    validity_start: Option<String>,
    validity_end: Option<String>,
    remaining_time: Option<String>,
    usage_info: Option<String>,
    rights_info_uri: Option<String>,
    content_info_uri: Option<String>,
    record_quality: Option<String>,
    daylight_saving: Option<String>,
    framerate: Option<UpnpFramerate>,
}

#[derive(Debug)]
struct UpnpDuration(std::time::Duration);

impl UpnpDuration {
    pub fn new(duration: std::time::Duration) -> Self {
        Self(duration)
    }
}

impl FromStr for UpnpDuration {
    type Err = anyhow::Error;

    fn from_str(mut s: &str) -> Result<Self, Self::Err> {
        if let Some(stripped) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) {
            s = stripped;
        };
        let mut parts = s.split(':');
        let hours: u64 = parts
            .next()
            .context("get hours")
            .and_then(|h| h.parse().context("parse hours number"))?;
        let minutes = parts.next().context("get minutes")?;
        anyhow::ensure!(minutes.len() == 2);
        let minutes: u64 = minutes.parse().context("parse minutes number")?;

        let seconds_part = parts.next().context("get seconds part")?;
        let (seconds, fraction) = seconds_part.split_at_checked(2).context("split seconds")?;
        anyhow::ensure!(seconds.len() == 2);
        let seconds: u64 = seconds.parse().context("parse seconds")?;

        let total_seconds = hours * 60 * 60 + minutes * 60 + seconds;

        let duration = match fraction.is_empty() {
            true => std::time::Duration::from_secs(total_seconds),
            false => {
                let fraction = if let Some((fraction, of)) = fraction.split_once('/') {
                    let fraction: f32 = fraction
                        .strip_prefix('.')
                        .context("strip fraction")?
                        .parse()
                        .context("parse fraction")?;
                    let of: f32 = of.parse().context("parse fraction")?;
                    fraction / of
                } else {
                    fraction.parse().context("parse fraction")?
                };

                std::time::Duration::from_secs_f32(total_seconds as f32 + fraction)
            }
        };
        Ok(UpnpDuration(duration))
    }
}

impl Display for UpnpDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let format_time = |duration: std::time::Duration| {
            let millis = duration.as_millis();
            let seconds = millis / 1000;
            let minutes = seconds / 60;
            let hours = minutes / 60;
            let without_fractions = format!("{}:{:0>2}:{:0>2}", hours, minutes % 60, seconds % 60);
            let millis = millis % 1000;
            if millis == 0 {
                without_fractions
            } else {
                format!("{without_fractions}.{millis}")
            }
        };
        write!(f, "{}", format_time(self.0))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UpnpResolution {
    width: usize,
    height: usize,
}

impl UpnpResolution {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }
}

impl Display for UpnpResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{width}x{height}",
            width = self.width,
            height = self.height
        )
    }
}

impl FromStr for UpnpResolution {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (width, height) = s
            .split_once('x')
            .context("split resolution by 'x'")
            .and_then(|(width, height)| {
                Ok((
                    width.parse().context("parse width")?,
                    height.parse().context("parse height")?,
                ))
            })?;
        Ok(Self { width, height })
    }
}

#[derive(Debug)]
pub struct UpnpFramerate {
    scanning: Scanning,
    framerate: f32,
}

#[derive(Debug)]
pub enum Scanning {
    Progressive,
    Interlaced,
}

impl UpnpFramerate {
    pub fn new(framerate: f32, scanning: Scanning) -> Self {
        Self {
            framerate,
            scanning,
        }
    }
}

impl Display for UpnpFramerate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scanning_letter = match self.scanning {
            Scanning::Interlaced => "i",
            Scanning::Progressive => "p",
        };
        write!(f, "{}{scanning_letter}", self.framerate)
    }
}

impl FromStr for UpnpFramerate {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(number) = s.strip_suffix('p') {
            return Ok(Self {
                scanning: Scanning::Progressive,
                framerate: number.parse()?,
            });
        };
        if let Some(number) = s.strip_suffix('i') {
            return Ok(Self {
                scanning: Scanning::Interlaced,
                framerate: number.parse()?,
            });
        }
        Err(anyhow::anyhow!("framerate must end with `i` or `p`"))
    }
}

impl IntoXml for Resource {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let mut attributes = Vec::new();
        attributes.push(("protocolInfo", self.protocol_info.to_owned()));
        if let Some(import_uri) = &self.import_uri {
            attributes.push(("importUri", import_uri.to_owned()));
        }
        if let Some(size) = self.size.map(|s| s.to_string()) {
            attributes.push(("size", size));
        }
        if let Some(duration) = &self.duration {
            attributes.push(("duration", duration.to_string()));
        }
        if let Some(protection) = &self.protection {
            attributes.push(("protection", protection.to_owned()));
        }
        if let Some(bitrate) = self.bitrate {
            attributes.push(("bitrate", bitrate.to_string()));
        }
        if let Some(bits_per_sample) = self.bits_per_sample {
            attributes.push(("bitsPerSample", bits_per_sample.to_string()));
        }
        if let Some(sample_frequency) = self.sample_frequency {
            attributes.push(("sampleFrequency", sample_frequency.to_string()));
        }
        if let Some(nr_audio_channels) = self.nr_audio_channels {
            attributes.push(("nrAudioChannels", nr_audio_channels.to_string()));
        }
        if let Some(resolution) = &self.resolution {
            attributes.push(("resolution", resolution.to_string()));
        }
        if let Some(color_depth) = self.color_depth {
            attributes.push(("colorDepth", color_depth.to_string()));
        }
        if let Some(tspec) = &self.tspec {
            attributes.push(("tspec", tspec.to_owned()));
        }
        if let Some(allowed_use) = &self.allowed_use {
            attributes.push(("allowedUse", allowed_use.to_owned()));
        }
        if let Some(validity_start) = &self.validity_start {
            attributes.push(("validityStart", validity_start.to_owned()));
        }
        if let Some(validity_end) = &self.validity_end {
            attributes.push(("validityEnd", validity_end.to_owned()));
        }
        if let Some(remaining_time) = &self.remaining_time {
            attributes.push(("remainingTime", remaining_time.to_owned()));
        }
        if let Some(usage_info) = &self.usage_info {
            attributes.push(("usageInfo", usage_info.to_owned()));
        }
        if let Some(rights_info_uri) = &self.rights_info_uri {
            attributes.push(("rightsInfoUri", rights_info_uri.to_owned()));
        }
        if let Some(content_info_uri) = &self.content_info_uri {
            attributes.push(("contentInfoUri", content_info_uri.to_owned()));
        }
        if let Some(record_quality) = &self.record_quality {
            attributes.push(("recordQuality", record_quality.to_owned()));
        }
        if let Some(daylight_saving) = &self.daylight_saving {
            attributes.push(("daylightSaving", daylight_saving.to_owned()));
        }
        if let Some(framerate) = &self.framerate {
            attributes.push(("framerate", framerate.to_string()));
        }
        w.create_element("res")
            .with_attributes(attributes.iter().map(|(k, v)| (*k, v.as_str())))
            .write_text_content(BytesText::new(&self.uri))?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub enum WriteStatus {
    /// The object’s resource(s) may be deleted and/or modified
    Writable,
    /// The object’s resource(s) shall not be deleted and/or modified.
    Protected,
    /// The object’s resource(s) shall not be modified.
    NotWritable,
    /// The object’s resource(s) write status is unknown.
    #[default]
    Unknown,
    /// Some of the object’s resource(s) have a different write status.
    Mixed,
}

impl Display for WriteStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteStatus::Writable => write!(f, "WRITABLE"),
            WriteStatus::Protected => write!(f, "PROTECTED"),
            WriteStatus::NotWritable => write!(f, "NOT WRITABLE"),
            WriteStatus::Unknown => write!(f, "UNKNOWN"),
            WriteStatus::Mixed => write!(f, "MIXED"),
        }
    }
}

impl FromStr for WriteStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "WRITABLE" => Ok(Self::Writable),
            "PROTECTED" => Ok(Self::Protected),
            "NOT WRITABLE" => Ok(Self::NotWritable),
            "UNKNOWN" => Ok(Self::Unknown),
            "MIXED" => Ok(Self::Mixed),
            _ => Err(anyhow::anyhow!("unknown write_status: {s}")),
        }
    }
}

#[derive(Debug)]
pub struct SearchClass {
    class: String,
    name: Option<String>,
    include_derived: bool,
}

impl IntoXml for SearchClass {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let mut attributes = Vec::new();
        attributes.push((
            "includeDerived",
            if self.include_derived { "1" } else { "0" },
        ));

        if let Some(name) = &self.name {
            attributes.push(("name", name));
        };

        w.create_element("upnp:searchClass")
            .with_attributes(attributes)
            .write_text_content(BytesText::new(&self.class))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct CreateClass {
    class: String,
    include_derived: bool,
}

impl IntoXml for CreateClass {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let mut attributes = Vec::new();
        attributes.push((
            "includeDerived",
            if self.include_derived { "1" } else { "0" },
        ));

        w.create_element("upnp:createClass")
            .with_attributes(attributes)
            .write_text_content(BytesText::new(&self.class))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct StorageFolder {
    storage_used: u64,
}

impl IntoXml for StorageFolder {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        w.create_element("upnp:storageUsed")
            .write_text_content(BytesText::new(&self.storage_used.to_string()))?;
        Ok(())
    }
}
