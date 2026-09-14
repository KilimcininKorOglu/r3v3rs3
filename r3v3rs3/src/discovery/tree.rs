//! A tree of text values that serde reads as the proxy model. Labels, tags and key-value stores
//! keep every value as text, so the tree parses numbers, booleans, names and lists from text.

use serde::de::{self, DeserializeOwned, DeserializeSeed, IntoDeserializer, Visitor};
use std::cell::RefCell;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Leaf(String),
    Map(BTreeMap<String, Node>),
}

impl Default for Node {
    fn default() -> Self {
        Node::Map(BTreeMap::new())
    }
}

impl Node {
    /// Sets the value at the path. Returns false when a value or a group already uses the path.
    pub fn insert(&mut self, path: &[&str], value: &str) -> bool {
        let Node::Map(map) = self else {
            return false;
        };
        match path {
            [] => false,
            [key] => match map.entry((*key).to_string()) {
                Entry::Vacant(entry) => {
                    entry.insert(Node::Leaf(value.to_string()));
                    true
                }
                Entry::Occupied(_) => false,
            },
            [key, rest @ ..] => map
                .entry((*key).to_string())
                .or_default()
                .insert(rest, value),
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Node> {
        match self {
            Node::Map(map) => map.remove(key),
            Node::Leaf(_) => None,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        matches!(self, Node::Map(map) if map.contains_key(key))
    }

    pub fn child_mut(&mut self, key: &str) -> Option<&mut Node> {
        match self {
            Node::Map(map) => map.get_mut(key),
            Node::Leaf(_) => None,
        }
    }

    /// The values of a group, for example each route of `routes`.
    pub fn children_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        match self {
            Node::Map(map) => Some(map.values_mut()),
            Node::Leaf(_) => None,
        }
        .into_iter()
        .flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeError {
    path: String,
    message: String,
}

impl TreeError {
    /// Adds the path of the value, unless a nested value already set its own path.
    fn at(mut self, path: &str) -> Self {
        if self.path.is_empty() {
            self.path = path.to_string();
        }
        self
    }
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for TreeError {}

impl de::Error for TreeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self {
            path: String::new(),
            message: msg.to_string(),
        }
    }
}

type UnknownKeys = Rc<RefCell<Vec<String>>>;

/// Reads the tree as `T`. Returns the value and the paths of the keys that `T` does not have.
pub fn from_node<T: DeserializeOwned>(node: Node) -> Result<(T, Vec<String>), TreeError> {
    let unknown = UnknownKeys::default();
    let value = T::deserialize(NodeDeserializer {
        node,
        path: String::new(),
        unknown: unknown.clone(),
    })?;
    let keys = std::mem::take(&mut *unknown.borrow_mut());
    Ok((value, keys))
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// A group whose keys are all numbers is a list.
fn is_list(map: &BTreeMap<String, Node>) -> bool {
    !map.is_empty() && map.keys().all(|key| key.parse::<usize>().is_ok())
}

struct NodeDeserializer {
    node: Node,
    path: String,
    unknown: UnknownKeys,
}

impl NodeDeserializer {
    fn child(&self, key: &str, node: Node) -> Self {
        Self {
            node,
            path: join(&self.path, key),
            unknown: self.unknown.clone(),
        }
    }

    fn error(&self, message: impl fmt::Display) -> TreeError {
        TreeError {
            path: self.path.clone(),
            message: message.to_string(),
        }
    }

    fn text(&self) -> Result<&str, TreeError> {
        match &self.node {
            Node::Leaf(text) => Ok(text),
            Node::Map(_) => Err(self.error("expected a value, found a group of keys")),
        }
    }

    fn parse<T: FromStr>(&self, expected: &str) -> Result<T, TreeError> {
        let text = self.text()?;
        text.trim()
            .parse()
            .map_err(|_| self.error(format!("expected {expected}, found {text:?}")))
    }

    /// A value lists its items with commas. A group lists them with the keys 0, 1, 2 and so on.
    fn items(&self) -> Result<ItemReader, TreeError> {
        let items = match &self.node {
            Node::Leaf(text) => text
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .enumerate()
                .map(|(index, item)| self.child(&index.to_string(), Node::Leaf(item.to_string())))
                .collect(),
            Node::Map(map) => {
                let mut indexed = map
                    .iter()
                    .map(|(key, node)| key.parse::<usize>().map(|index| (index, key, node)))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| self.error("expected a list with the keys 0, 1, 2 and so on"))?;
                indexed.sort_by_key(|(index, ..)| *index);
                indexed
                    .into_iter()
                    .map(|(_, key, node)| self.child(key, node.clone()))
                    .collect()
            }
        };
        Ok(ItemReader {
            items: Vec::into_iter(items),
        })
    }

    fn entries(&self) -> Result<EntryReader, TreeError> {
        let Node::Map(map) = &self.node else {
            return Err(self.error("expected a group of keys, found a value"));
        };
        let entries = map
            .iter()
            .map(|(key, node)| (key.clone(), self.child(key, node.clone())))
            .collect::<Vec<_>>();
        Ok(EntryReader {
            entries: entries.into_iter(),
            value: None,
        })
    }
}

struct ItemReader {
    items: std::vec::IntoIter<NodeDeserializer>,
}

impl<'de> de::SeqAccess<'de> for ItemReader {
    type Error = TreeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, TreeError> {
        self.items
            .next()
            .map(|item| {
                let path = item.path.clone();
                seed.deserialize(item).map_err(|err| err.at(&path))
            })
            .transpose()
    }
}

struct EntryReader {
    entries: std::vec::IntoIter<(String, NodeDeserializer)>,
    value: Option<NodeDeserializer>,
}

impl<'de> de::MapAccess<'de> for EntryReader {
    type Error = TreeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TreeError> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.value = Some(value);
        seed.deserialize(key.into_deserializer()).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, TreeError> {
        let value = self
            .value
            .take()
            .ok_or_else(|| de::Error::custom("value without a key"))?;
        let path = value.path.clone();
        seed.deserialize(value).map_err(|err| err.at(&path))
    }
}

macro_rules! parse_number {
    ($($method:ident $visit:ident $ty:ty),* $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
                visitor.$visit(self.parse::<$ty>("a number")?)
            }
        )*
    };
}

impl<'de> de::Deserializer<'de> for NodeDeserializer {
    type Error = TreeError;

