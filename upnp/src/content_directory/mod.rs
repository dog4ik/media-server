use std::{
    any::TypeId,
    collections::{hash_map::Entry, HashMap},
    fmt::Display,
    str::FromStr,
};

use anyhow::Context;
use property_name::PropertyValue;
use quick_xml::events::{BytesStart, BytesText, Event};

use crate::{
    action::{ActionError, IntoValueList},
    service::ArgumentScanner,
};

use super::{
    action::Action,
    service::Service,
    service_variables::{self, IntoUpnpValue, SVariable, StateVariableDescriptor},
    templates::{service_description::ServiceDescription, SpecVersion},
    urn::{ServiceType, UrnType, URN},
    IntoXml, XmlWriter,
};

pub mod class;
pub mod error;
pub mod properties;

pub trait ContentDirectoryHandler {
    fn browse_direct_children(
        &self,
        object_id: &str,
        requested_count: u32,
    ) -> impl std::future::Future<Output = Result<properties::DidlResponse, ActionError>> + Send;
    fn browse_metadata(
        &self,
        object_id: &str,
    ) -> impl std::future::Future<Output = Result<properties::DidlResponse, ActionError>> + Send;
    fn system_update_id(&self) -> impl std::future::Future<Output = u32> + Send;
}

#[derive(Debug, Clone)]
pub struct ContentDirectoryService<T: ContentDirectoryHandler> {
    pub handler: T,
}

impl<T: ContentDirectoryHandler> ContentDirectoryService<T> {
    pub fn new(handler: T) -> Self {
        Self { handler }
    }
}

impl<T: ContentDirectoryHandler> ContentDirectoryService<T> {
    async fn browse(
        &self,
        object_id: String,
        browse_flag: BrowseFlag,
        filter: filter::Filter,
        start_index: u32,
        requested_count: u32,
        sort_criteria: String,
    ) -> anyhow::Result<(String, u32, u32, u32)> {
        let update_id = self.handler.system_update_id().await;
        tracing::debug!(
            object_id,
            %browse_flag,
            %filter,
            start_index,
            requested_count,
            sort_criteria,
            "Invoking browse action"
        );
        let mut result = match browse_flag {
            BrowseFlag::BrowseDirectChildren => {
                self.handler
                    .browse_direct_children(object_id.as_ref(), requested_count)
                    .await?
            }
            BrowseFlag::BrowseMetadata => self.handler.browse_metadata(object_id.as_ref()).await?,
        };
        result.apply_filter(filter);
        let number_returned = result.len();
        let total_matches = result.len();
        let result = result.into_xml().unwrap();
        Ok((
            result,
            number_returned as u32,
            total_matches as u32,
            update_id,
        ))
    }
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

impl IntoXml for BrowseFlag {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for BrowseFlag {
    const TYPE_NAME: service_variables::DataType = service_variables::DataType::String;

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

mod property_name {
    use std::fmt::Debug;

    use crate::IntoXml;

    use super::filter::{FilterType, PropertyFilter};

    #[derive(Debug)]
    pub struct PropertyValue {
        pub ns: Option<&'static str>,
        pub name: &'static str,
        /// Is current property filtered? Should be always true for the required properties
        pub is_allowed: bool,
        pub value: ValueType,
        pub dependant_properties: Vec<DependantProperty>,
    }

    #[derive(Debug)]
    pub struct DependantProperty {
        pub name: &'static str,
        pub value: String,
        pub is_allowed: bool,
    }

    impl DependantProperty {
        pub fn new(name: &'static str, value: String, is_allowed: bool) -> Self {
            Self {
                name,
                value,
                is_allowed,
            }
        }
        pub fn new_required(name: &'static str, value: String) -> Self {
            Self::new(name, value, true)
        }
        pub fn new_optional(name: &'static str, value: String) -> Self {
            Self::new(name, value, false)
        }
    }

    pub enum ValueType {
        Value(Box<dyn IntoXml + Send>),
        NestedProperties(Vec<PropertyValue>),
    }

    impl ValueType {
        pub fn basic(value: impl IntoXml + 'static + Send) -> Self {
            Self::Value(Box::new(value))
        }
    }

    impl Debug for ValueType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ValueType::Value(_) => write!(f, "Value"),
                ValueType::NestedProperties(p) => write!(f, "{:?}", p),
            }
        }
    }

