use crate::action::{ActionError, ActionErrorCode};

/// The Browse() request failed because the specified ObjectID argument is invalid.
#[derive(Debug)]
pub struct NoSuchObjectError;

impl From<NoSuchObjectError> for ActionError {
    fn from(_value: NoSuchObjectError) -> Self {
        ActionError {
            code: ActionErrorCode::Other(701),
            description: Some("No such object".into()),
        }
    }
}

impl From<anyhow::Error> for NoSuchObjectError {
    fn from(_value: anyhow::Error) -> Self {
        Self
    }
}

/// Unsupported or invalid sort criteria
#[derive(Debug)]
pub struct InvalidSortError;

impl From<InvalidSortError> for ActionError {
    fn from(_value: InvalidSortError) -> Self {
        ActionError {
            code: ActionErrorCode::Other(709),
            description: Some("Unsupported or invalid sort criteria".into()),
        }
    }
}

impl From<anyhow::Error> for InvalidSortError {
    fn from(_value: anyhow::Error) -> Self {
        Self
    }
}

/// The Browse() request failed because the ContentDirectory service
/// is unable to compute, in the time allotted, the total number of
/// objects that are a match for the browse criteria and is additionally
/// unable to return, in the time allotted, any objects that match the
/// browse criteria
#[derive(Debug)]
pub struct CannotProcessError;

impl From<CannotProcessError> for ActionError {
    fn from(_value: CannotProcessError) -> Self {
        ActionError {
            code: ActionErrorCode::Other(720),
            description: Some("Cannot process the request".into()),
        }
    }
}

/// The Search() request failed because the ContainerID argument is invalid or identifies an object that is not a container
#[derive(Debug)]
pub struct NoSuchContainerError;

impl From<NoSuchContainerError> for ActionError {
    fn from(_value: NoSuchContainerError) -> Self {
        ActionError {
            code: ActionErrorCode::Other(710),
            description: Some("No such object".into()),
        }
    }
}

impl From<anyhow::Error> for NoSuchContainerError {
    fn from(_value: anyhow::Error) -> Self {
        Self
    }
}

/// CreateObject() failed because the Elements argument is not supported or is invalid.
#[derive(Debug)]
pub struct BadMetadata;

impl From<BadMetadata> for ActionError {
    fn from(_value: BadMetadata) -> Self {
        ActionError {
            code: ActionErrorCode::Other(712),
            description: Some("Bad metadata".into()),
        }
    }
}

impl From<anyhow::Error> for BadMetadata {
    fn from(_value: anyhow::Error) -> Self {
        Self
    }
}
