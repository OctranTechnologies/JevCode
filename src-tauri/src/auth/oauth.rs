use crate::domain::{ProviderAuthMethod, ProviderAuthMethodOption};

/// The OpenAI open-source Sign in with ChatGPT flow is reserved for a future
/// adapter that implements the documented PKCE, issued-client, scope and ID-token
/// verification contract. JevCode does not currently ship that integration.
/// See https://developers.openai.com/siwc/token-sharing-open-source/sign-in.
pub struct OpenAiChatGptAdapter;

impl OpenAiChatGptAdapter {
    pub const fn method() -> ProviderAuthMethod {
        ProviderAuthMethod::OpenAiChatGpt
    }
}

/// Google documents Gemini OAuth for applications with a registered desktop
/// client and project. JevCode has no application OAuth client configured yet.
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
        available: false,
        unavailable_reason: Some(
            "JevCode has not enabled OpenAI's official open-source ChatGPT sign-in flow yet. API keys remain available.".into(),
        ),
    }
}

pub(super) fn google_method() -> ProviderAuthMethodOption {
    ProviderAuthMethodOption {
        method: GoogleGeminiOAuthAdapter::method(),
        label: "Google account".into(),
        available: false,
        unavailable_reason: Some(
            "Google Gemini OAuth needs a registered JevCode desktop client and Google Cloud project. Use a Gemini API key for now.".into(),
        ),
    }
}
