use crate::metadata::ContentType;

use super::identification::Token;

#[derive(Debug)]
pub struct ExtrasIdent {
    name: String,
    parent_title: String,
    extra_type: ContentType,
}

#[derive(Debug)]
pub struct ExtrasIdentifier {
    name: String,
    parent_title: String,
    extra_type: ContentType,
}

impl ExtrasIdent {
    pub fn parse_parent<'a>(&mut self, parent_tokens: Vec<Token<'a>>, content_type: ContentType) {
        match content_type {
            ContentType::Movie => {}
            ContentType::Show => todo!(),
        }
    }

    pub fn parse_name<'a>(&mut self, name_tokens: Vec<Token<'a>>) {
        todo!()
    }
}
