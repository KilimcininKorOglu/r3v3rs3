//! The Git source of an app: the repository, the branch and the paths inside the checkout. Each
//! value is validated when it is parsed, so it is safe as an operand of `git` and as a path under
//! the checkout directory.

use crate::error::Error;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use url::Url;

/// The longest repository URL, branch or path.
const MAX_LENGTH: usize = 1024;

fn invalid(reason: String) -> Error {
    Error::InvalidContainerSpec { reason }
}

macro_rules! checked_string {
    ($(#[$doc:meta])* $name:ident, $check:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                $check(s)?;
                Ok(Self(s.to_string()))
            }
        }

        impl TryFrom<String> for $name {
            type Error = Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                $check(&value)?;
                Ok(Self(value))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

checked_string!(
    /// An `https://` repository URL without credentials. A private repository gets its token
    /// through the token of the app, never through the URL.
    RepoUrl,
    check_repo_url
);
checked_string!(
    /// A branch or a tag name.
    GitRef,
    check_git_ref
);
checked_string!(
    /// A relative path inside the checkout, without `..` segments.
    RelPath,
    check_rel_path
);

impl GitRef {
    /// The branch `main`.
    pub fn main() -> Self {
        Self("main".to_string())
    }
}

impl RelPath {
    /// The file `Dockerfile`.
    pub fn dockerfile() -> Self {
        Self("Dockerfile".to_string())
    }

    /// The root directory of the checkout.
    pub fn root() -> Self {
        Self(".".to_string())
    }
}

fn check_repo_url(value: &str) -> Result<(), Error> {
    let url = Url::parse(value).map_err(|_| invalid(format!("invalid repository URL: {value}")))?;
    let valid = value.len() <= MAX_LENGTH
        && value.chars().all(|c| c.is_ascii_graphic())
        && url.scheme() == "https"
        && url.host_str().is_some()
        && has_only_host_and_path(&url);
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "a repository URL must be https:// without credentials, a query or a fragment: {value}"
        )))
    }
}

/// Whether the URL carries no credentials, query or fragment.
fn has_only_host_and_path(url: &Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Accepts the names that `git check-ref-format --branch` accepts, restricted to safe characters.
fn check_git_ref(value: &str) -> Result<(), Error> {
    let valid = has_safe_chars(value)
        && !value.starts_with(['-', '/', '.'])
        && !value.ends_with(['/', '.'])
        && !value.ends_with(".lock")
        && !["..", "//", "/."].iter().any(|part| value.contains(part));
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("invalid branch name: {value}")))
    }
}

/// Whether the value is not empty, not too long, and holds only letters, digits and `._/-`.
fn has_safe_chars(value: &str) -> bool {
    let valid_char = |c: char| c.is_ascii_alphanumeric() || "._/-".contains(c);
    !value.is_empty() && value.len() <= MAX_LENGTH && value.chars().all(valid_char)
}

fn check_rel_path(value: &str) -> Result<(), Error> {
    let valid = has_safe_chars(value)
        && !value.starts_with(['/', '-'])
        && !value.split('/').any(|segment| segment == "..");
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("invalid path in the repository: {value}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_url_is_https_without_credentials() {
        for valid in [
            "https://github.com/owner/app.git",
            "https://git.example.com:8443/team/app",
        ] {
            assert!(valid.parse::<RepoUrl>().is_ok(), "{valid}");
        }
        for invalid in [
            "http://github.com/owner/app",
            "ssh://git@github.com/owner/app",
            "git@github.com:owner/app.git",
            "file:///srv/app",
            "https://user:token@github.com/owner/app",
            "https://github.com/owner/app?x=1",
            "https://github.com/owner/app#main",
            "--upload-pack=touch",
        ] {
            assert!(invalid.parse::<RepoUrl>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_branch_cannot_look_like_an_option_or_leave_the_refs() {
        for valid in ["main", "release/1.2", "v1.0.0", "feature_x-y"] {
            assert!(valid.parse::<GitRef>().is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "-main",
            "--upload-pack=x",
            "a..b",
            "main.lock",
            "/main",
            "main/",
            "a//b",
            "a/.b",
            "main@{1}",
            "a b",
        ] {
            assert!(invalid.parse::<GitRef>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_defaults_pass_their_own_checks() {
        assert!(check_git_ref(GitRef::main().as_str()).is_ok());
        assert!(check_rel_path(RelPath::dockerfile().as_str()).is_ok());
        assert!(check_rel_path(RelPath::root().as_str()).is_ok());
    }

    #[test]
    fn a_path_stays_inside_the_checkout() {
        for valid in ["Dockerfile", "docker/Dockerfile.prod", ".", "services/api"] {
            assert!(valid.parse::<RelPath>().is_ok(), "{valid}");
        }
        for invalid in ["", "/etc/passwd", "../x", "a/../../b", "-f", "a b"] {
            assert!(invalid.parse::<RelPath>().is_err(), "{invalid}");
        }
    }
}
