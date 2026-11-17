//! The profile's secrets on the Rust service, such as `GITHUB_TOKEN`:
//! `GET /api/secrets`, and `PUT` and `DELETE /api/secrets/{name}`.
//!
//! Only names are read back. A value goes out once, in the body of a `PUT`,
//! and nothing here keeps or prints it; that body is freed after the
//! request, not overwritten. The service never quotes a value in a refusal
//! either, so its message can be shown as it is.

use serde::{Deserialize, Serialize};

use super::{AddError, Connection, Error};
use crate::{RESPONSE_LIMIT, transport::local};

const SECRETS: &str = "/api/secrets";
/// The largest body the service takes for a secret.
const BODY_LIMIT: usize = 1024 * 1024;

/// One secret the service reads or keeps, and whether it has a value.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Secret {
    /// The secret's name, such as `GITHUB_TOKEN`.
    pub name: String,
    /// Whether it has a value.
    pub set: bool,
}

/// The service's answer to a `PUT` or a `DELETE`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Change {
    /// The secret's name.
    pub name: String,
    /// Whether the secret has a value now.
    pub set: bool,
    /// Whether it had one before: a `PUT` replaced it, or a `DELETE` removed it.
    pub was_set: bool,
}

#[derive(Deserialize)]
struct List {
    secrets: Vec<Secret>,
}

/// The body of a `PUT`, borrowing the value so no copy of it is made.
#[derive(Serialize)]
struct Value<'a> {
    value: &'a str,
}

/// Whether `name` is a secret's name as the service takes it: an upper-case
/// letter, then upper-case letters, digits and `_`.
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Whether the service takes `value` as a secret's value: one line of text,
/// with no control character, that is not only space. Space around it is
/// not kept.
pub fn is_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

impl Connection {
    /// The profile's secrets, by name. Runs on an I/O worker.
    pub fn secrets(&self) -> Result<Vec<Secret>, AddError> {
        if !self.served {
            return Err(Error::Unsupported.into());
        }
        let response = self.response(SECRETS, None, RESPONSE_LIMIT)?;
        answer::<List>(&response).map(|list| list.secrets)
    }

    /// Give the secret `name` the value `value`, adding or replacing it, under
    /// `request_id` (32 lowercase hexadecimal digits). A value the service
    /// would refuse, or one too long for its body, is `Invalid` and never
    /// sent. Runs on an I/O worker.
    pub fn set_secret(
        &self,
        name: &str,
        value: &str,
        request_id: &str,
    ) -> Result<Change, AddError> {
        if !is_value(value) {
            return Err(Error::Invalid.into());
        }
        let value = value.trim();
        let body = serde_json::to_vec(&Value { value }).map_err(|_| Error::Invalid)?;
        if body.len() > BODY_LIMIT {
            return Err(Error::Invalid.into());
        }
        self.change("PUT", name, Some(body), request_id)
    }

    /// Remove the secret `name` under `request_id`. Runs on an I/O worker.
    pub fn remove_secret(&self, name: &str, request_id: &str) -> Result<Change, AddError> {
        self.change("DELETE", name, None, request_id)
    }

    fn change(
        &self,
        method: &'static str,
        name: &str,
        body: Option<Vec<u8>>,
        request_id: &str,
    ) -> Result<Change, AddError> {
        if !self.served {
            return Err(Error::Unsupported.into());
        }
        if !is_name(name) {
            return Err(Error::Invalid.into());
        }
        let response = self.write(method, &format!("{SECRETS}/{name}"), body, Some(request_id))?;
        let change = answer::<Change>(&response)?;
        if change.name != name {
            return Err(Error::Invalid.into());
        }
        Ok(change)
    }
}

/// A success's body, or the service's reason for a refusal.
fn answer<T: serde::de::DeserializeOwned>(response: &local::Response) -> Result<T, AddError> {
    super::added(response)?;
    if response.body.len() > RESPONSE_LIMIT {
        return Err(Error::Invalid.into());
    }
    serde_json::from_slice(&response.body).map_err(|_| Error::Invalid.into())
}

#[cfg(test)]
mod tests;