    impl IntoXml for ValueType {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            match self {
                Self::Value(v) => v.write_xml(w),
                Self::NestedProperties(properties) => {
                    for property in properties {
                        property.write_xml(w)?;
                    }
                    Ok(())
                }
            }
        }
    }

    impl IntoXml for PropertyValue {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            if !self.is_allowed {
                return Ok(());
            }
            let full_name = self.ns.map_or(std::borrow::Cow::Borrowed(self.name), |ns| {
                std::borrow::Cow::Owned(ns.to_owned() + ":" + self.name)
            });
            w.create_element(full_name)
                .with_attributes(self.dependant_properties.iter().filter_map(|property| {
                    property
                        .is_allowed
                        .then_some((property.name, property.value.as_str()))
                }))
                .write_inner_content(|w| self.value.write_xml(w))?;
            Ok(())
        }
    }

    impl PropertyValue {
        pub fn apply_filter(&mut self, filter: &PropertyFilter, path_idx: usize) {
            if filter.property_path[path_idx] != self.name {
                return;
            };
            let is_target = path_idx == filter.property_path.len() - 1;
            if !is_target {
                self.is_allowed = true;
                return;
            }
            match &filter.filter_type {
                FilterType::BasicPropertyFilter => {
                    self.is_allowed = true;
                }
                FilterType::Wildcard => {
                    for attribute in &mut self.dependant_properties {
                        attribute.is_allowed = true;
                    }
                    match &mut self.value {
                        ValueType::Value(_) => {}
                        ValueType::NestedProperties(nested_properties) => {
                            if path_idx + 1 != filter.property_path.len() {
                                for nested_property in nested_properties {
                                    nested_property.apply_filter(&filter, path_idx + 1);
                                }
                            }
                        }
                    }
                }
                FilterType::AttributeFilter(filtered_attr) => {
                    for attribute in &mut self.dependant_properties {
                        if attribute.name == filtered_attr {
                            attribute.is_allowed = true;
                        }
                    }
                }
            }
        }

        pub fn allow_all(&mut self) {
            self.is_allowed = true;
            for property in &mut self.dependant_properties {
                property.is_allowed = true;
            }
            match &mut self.value {
                ValueType::Value(_) => {}
                ValueType::NestedProperties(properties) => {
                    for property in properties {
                        property.is_allowed = true;
                        property.allow_all();
                    }
                }
            }
        }
    }
}

mod filter {
    use std::fmt::Display;

    use quick_xml::events::{BytesText, Event};

    use crate::{
        service_variables::{IntoUpnpValue, SVariable},
        IntoXml, XmlWriter,
    };

    #[derive(Debug)]
    pub enum FilterType {
        BasicPropertyFilter,
        Wildcard,
        AttributeFilter(String),
    }

    #[derive(Debug)]
    pub struct PropertyFilter {
        pub filter_type: FilterType,
        pub property_path: Vec<String>,
    }

    impl Display for PropertyFilter {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.property_path.join("::"))?;
            match &self.filter_type {
                FilterType::BasicPropertyFilter => Ok(()),
                FilterType::Wildcard => write!(f, "#"),
                FilterType::AttributeFilter(attr) => write!(f, "@{attr}"),
            }
        }
    }

    impl PropertyFilter {
        pub fn from_filter_part(raw_filter: &str) -> PropertyFilter {
            if let Some(property) = raw_filter.strip_suffix('#') {
                let property_path = property.split("::").map(|p| p.to_owned()).collect();
                return PropertyFilter {
                    property_path,
                    filter_type: FilterType::Wildcard,
                };
            }
            if let Some((property, attribute)) = raw_filter.split_once('@') {
                let property_path = property.split("::").map(|p| p.to_owned()).collect();
                return PropertyFilter {
                    property_path,
                    filter_type: FilterType::AttributeFilter(attribute.to_owned()),
                };
            }
            let property_path = raw_filter.split("::").map(|p| p.to_owned()).collect();
            PropertyFilter {
                property_path,
                filter_type: FilterType::BasicPropertyFilter,
            }
        }
    }

    #[derive(Default, Debug)]
    pub enum Filter {
        #[default]
        Wildcard,
        Allowed(Vec<PropertyFilter>),
    }

    impl Filter {
        pub fn new(raw_filter: &str) -> Filter {
            match raw_filter {
                "*" => Filter::Wildcard,
                _ => {
                    let properties = raw_filter
                        .split(',')
                        .map(|p| PropertyFilter::from_filter_part(p))
                        .collect();
                    Filter::Allowed(properties)
                }
            }
        }
    }

    impl IntoXml for Filter {
        fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
            match self {
                Filter::Wildcard => w.write_event(Event::Text(BytesText::from_escaped("*"))),
                Filter::Allowed(properties) => {
                    if !properties.is_empty() {
                        for property in &properties[..properties.len() - 1] {
                            w.write_event(Event::Text(BytesText::new(
                                &property.property_path.join("::"),
                            )))?;
                            w.write_event(Event::Text(BytesText::from_escaped(",")))?;
                        }
                        w.write_event(Event::Text(BytesText::new(
                            &properties
                                .last()
                                .expect("non empty")
                                .property_path
                                .join("::"),
                        )))?;
                    }

                    Ok(())
                }
            }
        }
    }

    impl Display for Filter {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.into_string().unwrap())
        }
    }

    impl IntoUpnpValue for Filter {
        fn from_xml_value(value: &str) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            Ok(Self::new(value))
        }
    }

    impl SVariable for Filter {
        type VarType = String;
        const VAR_NAME: &str = "A_ARG_TYPE_Filter";
    }
}

