use std::fmt;

/// A single validation problem, optionally tied to a provider id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub provider: Option<String>,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider {
            Some(id) => write!(f, "provider `{id}`: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// All validation problems found, so the user can fix them in one go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Errors(pub Vec<Error>);

impl Errors {
    pub(crate) fn single(provider: Option<&str>, message: impl Into<String>) -> Self {
        Errors(vec![Error {
            provider: provider.map(str::to_owned),
            message: message.into(),
        }])
    }
}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Errors {}
