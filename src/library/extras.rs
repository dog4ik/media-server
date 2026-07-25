use crate::metadata::ParentMediaType;

use super::identification::Token;

#[derive(Debug)]
pub struct ExtrasIdent {
    name: String,
    parent_title: String,
    extra_type: ParentMediaType,
}

#[derive(Debug)]
pub struct ExtrasIdentifier {
    name: String,
    parent_title: String,
    extra_type: ParentMediaType,
}

impl ExtrasIdent {
    pub fn parse_parent(&mut self, parent_tokens: Vec<Token<'_>>, content_type: ParentMediaType) {
        match content_type {
            ParentMediaType::Movie => {}
            ParentMediaType::Show => todo!(),
        }
    }

    pub fn parse_name(&mut self, name_tokens: Vec<Token<'_>>) {
        todo!()
    }
}