mod feature_list {
    use quick_xml::events::{BytesStart, Event};

    use crate::{
        service_variables::{IntoUpnpValue, SVariable},
        IntoXml,
    };

    #[derive(Debug, Clone)]
    pub struct Feature {
        name: String,
        version: usize,
    }

    impl IntoXml for Feature {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            let version = self.version.to_string();
            let start = BytesStart::new("Feature")
                .with_attributes([("name", self.name.as_str()), ("version", version.as_str())]);
            let end = start.to_end().into_owned();
            w.write_event(Event::Start(start))?;
            w.write_event(Event::End(end))
        }
    }

    #[derive(Default, Debug)]
    pub struct FeatureList(Vec<Feature>);

    impl IntoXml for FeatureList {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            let start = BytesStart::new("FeatureList");
            let end = start.to_end().into_owned();
            w.write_event(Event::Start(start))?;
            for feature in &self.0 {
                feature.write_xml(w)?;
            }
            w.write_event(Event::End(end))
        }
    }

    impl IntoUpnpValue for FeatureList {
        const TYPE_NAME: crate::service_variables::DataType =
            crate::service_variables::DataType::String;

        fn from_xml_value(_value: &str) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            todo!()
        }
    }

    impl SVariable for FeatureList {
        type VarType = Self;
        const VAR_NAME: &str = "FeatureList";
    }
}

impl<T: ContentDirectoryHandler + Send + Sync + 'static> Service for ContentDirectoryService<T> {
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
            StateVariableDescriptor::from_variable::<filter::Filter>(),
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
        browse.add_input::<filter::Filter>("Filter");
        browse.add_input::<Index>("StartingIndex");
        browse.add_input::<Count>("RequestedCount");
        browse.add_input::<SortCriteria>("SortCriteria");
        browse.add_output::<ArgResult>("Result");
        browse.add_output::<Count>("NumberReturned");
        browse.add_output::<Count>("TotalMatches");
        browse.add_output::<UpdateID>("UpdateID");
        let mut sort_capabilities = Action::empty("GetSortCapabilities");
        sort_capabilities.add_output::<SortCapabilities>("SortCaps");
        let mut system_update_id = Action::empty("GetSystemUpdateID");
        system_update_id.add_output::<SystemUpdateId>("Id");
        let mut search_capabilities = Action::empty("GetSearchCapabilities");
        search_capabilities.add_output::<SearchCapabilities>("SearchCaps");
        let mut feature_list = Action::empty("GetFeatureList");
        feature_list.add_output::<feature_list::FeatureList>("SearchCaps");

        vec![
            browse,
            sort_capabilities,
            system_update_id,
            search_capabilities,
        ]
    }

    async fn control_handler<'a>(
        &self,
        name: &'a str,
        mut inputs: ArgumentScanner<'a>,
    ) -> anyhow::Result<impl IntoValueList> {
        tracing::debug!("Got action: {name}", name = name);
        let values = match name {
            "Browse" => {
                let browse_result = self
                    .browse(
                        inputs.next()?,
                        inputs.next()?,
                        inputs.next()?,
                        inputs.next()?,
                        inputs.next()?,
                        inputs.next()?,
                    )
                    .await?;
                browse_result.into_value_list()
            }
            "GetSortCapabilities" => {
                todo!()
            }
            "GetSearchCapabilities" => {
                todo!()
            }
            "GetSystemUpdateID" => self.handler.system_update_id().await.into_value_list(),
            rest => Err(anyhow::anyhow!("unhandled action: {rest}"))?,
        };
        Ok(values)
    }
}

