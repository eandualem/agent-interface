//! Command-line options: `--name value` pairs.
//!
//! `agent-interface-bench-rio` includes this file.

use std::collections::HashMap;
use std::str::FromStr;

/// Parsed `--name value` pairs.
pub struct Options(HashMap<String, String>);

impl Options {
    /// Parses `args`, which must be `--name value` pairs.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = HashMap::new();
        let mut args = args.iter();
        while let Some(name) = args.next() {
            let name = name
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {name}"))?;
            let value = args
                .next()
                .ok_or_else(|| format!("--{name} needs a value"))?;
            options.insert(name.to_owned(), value.clone());
        }
        Ok(Self(options))
    }

    /// Returns option `name`, or an error naming it if it is missing.
    pub fn required(&self, name: &str) -> Result<&str, String> {
        self.0
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("--{name} is required"))
    }

    /// Returns option `name` parsed, or `default` if it is missing.
    pub fn get<T: FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.0.get(name) {
            Some(value) => value
                .parse()
                .map_err(|_| format!("--{name}: cannot use {value:?}")),
            None => Ok(default),
        }
    }
}
