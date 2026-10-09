use std::fmt;

/// Failure context for supervisor and service operations.
///
/// Production paths should return this value. The fields identify where the
/// failure happened without requiring a separate error framework. The variable
/// fields live on the heap so this type stays small enough to return directly.
pub struct Error {
    component: &'static str,
    operation: &'static str,
    detail: Box<ErrorDetail>,
}

struct ErrorDetail {
    service: Option<String>,
    platform: Option<String>,
    state: Option<String>,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub fn new(
        component: &'static str,
        operation: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            component,
            operation,
            detail: Box::new(ErrorDetail {
                service: None,
                platform: None,
                state: None,
                message: message.into(),
                source: None,
            }),
        }
    }

    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.detail.service = Some(service.into());
        self
    }

    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.detail.platform = Some(platform.into());
        self
    }

    pub fn with_state(mut self, state: impl Into<String>) -> Self {
        self.detail.state = Some(state.into());
        self
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.detail.source = Some(Box::new(source));
        self
    }

    pub fn component(&self) -> &'static str {
        self.component
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn service(&self) -> Option<&str> {
        self.detail.service.as_deref()
    }

    pub fn state(&self) -> Option<&str> {
        self.detail.state.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.detail.message
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Error")
            .field("component", &self.component)
            .field("operation", &self.operation)
            .field("service", &self.detail.service)
            .field("platform", &self.detail.platform)
            .field("state", &self.detail.state)
            .field("message", &self.detail.message)
            .field(
                "source",
                &self.detail.source.as_ref().map(ToString::to_string),
            )
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "component={} operation={}",
            self.component, self.operation
        )?;
        if let Some(service) = &self.detail.service {
            write!(formatter, " service={service}")?;
        }
        if let Some(platform) = &self.detail.platform {
            write!(formatter, " platform={platform}")?;
        }
        if let Some(state) = &self.detail.state {
            write!(formatter, " state={state}")?;
        }
        write!(formatter, " {}", self.detail.message)?;
        if let Some(source) = &self.detail.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.detail
            .source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