/// Marker trait for object property
/// Object property can be attached to container or item.
pub trait ObjectProperty {
    const NAME: &str;
    const MULTIVALUE: bool = false;
}
/// Marker trait to restrict property only for containers
/// Container property can be attached only on containers.
pub trait ContainerProperty {
    const NAME: &str;
    const MULTIVALUE: bool = false;
}
/// Marker trait to restrict property only for items
/// Item property can be attached only on items.
pub trait ItemProperty {
    const NAME: &str;
    const MULTIVALUE: bool = false;
}

impl<T: ObjectProperty> ContainerProperty for T {
    const NAME: &str = T::NAME;
    const MULTIVALUE: bool = T::MULTIVALUE;
}
impl<T: ObjectProperty> ItemProperty for T {
    const NAME: &str = T::NAME;
    const MULTIVALUE: bool = T::MULTIVALUE;
}

#[derive(Debug)]
pub struct ItemBase {
    parent_id: String,
    id: String,
    restricted: bool,
    dc_title: String,
    class: Option<class::ItemType>,
    ref_id: Option<String>,
}

impl ItemBase {
    pub fn new(id: String, parent_id: String, title: String) -> Self {
        Self {
            parent_id,
            id,
            dc_title: title,
            restricted: true,
            class: None,
            ref_id: None,
        }
    }

    pub fn set_upnp_class(&mut self, upnp_class: impl Into<Option<class::ItemType>>) {
        self.class = upnp_class.into();
    }

    pub fn set_restricted(&mut self, restricted: bool) {
        self.restricted = restricted;
    }
}

#[derive(Debug)]
pub struct Item {
    pub base: ItemBase,
    properties: HashMap<TypeId, PropertyValue>,
    multivalue_properties: HashMap<TypeId, Vec<PropertyValue>>,
}

impl Item {
    pub fn new(id: String, parent_id: String, title: String) -> Self {
        Self {
            base: ItemBase::new(id, parent_id, title),
            properties: HashMap::new(),
            multivalue_properties: HashMap::new(),
        }
    }

    pub fn set_property<T>(&mut self, p: T)
    where
        T: ItemProperty + Into<PropertyValue> + 'static,
    {
        let type_id = std::any::TypeId::of::<T>();
        let value = p.into();
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
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let mut item_tag = BytesStart::new("item").with_attributes([
            ("id", self.base.id.as_str()),
            ("parentID", self.base.parent_id.as_str()),
            ("restricted", if self.base.restricted { "1" } else { "0" }),
        ]);

        if let Some(ref_id) = &self.base.ref_id {
            item_tag.extend_attributes([("refID", ref_id.as_str())]);
        };

        let item_tag_end = item_tag.to_end().into_owned();
        w.write_event(Event::Start(item_tag))?;
        w.create_element("dc:title")
            .write_text_content(BytesText::new(&self.base.dc_title))?;
        w.create_element("upnp:class")
            .write_text_content(BytesText::new(
                &class::UpnpClass::Item(self.base.class).as_str(),
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

#[derive(Debug)]
pub struct Container {
    pub base: ContainerBase,
    properties: HashMap<TypeId, PropertyValue>,
    multivalue_properties: HashMap<TypeId, Vec<PropertyValue>>,
}

#[derive(Debug)]
pub struct ContainerBase {
    parent_id: String,
    id: String,
    restricted: bool,
    dc_title: String,
    class: Option<class::ContainerType>,
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
            class: None,
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

    pub fn set_upnp_class(&mut self, upnp_class: Option<class::ContainerType>) {
        self.class = upnp_class;
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

    pub fn set_property<T>(&mut self, p: T)
    where
        T: ContainerProperty + Into<PropertyValue> + 'static,
    {
        let type_id = std::any::TypeId::of::<T>();
        let value = p.into();
        if T::MULTIVALUE {
            let entry = self.multivalue_properties.entry(type_id);
            match entry {
                Entry::Occupied(mut occupied_entry) => occupied_entry.get_mut().push(value),
                Entry::Vacant(vacant_entry) => {
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
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
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
        container_tag.extend_attributes(child_count.as_ref().map(|x| ("childCount", x.as_str())));

        let container_tag_end = container_tag.to_end().into_owned();
        w.write_event(Event::Start(container_tag))?;
        w.create_element("dc:title")
            .write_text_content(BytesText::new(&self.base.dc_title))?;
        w.create_element("upnp:class")
            .write_text_content(BytesText::new(
                &class::UpnpClass::Container(self.base.class).as_str(),
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

#[derive(Debug)]
pub struct UpnpDuration(pub std::time::Duration);

impl UpnpDuration {
    pub fn new(duration: std::time::Duration) -> Self {
        Self(duration)
    }
}

impl From<std::time::Duration> for UpnpDuration {
    fn from(value: std::time::Duration) -> Self {
        Self(value)
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
