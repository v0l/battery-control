//! Generic authentication / binding flow for backends that require pairing.
//!
//! Some devices (e.g. Anker SOLIX gen-2) won't accept control until the client
//! is **bound** — often via a physical confirmation on the unit. Backends drive
//! this through [`Battery::authenticate`](crate::Battery::authenticate), which
//! returns an [`AuthState`]:
//!
//! - [`AuthState::Authed`] — done; the backend has persisted any credential (see
//!   [`crate::credentials`]) so future connects skip the flow.
//! - [`AuthState::PendingApproval`] — the user must perform a physical action on
//!   the device (e.g. press & hold the power button), then call `authenticate`
//!   again to continue.
//! - [`AuthState::PinCode`] — the device needs a code; call `authenticate` again
//!   with [`AuthInput::Pin`].
//!
//! A caller loops: `authenticate(AuthInput::None)` → act on the returned state →
//! `authenticate(...)` again until `Authed`.

use serde::Serialize;

/// The result of one step of a backend's auth/binding flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum AuthState {
    /// Fully authenticated and ready; the credential (if any) is saved.
    Authed,
    /// A physical confirmation is required on the device, then retry.
    PendingApproval {
        /// Human-readable instruction, e.g. "press & hold the power button".
        message: String,
    },
    /// A PIN/code is required; retry with [`AuthInput::Pin`].
    PinCode {
        /// Human-readable prompt.
        message: String,
    },
}

impl AuthState {
    /// A pending-approval state with the given instruction.
    pub fn approval(message: impl Into<String>) -> Self {
        AuthState::PendingApproval { message: message.into() }
    }

    /// A pin-required state with the given prompt.
    pub fn pin(message: impl Into<String>) -> Self {
        AuthState::PinCode { message: message.into() }
    }

    /// Whether the flow is complete.
    pub fn is_authed(&self) -> bool {
        matches!(self, AuthState::Authed)
    }
}

/// Input provided to continue an auth flow.
#[derive(Debug, Clone, Default)]
pub enum AuthInput {
    /// Initial attempt, or a retry after completing a physical approval.
    #[default]
    None,
    /// A PIN/code entered by the user.
    Pin(String),
}

impl AuthInput {
    /// The PIN string, if this is [`AuthInput::Pin`].
    pub fn pin(&self) -> Option<&str> {
        match self {
            AuthInput::Pin(p) => Some(p),
            AuthInput::None => None,
        }
    }
}
