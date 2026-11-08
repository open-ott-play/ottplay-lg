//! Safe, per-write diagnostics for the existing settings error surface.
use plx_platform::storage::{private, CommitStage, StoreError};
use plx_platform::storage::wire::{ErrorCode, KeymanagerFailureCategory, KeymanagerOperation, KeymanagerStage};

/// The input is captured by the failed write, never read from a later global snapshot.
/// Every label comes from a closed enum; numbers are bounded signed 32-bit values.
pub(super) fn message(error: private::Error) -> String {
    describe(error, "Could not save protected OTT settings")
}

pub(super) fn preference_message(error: private::Error) -> String {
    describe(error, "Could not save player preferences")
}

fn describe(error: private::Error, prefix: &str) -> String {
    let detail = match error {
        private::Error::HelperFailed { failure, .. } => failure.line(),
        private::Error::ProtectionFailed(failure) => {
            let operation = match failure.operation {
                KeymanagerOperation::Seal => "seal",
                KeymanagerOperation::Open => "open",
                KeymanagerOperation::Readback => "readback",
            };
            let stage = match failure.stage {
                KeymanagerStage::Validate => "validate",
                KeymanagerStage::Generate => "generate",
                KeymanagerStage::Begin => "begin",
                KeymanagerStage::Finish => "finish",
                KeymanagerStage::Roundtrip => "roundtrip",
            };
            let category = match failure.category {
                KeymanagerFailureCategory::Unavailable => "unavailable",
                KeymanagerFailureCategory::Timeout => "timeout",
                KeymanagerFailureCategory::ServiceRejected => "service-rejected",
                KeymanagerFailureCategory::InvalidResponse => "invalid-response",
                KeymanagerFailureCategory::Other => "other",
            };
            let code = match failure.code {
                ErrorCode::Invalid => "invalid",
                ErrorCode::Unavailable => "unavailable",
                ErrorCode::Timeout => "timeout",
                ErrorCode::Capability => "capability",
                ErrorCode::Authentication => "authentication",
                ErrorCode::Protocol => "protocol",
                ErrorCode::Corrupt => "corrupt",
            };
            let number = failure.service_code.map_or_else(|| "unknown".into(), |n| n.to_string());
            format!("storage: {operation}/{stage} {category} {code} ({number})")
        }
        private::Error::Store(error) => store_message(error),
        private::Error::ProtectionUnavailable => "storage: protection unavailable".into(),
        private::Error::Invalid => "storage: invalid record".into(),
        private::Error::TooLarge => "storage: record too large".into(),
        private::Error::Revoked => "storage: write revoked".into(),
    };
    format!("{prefix}. {detail}")
}

fn store_message(error: StoreError) -> String {
    let label = match error {
        StoreError::Io { stage, errno } => {
            let stage = match stage {
                CommitStage::CreateTemp => "create-temp",
                CommitStage::Write => "write",
                CommitStage::FileSync => "file-sync",
                CommitStage::Rename => "rename",
                CommitStage::ParentOpen => "parent-open",
                CommitStage::ParentSync => "parent-sync",
                CommitStage::Readback => "readback",
                CommitStage::Cleanup => "cleanup",
            };
            return format!("storage: {stage} ({errno})");
        }
        StoreError::RootNotDirectory => "root not directory",
        StoreError::RootWrongOwner => "root wrong owner",
        StoreError::RootUnsafeMode => "root unsafe mode",
        StoreError::DestinationNotRegular => "destination not regular",
        StoreError::DestinationWrongOwner => "destination wrong owner",
        StoreError::DestinationUnsafeMode => "destination unsafe mode",
        StoreError::RecordNotRegular => "record not regular",
        StoreError::RecordWrongOwner => "record wrong owner",
        StoreError::RecordUnsafeMode => "record unsafe mode",
        StoreError::TooLarge => "record too large",
        StoreError::InvalidUtf8 => "invalid utf8",
        StoreError::InvalidSchema => "invalid schema",
        StoreError::UnknownFormat => "unknown format",
        StoreError::UnsupportedVersion => "unsupported version",
        StoreError::DomainKeyMismatch => "domain mismatch",
        StoreError::RootChanged => "root changed",
        StoreError::HelperUnavailable => "helper unavailable",
        StoreError::HelperAuthentication => "helper authentication",
        StoreError::HelperProtocol => "helper protocol",
        StoreError::AuthLocked => "authentication locked",
        StoreError::Conflict => "conflict",
    };
    format!("storage: {label}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_platform::storage::wire::{failure::{Detail, HelperFailure, Stage}, KeymanagerFailure};

    #[test]
    fn preference_diagnostic_keeps_the_closed_detail_without_claiming_protected_write() {
        assert_eq!(preference_message(private::Error::Revoked),
            "Could not save player preferences. storage: write revoked");
    }

    #[test]
    fn helper_diagnostic_reuses_current_operation_evidence() {
        let mut failure = HelperFailure::new(Stage::SocketAbsent, Some(2));
        failure.activation = Some(Detail::new(Stage::ActivationCall, Some(-13)));
        failure.helper = Some(Detail::new(Stage::Db8, Some(-3963)));
        assert_eq!(message(private::Error::HelperFailed {
            source: StoreError::HelperUnavailable, failure,
        }), "Could not save protected OTT settings. storage: helper · db8 (-3963)");
    }

    #[test]
    fn protection_diagnostic_has_closed_operation_stage_and_numeric_code() {
        let failure = KeymanagerFailure {
            operation: KeymanagerOperation::Seal,
            stage: KeymanagerStage::Begin,
            category: KeymanagerFailureCategory::ServiceRejected,
            code: ErrorCode::Authentication,
            service_code: Some(-1000),
            hub_error: None,
        };
        assert_eq!(message(private::Error::ProtectionFailed(failure)),
            "Could not save protected OTT settings. storage: seal/begin service-rejected authentication (-1000)");
        let missing = KeymanagerFailure { service_code: None, ..failure };
        assert!(message(private::Error::ProtectionFailed(missing)).ends_with("(unknown)"));
        for code in [i32::MIN, i32::MAX] {
            let text = message(private::Error::ProtectionFailed(KeymanagerFailure {service_code:Some(code), ..failure}));
            assert!(text.ends_with(&format!("({code})")));
            assert!(text.len() < 180);
        }
    }

    #[test]
    fn local_storage_diagnostic_retains_exact_stage_without_paths() {
        assert_eq!(message(private::Error::Store(StoreError::Io {stage:CommitStage::FileSync,errno:-13})),
            "Could not save protected OTT settings. storage: file-sync (-13)");
        for error in [private::Error::Invalid, private::Error::TooLarge,
            private::Error::Revoked, private::Error::ProtectionUnavailable,
            private::Error::Store(StoreError::HelperUnavailable)] {
            let text = message(error);
            assert!(text.starts_with("Could not save protected OTT settings. storage: "));
            assert!(text.len() < 120);
            assert!(!text.contains("/"));
        }
    }
}
