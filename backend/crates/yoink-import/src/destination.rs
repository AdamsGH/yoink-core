pub use yoink_db::publication::DestinationGuard;

impl From<yoink_db::publication::PublicationError> for crate::ImportError {
    fn from(error: yoink_db::publication::PublicationError) -> Self {
        use yoink_db::publication::PublicationError;
        match error {
            PublicationError::Destination => Self::Destination,
            PublicationError::Storage => Self::Storage,
            PublicationError::EngineCapability => Self::EngineCapability,
            PublicationError::PublishedDurabilityUncertain => Self::PublishedDurabilityUncertain,
        }
    }
}
