//! Greeting API used by the repository example.

use rspyts::Model;
use serde::{Deserialize, Serialize};

/// A greeting returned to Python and TypeScript callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Model)]
pub struct Greeting {
    pub message: String,
}

/// Greet one person.
#[rspyts::export]
#[must_use]
pub fn greet(name: String) -> Greeting {
    Greeting {
        message: format!("Hello, {name}!"),
    }
}