    /// A tagged enum, such as the authentication policy, reads its fields without a type hint,
    /// so every value stays text.
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        match &self.node {
            Node::Leaf(text) => visitor.visit_string(text.clone()),
            Node::Map(map) if is_list(map) => visitor.visit_seq(self.items()?),
            Node::Map(_) => visitor.visit_map(self.entries()?),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        visitor.visit_bool(self.parse::<bool>("true or false")?)
    }

    parse_number! {
        deserialize_i8 visit_i8 i8,
        deserialize_i16 visit_i16 i16,
        deserialize_i32 visit_i32 i32,
        deserialize_i64 visit_i64 i64,
        deserialize_u8 visit_u8 u8,
        deserialize_u16 visit_u16 u16,
        deserialize_u32 visit_u32 u32,
        deserialize_u64 visit_u64 u64,
        deserialize_f32 visit_f32 f32,
        deserialize_f64 visit_f64 f64,
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        visitor.visit_char(self.parse::<char>("one character")?)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        visitor.visit_string(self.text()?.to_string())
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        self.deserialize_str(visitor)
    }

    /// An empty value is no value.
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        match &self.node {
            Node::Leaf(text) if text.trim().is_empty() => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, TreeError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, TreeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        visitor.visit_seq(self.items()?)
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, TreeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, TreeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        visitor.visit_map(self.entries()?)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TreeError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TreeError> {
        visitor.visit_enum(self.text()?.trim().to_string().into_deserializer())
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        self.deserialize_str(visitor)
    }

    /// Serde skips the keys that the type does not have. They are recorded, so a misspelled key
    /// is reported.
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TreeError> {
        self.unknown.borrow_mut().push(self.path);
        visitor.visit_unit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::policy::{AuthPolicy, RatePeriod};
    use r3v3rs3_api::proxy::{HttpProxy, TcpProxy};
    use std::time::Duration;

    fn tree(pairs: &[(&str, &str)]) -> Node {
        let mut node = Node::default();
        for (path, value) in pairs {
            let path = path.split('.').collect::<Vec<_>>();
            assert!(node.insert(&path, value), "{path:?}");
        }
        node
    }

    #[test]
    fn text_values_become_numbers_booleans_durations_and_lists() {
        let node = tree(&[
            ("vhosts", "app.example.com, www.example.com"),
            ("routes.0.path", "/"),
            ("routes.0.servers.0.url", "http://10.0.0.5:8080"),
            ("routes.0.servers.0.weight", "3"),
            ("circuit_breaker.failure_ratio", "25"),
            ("retry.retry_on", "connect, http_503"),
            ("load_balancing", "client_ip_hash"),
            ("sticky.enabled", "true"),
            ("sticky.max_age", "30m"),
            ("routes.1.path", "/api"),
            ("routes.1.servers.0.url", "http://10.0.0.6:8080"),
            ("upgrade_insecure", "false"),
            ("rate_limit.requests", "100"),
            ("rate_limit.per", "minute"),
            ("timeouts.connect", "3s"),
            ("client_cert", ""),
        ]);
        let (http, unknown) = from_node::<HttpProxy>(node).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(http.vhosts.len(), 2);
        assert_eq!(http.routes[1].path, "/api");
        assert_eq!(
            http.routes[0].servers[0].url.to_string(),
            "http://10.0.0.5:8080/"
        );
        assert_eq!(http.routes[0].servers[0].weight, 3);
        assert_eq!(http.routes[1].servers[0].weight, 1);
        assert_eq!(http.circuit_breaker.failure_ratio, 25);
        assert_eq!(
            http.retry.retry_on,
            [
                r3v3rs3_api::upstream::RetryOn::Connect,
                r3v3rs3_api::upstream::RetryOn::Http503
            ]
        );
        assert_eq!(
            http.load_balancing,
            r3v3rs3_api::upstream::LoadBalancing::ClientIpHash
        );
        assert!(http.sticky.enabled);
        assert_eq!(http.sticky.max_age, Some(Duration::from_secs(1800)));
        assert!(!http.upgrade_insecure);
        assert_eq!(http.rate_limit.requests, 100);
        assert_eq!(http.rate_limit.per, RatePeriod::Minute);
        assert_eq!(http.timeouts.connect, Duration::from_secs(3));
        assert_eq!(http.client_cert, None);
    }

    #[test]
    fn list_keys_keep_their_numeric_order() {
        let node = tree(&[
            ("upstream_servers.10.addr", "/ip4/10.0.0.3/tcp/80"),
            ("upstream_servers.2.addr", "/ip4/10.0.0.2/tcp/80"),
        ]);
        let (tcp, _) = from_node::<TcpProxy>(node).unwrap();
        let addrs = tcp
            .upstream_servers
            .iter()
            .map(|server| server.addr.to_string())
            .collect::<Vec<_>>();
        assert_eq!(addrs, ["/ip4/10.0.0.2/tcp/80", "/ip4/10.0.0.3/tcp/80"]);
    }

    #[test]
    fn tagged_policies_read_their_fields() {
        let node = tree(&[
            ("routes.0.servers.0.url", "http://app:8080"),
            ("auth.type", "basic"),
            ("auth.realm", "Staff"),
            ("auth.users.0.username", "1001"),
            ("auth.users.0.password", "secret"),
            ("headers.request.0.action", "set"),
            ("headers.request.0.name", "X-Env"),
            ("headers.request.0.value", "a, b"),
        ]);
        let (http, _) = from_node::<HttpProxy>(node).unwrap();
        let AuthPolicy::Basic(basic) = &http.auth else {
            panic!("expected basic auth");
        };
        assert_eq!(basic.realm, "Staff");
        assert_eq!(basic.users[0].username, "1001");
        assert_eq!(http.headers.request[0].to_string(), "set X-Env: a, b");
    }

    #[test]
    fn errors_name_the_key_and_unknown_keys_are_reported() {
        let node = tree(&[
            ("routes.0.servers.0.url", "http://app:8080"),
            ("rate_limit.requests", "many"),
        ]);
        let err = from_node::<HttpProxy>(node).unwrap_err();
        assert_eq!(
            err.to_string(),
            "rate_limit.requests: expected a number, found \"many\""
        );

        let node = tree(&[
            ("routes.0.servers.0.url", "http://app:8080"),
            ("routes.0.pth", "/"),
            ("vhost", "app"),
        ]);
        let (_, unknown) = from_node::<HttpProxy>(node).unwrap();
        assert_eq!(unknown, ["routes.0.pth", "vhost"]);

        let err = from_node::<HttpProxy>(tree(&[("vhosts", "app")])).unwrap_err();
        assert_eq!(err.to_string(), "missing field `routes`");
    }

    #[test]
    fn insert_rejects_a_path_that_a_value_uses() {
        let mut node = tree(&[("rate_limit", "10")]);
        assert!(!node.insert(&["rate_limit", "requests"], "10"));
        assert!(!node.insert(&["rate_limit"], "20"));
    }
}
