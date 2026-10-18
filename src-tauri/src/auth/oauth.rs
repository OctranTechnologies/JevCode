use crate::domain::{ProviderAuthMethod, ProviderAuthMethodOption};

/// OpenAI documents Sign in with ChatGPT for eligible open-source apps. The
/// adapter owns the native browser callback, code exchange, and identity check.
/// See https://developers.openai.com/siwc/token-sharing-open-source/sign-in.
pub struct OpenAiChatGptAdapter;

impl OpenAiChatGptAdapter {
    pub const fn method() -> ProviderAuthMethod {
        ProviderAuthMethod::OpenAiChatGpt
    }
}

/// Google documents Gemini OAuth for applications with a registered desktop
/// client and project configured in JevCode's local app configuration.
/// See https://ai.google.dev/gemini-api/docs/oauth and Google's native app guide.
pub struct GoogleGeminiOAuthAdapter;

impl GoogleGeminiOAuthAdapter {
    pub const fn method() -> ProviderAuthMethod {
        ProviderAuthMethod::GoogleOAuth
    }
}

pub(super) fn chatgpt_method() -> ProviderAuthMethodOption {
    ProviderAuthMethodOption {
        method: OpenAiChatGptAdapter::method(),
        label: "Sign in with ChatGPT".into(),
        available: true,
        unavailable_reason: None,
    }
}

pub(super) fn google_method() -> ProviderAuthMethodOption {
    ProviderAuthMethodOption {
        method: GoogleGeminiOAuthAdapter::method(),
        label: "Google account".into(),
        available: false,
        unavailable_reason: Some(
            "Google sign-in needs a registered Desktop OAuth client ID and Google Cloud project in JevCode's configuration.".into(),
        ),
    }
}
