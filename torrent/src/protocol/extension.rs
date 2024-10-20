/// If the client supporting the extensions can decide which numbers the messages it receives will have,
/// it means they are constants within that client. i.e. they can be used in switch statements.
/// It's easy for the other end to store an array with the ID's we expect for each message
/// and use that for lookups each time it sends an extension message.
pub trait Extension<'a>: Into<bytes::Bytes> + TryFrom<&'a [u8]> {
    const NAME: &'static str;
    const CLIENT_ID: u8;
}
