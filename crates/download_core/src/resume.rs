//! Observed safe resume capability, independent of local file/action availability.
use crate::transfer::text;
use reqwest::{Response, header};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResumeCapability {
    #[default]
    Unknown,
    Supported,
    Unsupported,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResumeReason {
    #[default]
    NotObserved,
    RevalidationRequired,
    ValidatedRange,
    RangeIgnored,
    MissingStrongValidator,
    ConditionalFullResponse,
    IdentityChanged,
    InvalidRangeResponse,
    AuthenticationRequired,
    UnknownTotal,
}
#[derive(Debug, Clone, Default)]
pub struct ResumeObservation {
    pub capability: ResumeCapability,
    pub reason: ResumeReason,
    pub observed_at_ms: Option<u64>,
    identity: Option<Identity>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    etag: Option<String>,
    modified: Option<String>,
    url: String,
    total: Option<u64>,
}
impl Identity {
    fn from_response(response: &Response, total: Option<u64>) -> Self {
        Self {
            etag: text(response, header::ETAG),
            modified: text(response, header::LAST_MODIFIED),
            url: response.url().to_string(),
            total,
        }
    }
}
pub(crate) fn strong_etag(response: &Response) -> Option<String> {
    text(response, header::ETAG).filter(|v| v.len() >= 2 && v.starts_with('"') && v.ends_with('"'))
}
impl ResumeObservation {
    pub(crate) fn invalidate(&mut self, reason: ResumeReason) {
        *self = Self {
            reason,
            ..Self::default()
        };
    }
    pub(crate) fn unknown(&mut self, reason: ResumeReason) {
        self.invalidate(reason);
        self.observed_at_ms = Some(crate::connections::deadline_ms(0));
    }
    pub(crate) fn range(&mut self, response: &Response, total: u64) {
        self.capability = if strong_etag(response).is_some() {
            ResumeCapability::Supported
        } else {
            ResumeCapability::Unsupported
        };
        self.reason = if self.capability == ResumeCapability::Supported {
            ResumeReason::ValidatedRange
        } else {
            ResumeReason::MissingStrongValidator
        };
        self.observed_at_ms = Some(crate::connections::deadline_ms(0));
        self.identity = Some(Identity::from_response(response, Some(total)));
    }
    pub(crate) fn ignored(&mut self, response: &Response, conditional: bool) {
        self.capability = if conditional {
            ResumeCapability::Unknown
        } else {
            ResumeCapability::Unsupported
        };
        self.reason = if conditional {
            ResumeReason::ConditionalFullResponse
        } else {
            ResumeReason::RangeIgnored
        };
        self.observed_at_ms = Some(crate::connections::deadline_ms(0));
        self.identity = Some(Identity::from_response(response, response.content_length()));
    }
    pub(crate) fn full(&mut self, response: &Response, conditional: bool) {
        if conditional {
            self.ignored(response, true);
        } else if strong_etag(response).is_none() {
            self.capability = ResumeCapability::Unsupported;
            self.reason = ResumeReason::MissingStrongValidator;
            self.observed_at_ms = Some(crate::connections::deadline_ms(0));
            self.identity = None;
        } else if self.identity.as_ref()
            != Some(&Identity::from_response(
                response,
                response.content_length(),
            ))
        {
            // A normal 200 response with a validator does not prove range support.
            self.unknown(ResumeReason::NotObserved);
        }
    }
}
