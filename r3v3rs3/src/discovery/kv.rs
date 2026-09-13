//! The keys of a key-value store as labels. The Consul and the etcd providers use the same key
//! layout: the key `<prefix>/http/app/ports` is the label `r3v3rs3.http.app.ports`.

use super::labels;
use super::{Built, ProxyGroups};
use base64::prelude::{Engine as _, BASE64_STANDARD};

/// A key and its Base64 value. A folder has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvEntry {
    pub key: String,
    pub value: Option<String>,
}

/// Adds the proxies of the keys under the prefix.
pub fn add_kv(built: &mut Built, groups: &mut ProxyGroups, entries: &[KvEntry], prefix: &str) {
    let mut pairs = Vec::new();
    for entry in entries {
        match kv_label(entry, prefix) {
            Ok(Some(pair)) => pairs.push(pair),
            Ok(None) => {}
            Err(message) => built.issue(prefix, message),
        }
    }
    let parsed = labels::parse(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())), None);
    for message in parsed.issues {
        built.issue(prefix, message);
    }
    for definition in parsed.definitions {
        let resource = format!("{prefix}/{}", definition.key.replace('.', "/"));
        if let Err(message) = groups.add("kv", &resource, definition) {
            built.issue(prefix, message);
        }
    }
}

/// The label of a key. A folder and a key outside the prefix have no label.
fn kv_label(entry: &KvEntry, prefix: &str) -> Result<Option<(String, String)>, String> {
    let rest = entry
        .key
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty() && !rest.ends_with('/'));
    let (Some(rest), Some(value)) = (rest, &entry.value) else {
        return Ok(None);
    };
    if rest
        .split('/')
        .any(|part| part.is_empty() || part.contains('.'))
    {
        return Err(format!("invalid key: {}", entry.key));
    }
    let text = BASE64_STANDARD
        .decode(value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| format!("the value is not UTF-8 text: {}", entry.key))?;
    let label = format!("{}.{}", labels::PREFIX, rest.replace('/', "."));
    Ok(Some((label, text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::first_route_servers as servers;
    use r3v3rs3_api::discovery::DiscoveryProvider;

    fn entry(key: &str, value: Option<&str>) -> KvEntry {
        KvEntry {
            key: key.into(),
            value: value.map(|text| BASE64_STANDARD.encode(text)),
        }
    }

    #[test]
    fn keys_under_the_prefix_become_labels() {
        let entries = [
            entry("r3v3rs3/http/", None),
            entry("r3v3rs3/http/app/ports", Some("http")),
            entry(
                "r3v3rs3/http/app/routes/0/servers/0/url",
                Some("http://10.0.0.5:8080"),
            ),
            entry(
                "r3v3rs3/http/app/vhosts",
                Some("a.example.com,b.example.com"),
            ),
            entry("r3v3rs3/http/bad.name/ports", Some("http")),
            entry("r3v3rs3x/http/other/ports", Some("http")),
        ];
        let mut built = Built::default();
        let mut groups = ProxyGroups::new(DiscoveryProvider::Consul);
        add_kv(&mut built, &mut groups, &entries, "r3v3rs3");
        let proxies = groups.into_proxies();
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].key, "kv/http.app");
        assert_eq!(proxies[0].source.resource, "r3v3rs3/http/app");
        assert_eq!(servers(&proxies[0]), ["http://10.0.0.5:8080/"]);
        assert_eq!(
            built.issues.iter().map(|i| &i.message).collect::<Vec<_>>(),
            ["invalid key: r3v3rs3/http/bad.name/ports"]
        );
    }
}
