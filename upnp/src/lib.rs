pub mod action;

#[allow(unused, unused_variables)]
pub mod connection_manager;
pub mod content_directory;
mod device_description;
#[allow(unused)]
mod eventing;
pub mod internet_gateway;
pub mod router;
pub mod search_client;
mod service;
pub mod service_client;
mod service_variables;
pub mod ssdp;
pub mod templates;
pub mod urn;

pub trait XmlReaderExt<'a> {
    fn read_event_err_eof(&mut self) -> anyhow::Result<quick_xml::events::Event<'a>>;
    fn read_to_start(&mut self) -> anyhow::Result<quick_xml::events::BytesStart<'a>>;
    fn read_end(&mut self) -> anyhow::Result<quick_xml::events::BytesEnd<'a>>;
    fn read_text(&mut self) -> anyhow::Result<quick_xml::events::BytesText<'a>>;
}

impl<'a> XmlReaderExt<'a> for quick_xml::Reader<&'a [u8]> {
    fn read_event_err_eof(&mut self) -> anyhow::Result<quick_xml::events::Event<'a>> {
        let event = self.read_event()?;
        match event {
            quick_xml::events::Event::Eof => Err(anyhow::anyhow!("early eof")),
            _ => Ok(event),
        }
    }
    fn read_to_start(&mut self) -> anyhow::Result<quick_xml::events::BytesStart<'a>> {
        loop {
            let event = self.read_event_err_eof()?.into_owned();
            match event {
                quick_xml::events::Event::Start(e) => break Ok(e),
                _ => (),
            }
        }
    }
    fn read_end(&mut self) -> anyhow::Result<quick_xml::events::BytesEnd<'a>> {
        let event = self.read_event()?;
        match event {
            quick_xml::events::Event::End(e) => Ok(e),
            e => anyhow::bail!("expected end, got {:?}", e),
        }
    }
    fn read_text(&mut self) -> anyhow::Result<quick_xml::events::BytesText<'a>> {
        let event = self.read_event()?;
        match event {
            quick_xml::events::Event::Text(e) => Ok(e),
            e => anyhow::bail!("expected text, got {:?}", e),
        }
    }
}

pub type XmlWriter = quick_xml::Writer<Vec<u8>>;

/// Allows structs to serialize themselves into xml fragments
pub trait IntoXml {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()>;

    fn into_string(&self) -> std::io::Result<String> {
        let mut w = quick_xml::Writer::new(Vec::new());
        self.write_xml(&mut w)?;
        Ok(String::from_utf8(w.into_inner()).expect("produced value to be utf-8"))
    }
}

impl std::fmt::Debug for Box<dyn IntoXml> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.into_string().unwrap())
    }
}

/// Allows structs to deserialize themselves from xml reader
pub trait FromXml<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl<T: IntoXml> IntoXml for Vec<T> {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        for el in self {
            el.write_xml(w)?;
        }
        Ok(())
    }
}
