//! Map a capture-library error string onto a platform-neutral class.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Permission,
    Unsupported,
    NoDisplay,
    Capture,
}

pub(crate) fn classify(reason: &str) -> FailureKind {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("access is not")
        || lower.contains("access denied")
    {
        FailureKind::Permission
    } else if lower.contains("unsupported") || lower.contains("not supported") {
        FailureKind::Unsupported
    } else if lower.contains("no primary")
        || lower.contains("no display")
        || lower.contains("no monitor")
    {
        FailureKind::NoDisplay
    } else {
        FailureKind::Capture
    }
}
